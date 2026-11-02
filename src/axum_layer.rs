//! Axum middleware that extracts W3C context and starts a SERVER HTTP span.
//!
//! Enable with the `axum` feature (on by default):
//!
//! ```
//! use axum::{Router, routing::get};
//!
//! let app: Router = Router::new()
//!     .route("/orders/{id}", get(|| async { "order" }))
//!     .layer(autotel::axum::layer());
//! ```

use crate::{FutureExt, extract_trace_context, instrumentation::http_span};
use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use opentelemetry::trace::SpanKind;
use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// Tower layer that joins every request to its caller's trace and records HTTP semantics.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutotelLayer;

/// Same as [`AutotelLayer::default`].
#[must_use]
pub fn layer() -> AutotelLayer {
    AutotelLayer
}

impl<S> Layer<S> for AutotelLayer {
    type Service = AutotelMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AutotelMiddleware { inner }
    }
}

/// Service produced by [`AutotelLayer`].
#[derive(Clone)]
pub struct AutotelMiddleware<S> {
    inner: S,
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
        Box::pin(serve(request, move |request| inner.call(request)))
    }
}

/// Same work as [`layer`], for `Router::layer(axum::middleware::from_fn(autotel::axum::middleware))`.
pub async fn middleware(request: Request, next: Next) -> Response {
    let Ok(response) = serve(request, |request| async move {
        Ok::<_, Infallible>(next.run(request).await)
    })
    .await;
    response
}

async fn serve<E, F, Fut>(request: Request, run: F) -> Result<Response, E>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Response, E>>,
{
    let incoming = extract_trace_context(request.headers());
    let method = request.method().to_string();
    // `MatchedPath` is absent for unmatched requests (404s). Never fall back to
    // the raw path: scanners would mint one span name and metric series per URL.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());

    async move {
        http_span(
            &method,
            route.as_deref(),
            SpanKind::Server,
            |context| async move {
                let response = run(request).await?;
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
