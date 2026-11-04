//! Axum middleware that extracts W3C context and starts a SERVER HTTP span.
//!
//! Enable with the `axum` feature:
//!
//! ```
//! use axum::{Router, routing::get};
//!
//! let app: Router = Router::new()
//!     .route("/orders/{id}", get(|| async { "order" }))
//!     .route("/health", get(|| async { "ok" }))
//!     .layer(autotel::axum::layer().skip_routes(["/health"]).trace_response_header());
//! ```

use crate::{
    FutureExt, extract_trace_context, inject_trace_context,
    instrumentation::{http_span, redact_query},
    server_attrs::{connection_attributes, header_text},
};
use axum::{
    extract::{ConnectInfo, MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use opentelemetry::trace::SpanKind;
use serde_json::Value;
use std::{
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// Tower layer that joins every request to its caller's trace and records HTTP semantics.
#[derive(Clone, Debug, Default)]
pub struct AutotelLayer {
    options: Options,
}

#[derive(Clone, Debug, Default)]
struct Options {
    skip_routes: Arc<[String]>,
    response_header: bool,
    trust_forwarded_for: bool,
    raw_query: bool,
}

impl AutotelLayer {
    /// Record nothing for these route templates, such as health checks and probes.
    #[must_use]
    pub fn skip_routes<S: Into<String>>(mut self, routes: impl IntoIterator<Item = S>) -> Self {
        self.options.skip_routes = routes.into_iter().map(Into::into).collect();
        self
    }

    /// Answer with a `traceparent` header, so a caller can quote the trace in a support
    /// ticket. Off by default: it tells clients your trace ids. Baggage is never echoed.
    #[must_use]
    pub fn trace_response_header(mut self) -> Self {
        self.options.response_header = true;
        self
    }

    /// Take `client.address` from `X-Forwarded-For` and `url.scheme` from
    /// `X-Forwarded-Proto`, for services behind a proxy that sets them. By default
    /// `client.address` is the TCP peer, which needs `into_make_service_with_connect_info`.
    #[must_use]
    pub fn trust_forwarded_for(mut self) -> Self {
        self.options.trust_forwarded_for = true;
        self
    }

    /// Record `url.query` as sent. By default the keys stay and the values read
    /// `REDACTED`, because query strings carry tokens and search terms.
    #[must_use]
    pub fn raw_query(mut self) -> Self {
        self.options.raw_query = true;
        self
    }
}

/// Same as [`AutotelLayer::default`].
#[must_use]
pub fn layer() -> AutotelLayer {
    AutotelLayer::default()
}

impl<S> Layer<S> for AutotelLayer {
    type Service = AutotelMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AutotelMiddleware {
            inner,
            options: self.options.clone(),
        }
    }
}

/// Service produced by [`AutotelLayer`].
#[derive(Clone)]
pub struct AutotelMiddleware<S> {
    inner: S,
    options: Options,
}

impl<S> Service<Request> for AutotelMiddleware<S>
where
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let options = self.options.clone();
        Box::pin(serve(request, options, move |request| inner.call(request)))
    }
}

/// Same work as [`layer`], for `Router::layer(axum::middleware::from_fn(autotel::axum::middleware))`.
pub async fn middleware(request: Request, next: Next) -> Response {
    let Ok(response) = serve(request, Options::default(), |request| async move {
        Ok::<_, Infallible>(next.run(request).await)
    })
    .await;
    response
}

fn request_attributes(request: &Request, options: &Options) -> Vec<(String, Value)> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| *peer);
    let mut attributes = connection_attributes(request, peer, options.trust_forwarded_for);
    let uri = request.uri();
    let forwarded_scheme = options
        .trust_forwarded_for
        .then(|| header_text(request, "x-forwarded-proto"))
        .flatten()
        .filter(|scheme| matches!(*scheme, "http" | "https"));
    let scheme = uri.scheme_str().or(forwarded_scheme).unwrap_or("http");
    attributes.push(("url.path".to_owned(), Value::from(uri.path())));
    attributes.push(("url.scheme".to_owned(), Value::from(scheme)));
    if let Some(query) = uri.query() {
        let query = if options.raw_query {
            query.to_owned()
        } else {
            redact_query(query)
        };
        attributes.push(("url.query".to_owned(), Value::from(query)));
    }
    attributes
}

async fn serve<E, F, Fut>(request: Request, options: Options, run: F) -> Result<Response, E>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Response, E>>,
{
    // `MatchedPath` is absent for unmatched requests (404s). Never fall back to
    // the raw path: scanners would mint one span name and metric series per URL.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    if route
        .as_ref()
        .is_some_and(|route| options.skip_routes.contains(route))
    {
        return run(request).await;
    }
    let incoming = extract_trace_context(request.headers());
    let method = request.method().to_string();
    let attributes = request_attributes(&request, &options);

    async move {
        http_span(
            &method,
            route.as_deref(),
            SpanKind::Server,
            |context| async move {
                context.set_attributes(attributes);
                let mut response = run(request).await?;
                if options.response_header
                    && let Some(traceparent) = inject_trace_context().remove("traceparent")
                {
                    response.headers_mut().insert("traceparent", traceparent);
                }
                let status = response.status().as_u16();
                context.set_attribute("http.response.status_code", status);
                if response.status().is_server_error() {
                    context.record_error(&format!("HTTP {status}"));
                }
                Ok(response)
            },
        )
        .await
    }
    .with_context(incoming)
    .await
}
