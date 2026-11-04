//! Tower layer that records one gRPC SERVER span per call.
//!
//! Enable with the `tonic` feature:
//!
//! ```no_run
//! # async fn serve(service: tonic_health::pb::health_server::HealthServer<tonic_health::server::HealthService>) {
//! tonic::transport::Server::builder()
//!     .layer(autotel::tonic::layer())
//!     .add_service(service)
//!     .serve("127.0.0.1:50051".parse().unwrap())
//!     .await
//!     .unwrap();
//! # }
//! ```
//!
//! Behind an axum router instead, add the layer to the router and serve it with
//! `into_make_service_with_connect_info::<SocketAddr>()`, or `client.address` stays empty.
//!
//! A successful call reports `grpc-status` in trailers, after the last message, so
//! the span stays open until the response body finishes. A streaming call's duration
//! is the length of the stream.

use crate::{
    FutureExt,
    core::{OpenSpan, open_span},
    extract_trace_context,
    instruments::record_rpc_server,
    server_attrs::{connection_attributes, header_text},
};
use ::axum::extract::ConnectInfo;
use ::tonic::metadata::MetadataMap;
use http::{HeaderMap, Request, Response, StatusCode};
use http_body::{Body, Frame, SizeHint};
use opentelemetry::trace::SpanKind;
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::{Layer, Service};

const GRPC: &str = "grpc";
const OTHER_METHOD: &str = "_OTHER";
const CANCELLED: i32 = 1;
const UNKNOWN: i32 = 2;
const PERMISSION_DENIED: i32 = 7;
const UNIMPLEMENTED: i32 = 12;
const INTERNAL: i32 = 13;
const UNAVAILABLE: i32 = 14;
const UNAUTHENTICATED: i32 = 16;
const CODE_NAMES: [&str; 17] = [
    "OK",
    "CANCELLED",
    "UNKNOWN",
    "INVALID_ARGUMENT",
    "DEADLINE_EXCEEDED",
    "NOT_FOUND",
    "ALREADY_EXISTS",
    "PERMISSION_DENIED",
    "RESOURCE_EXHAUSTED",
    "FAILED_PRECONDITION",
    "ABORTED",
    "OUT_OF_RANGE",
    "UNIMPLEMENTED",
    "INTERNAL",
    "UNAVAILABLE",
    "DATA_LOSS",
    "UNAUTHENTICATED",
];
/// The codes gRPC semconv counts as errors.
const SERVER_FAULTS: [i32; 6] = [2, 4, 12, 13, 14, 15];

/// Tower layer that joins every call to its caller's trace and records RPC semantics.
#[derive(Clone, Debug, Default)]
pub struct AutotelLayer {
    trust_forwarded_for: bool,
    methods: Arc<[String]>,
}

impl AutotelLayer {
    /// Take `client.address` from `X-Forwarded-For`, for services behind a proxy that
    /// sets it.
    #[must_use]
    pub fn trust_forwarded_for(mut self) -> Self {
        self.trust_forwarded_for = true;
        self
    }

    /// The `package.Service/Method` names this server implements. Any other records as
    /// `_OTHER`. Set it when auth or rate-limit layers answer before tonic routes a call,
    /// so that clients can't add a metric series per made-up path.
    #[must_use]
    pub fn methods<S: Into<String>>(mut self, methods: impl IntoIterator<Item = S>) -> Self {
        self.methods = methods.into_iter().map(Into::into).collect();
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
            options: self.clone(),
        }
    }
}

/// Service produced by [`AutotelLayer`].
#[derive(Clone)]
pub struct AutotelMiddleware<S> {
    inner: S,
    options: AutotelLayer,
}

/// The call's span, ended once. Dropping it means the client went away before
/// a status: the response future or body was abandoned mid-call.
struct CallSpan {
    open: Option<OpenSpan>,
    /// `package.Service/Method`, as the client asked for it.
    method: String,
    recognised: bool,
}

impl CallSpan {
    fn finish(&mut self, code: i32) {
        let Some(open) = self.open.take() else {
            return;
        };
        let status = usize::try_from(code)
            .ok()
            .and_then(|code| CODE_NAMES.get(code))
            .copied()
            .unwrap_or("UNKNOWN");
        let failed = SERVER_FAULTS.contains(&code);
        open.context.set_attribute("rpc.status_code", status);
        if failed {
            open.context.set_attribute("error.type", status);
            open.context.record_error(&status);
        }
        // Clients choose the path, so an unknown one stays out of span names and
        // metric series and lives only as an attribute of this span.
        let mut method = self.method.as_str();
        if code == UNIMPLEMENTED || !self.recognised {
            open.context.rename(GRPC);
            open.context.set_attribute("rpc.method_original", method);
            method = OTHER_METHOD;
        }
        // Set once, here: the SDK keeps every value a span is given for a key.
        open.context.set_attribute("rpc.method", method);
        record_rpc_server(method, status, failed, open.elapsed());
        open.finish();
    }
}

impl Drop for CallSpan {
    fn drop(&mut self) {
        self.finish(if std::thread::panicking() {
            UNKNOWN
        } else {
            CANCELLED
        });
    }
}

fn grpc_status(headers: &HeaderMap) -> Option<i32> {
    headers.get("grpc-status")?.to_str().ok()?.parse().ok()
}

/// gRPC's own mapping, for a router or middleware that answers with plain HTTP.
fn status_from_http(status: StatusCode) -> i32 {
    match status.as_u16() {
        400 => INTERNAL,
        401 => UNAUTHENTICATED,
        403 => PERMISSION_DENIED,
        404 => UNIMPLEMENTED,
        429 | 502 | 503 | 504 => UNAVAILABLE,
        _ => UNKNOWN,
    }
}

/// A status the response carries before any body.
fn status_without_body<B>(response: &Response<B>) -> Option<i32> {
    let status = response.status();
    grpc_status(response.headers())
        .or_else(|| (!status.is_success()).then(|| status_from_http(status)))
}

/// Whether `/package.Service/Method`, the only shape a gRPC path has.
fn is_method_path(path: &str) -> bool {
    path.strip_prefix('/')
        .and_then(|path| path.split_once('/'))
        .is_some_and(|(service, method)| {
            !service.is_empty() && !method.is_empty() && !method.contains('/')
        })
}

/// tonic's own lookup, because a TLS stream the app accepted itself leaves only
/// `TlsConnectInfo` behind. `axum::serve` records the peer its own way, and only with
/// connect info.
fn peer_address<B>(request: &mut Request<B>) -> Option<SocketAddr> {
    let extensions = std::mem::take(request.extensions_mut());
    let tonic_request = ::tonic::Request::from_parts(MetadataMap::new(), extensions, ());
    let peer = tonic_request.remote_addr();
    let (_, extensions, ()) = tonic_request.into_parts();
    *request.extensions_mut() = extensions;
    peer.or_else(|| {
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer)
    })
}

/// `application/grpc`, with or without a codec or parameters. gRPC-Web carries its
/// status in the body, so it isn't a call this layer can finish.
fn is_grpc_content_type(content_type: &str) -> bool {
    content_type
        .strip_prefix("application/grpc")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['+', ';']))
}

fn open_call<B>(request: &mut Request<B>, options: &AutotelLayer) -> CallSpan {
    let is_grpc = header_text(request, "content-type").is_some_and(is_grpc_content_type);
    let path = request.uri().path();
    if !is_grpc || !is_method_path(path) {
        return CallSpan {
            open: None,
            method: String::new(),
            recognised: false,
        };
    }
    let method = path.trim_start_matches('/').to_owned();
    let parent = extract_trace_context(request.headers());
    let open = open_span(method.clone(), SpanKind::Server, &parent);
    let peer = peer_address(request);
    open.context.set_attribute("rpc.system.name", GRPC);
    open.context.set_attributes(connection_attributes(
        request,
        peer,
        options.trust_forwarded_for,
    ));
    let recognised = options.methods.is_empty() || options.methods.contains(&method);
    CallSpan {
        open: Some(open),
        method,
        recognised,
    }
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for AutotelMiddleware<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Send + 'static,
{
    type Response = Response<TracedBody<ResBody>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request<ReqBody>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let mut span = open_call(&mut request, &self.options);
        let scope = span.open.as_ref().map(|open| open.scope.clone());
        Box::pin(async move {
            // `call` itself goes inside: a service can log or read the context there,
            // before its future is ever polled.
            let call = async move { inner.call(request).await };
            let result = match scope {
                Some(scope) => call.with_context(scope).await,
                None => call.await,
            };
            let response = match result {
                Ok(response) => response,
                Err(error) => {
                    span.finish(UNKNOWN);
                    return Err(error);
                }
            };
            // A call that ends before its first message reports its status in the headers.
            span.recognised &= response.status().is_success();
            if let Some(code) = status_without_body(&response) {
                span.finish(code);
            }
            Ok(response.map(|inner| TracedBody {
                inner: Box::pin(inner),
                span,
            }))
        })
    }
}

/// Response body that ends the call's span when the stream does.
pub struct TracedBody<B> {
    inner: Pin<Box<B>>,
    span: CallSpan,
}

impl<B: Body> Body for TracedBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let polled = {
            // A streaming handler runs here, so its spans need the call as their parent.
            let _scope = this
                .span
                .open
                .as_ref()
                .map(|open| open.scope.clone().attach());
            this.inner.as_mut().poll_frame(cx)
        };
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(trailers) = frame.trailers_ref() {
                    this.span.finish(grpc_status(trailers).unwrap_or(UNKNOWN));
                }
            }
            Poll::Ready(Some(Err(_)) | None) => this.span.finish(UNKNOWN),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
