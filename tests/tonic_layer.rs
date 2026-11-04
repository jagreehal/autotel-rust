//! Real gRPC over TCP: `tonic-health` ships generated code, so no `protoc` is needed.
//! Serialized: `init` owns a process-wide tracer provider and span buffer.

#![cfg(feature = "tonic")]

use autotel::{SpanData, TraceCollector, inject_trace_context, trace};
use bytes::Bytes;
use http_body::{Body, Frame};
use http_body_util::{BodyExt, Empty, StreamBody};
use serde_json::json;
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use tonic::{
    Code, Request,
    metadata::MetadataMap,
    transport::{Certificate, Channel, ClientTlsConfig, Identity, Server, ServerTlsConfig},
};
use tonic_health::{
    ServingStatus,
    pb::{HealthCheckRequest, health_client::HealthClient},
    server::{HealthReporter, health_reporter},
};
use tower::{Layer, Service, ServiceExt, service_fn};

static SERIAL: Mutex<()> = Mutex::new(());

struct Scope {
    _guard: MutexGuard<'static, ()>,
    collector: TraceCollector,
}

impl Scope {
    fn open() -> Self {
        let guard = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            _guard: guard,
            collector: TraceCollector::new("grpc").unwrap(),
        }
    }
}

async fn start() -> (HealthClient<Channel>, HealthReporter, SocketAddr) {
    let (reporter, service) = health_reporter();
    reporter
        .set_service_status("orders", ServingStatus::Serving)
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(
        Server::builder()
            .layer(autotel::tonic::layer())
            .add_service(service)
            .serve_with_incoming(incoming),
    );
    let channel = Channel::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    (HealthClient::new(channel), reporter, address)
}

fn check(service: &str) -> HealthCheckRequest {
    HealthCheckRequest {
        service: service.into(),
    }
}

/// Spans end on the server task, a moment after the client hears the trailers.
async fn span_named(collector: &TraceCollector, name: &str) -> SpanData {
    for _ in 0..200 {
        if let Some(span) = collector.by_name(name).pop() {
            return span;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no span {name}: {:?}", collector.spans());
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_status_comes_from_trailers_and_the_call_joins_the_callers_trace() {
    let scope = Scope::open();
    let collector = &scope.collector;
    let (mut client, _reporter, address) = start().await;

    trace("caller", |_| async {
        let mut request = Request::new(check("orders"));
        *request.metadata_mut() = MetadataMap::from_headers(inject_trace_context());
        client.check(request).await.unwrap();
    })
    .await;

    let span = span_named(collector, "grpc.health.v1.Health/Check").await;
    let caller = collector.expect_span("caller");
    assert_eq!(
        span.parent_span_id.as_deref(),
        Some(caller.span_id.as_str())
    );
    assert_eq!(span.attributes["otel.span.kind"], json!("server"));
    assert_eq!(span.attributes["rpc.system.name"], json!("grpc"));
    assert_eq!(
        span.attributes["rpc.method"],
        json!("grpc.health.v1.Health/Check")
    );
    assert_eq!(span.attributes["rpc.status_code"], json!("OK"));
    assert!(!span.attributes.contains_key("error.type"));
    assert_eq!(span.attributes["network.protocol.version"], json!("2"));
    assert_eq!(span.attributes["server.port"], json!(address.port()));
    assert_eq!(span.attributes["client.address"], json!("127.0.0.1"));
    assert_eq!(span.error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_status_in_the_headers_is_recorded_and_not_found_is_not_a_server_fault() {
    let scope = Scope::open();
    let collector = &scope.collector;
    let (mut client, _reporter, _) = start().await;

    let status = client.check(check("missing")).await.unwrap_err();

    assert_eq!(status.code(), Code::NotFound);
    let span = span_named(collector, "grpc.health.v1.Health/Check").await;
    assert_eq!(span.attributes["rpc.status_code"], json!("NOT_FOUND"));
    assert!(!span.attributes.contains_key("error.type"));
    assert_eq!(span.error, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_is_one_span_as_long_as_the_stream_and_a_cancel_reads_as_cancelled() {
    let scope = Scope::open();
    let collector = &scope.collector;
    let (mut client, _reporter, _) = start().await;

    let mut stream = client.watch(check("orders")).await.unwrap().into_inner();
    stream.message().await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        collector.by_name("grpc.health.v1.Health/Watch").is_empty(),
        "ended at the first message"
    );
    drop(stream);

    let span = span_named(collector, "grpc.health.v1.Health/Watch").await;
    assert_eq!(span.attributes["rpc.status_code"], json!("CANCELLED"));
    assert!(
        span.duration >= Duration::from_millis(150),
        "{:?}",
        span.duration
    );
}

/// A body that delivers `grpc-status` in trailers, the way a finished call does.
fn with_trailers(status: &'static str) -> impl Body<Data = Bytes, Error = Infallible> {
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", status.parse().unwrap());
    let frames = [
        Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"x"))),
        Ok(Frame::trailers(trailers)),
    ];
    StreamBody::new(tokio_stream::iter(frames))
}

fn grpc_request(path: &str) -> http::Request<()> {
    http::Request::builder()
        .uri(path)
        .header("content-type", "application/grpc")
        .body(())
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn server_fault_in_trailers_marks_the_span_as_an_error() {
    let scope = Scope::open();
    let collector = &scope.collector;
    let service =
        service_fn(|_| async { Ok::<_, Infallible>(http::Response::new(with_trailers("13"))) });

    let response = autotel::tonic::layer()
        .layer(service)
        .oneshot(grpc_request("/a.B/C"))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();

    let span = collector.expect_span("a.B/C");
    assert_eq!(span.attributes["rpc.status_code"], json!("INTERNAL"));
    assert_eq!(span.attributes["error.type"], json!("INTERNAL"));
    assert_eq!(span.error.as_deref(), Some("INTERNAL"));
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_methods_cannot_mint_span_names() {
    let scope = Scope::open();
    let collector = &scope.collector;
    let service = service_fn(|_| async {
        let response = http::Response::builder().header("grpc-status", "12");
        Ok::<_, Infallible>(response.body(Empty::<Bytes>::new()).unwrap())
    });

    autotel::tonic::layer()
        .layer(service)
        .oneshot(grpc_request("/scan.Me/Random123"))
        .await
        .unwrap();

    assert!(collector.by_name("scan.Me/Random123").is_empty());
    let span = collector.expect_span("grpc");
    assert_eq!(span.attributes["rpc.method"], json!("_OTHER"));
    assert_eq!(
        span.attributes["rpc.method_original"],
        json!("scan.Me/Random123")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn plain_http_through_the_layer_records_nothing() {
    let scope = Scope::open();
    let collector = &scope.collector;
    let mut service = autotel::tonic::layer().layer(service_fn(|_: http::Request<()>| async {
        Ok::<_, Infallible>(http::Response::new(Empty::<Bytes>::new()))
    }));

    let request = http::Request::builder().uri("/a/b").body(()).unwrap();
    service.ready().await.unwrap().call(request).await.unwrap();

    assert!(collector.spans().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_bare_404_from_a_router_counts_as_an_unknown_method() {
    let scope = Scope::open();
    let service = service_fn(|_| async {
        let response = http::Response::builder().status(404);
        Ok::<_, Infallible>(response.body(Empty::<Bytes>::new()).unwrap())
    });

    autotel::tonic::layer()
        .layer(service)
        .oneshot(grpc_request("/scan.Me/Random123"))
        .await
        .unwrap();

    let span = scope.collector.expect_span("grpc");
    assert_eq!(span.attributes["rpc.status_code"], json!("UNIMPLEMENTED"));
}

async fn check_orders(channel: Channel) {
    HealthClient::new(channel)
        .check(check("orders"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_is_known_when_tonic_is_served_through_an_axum_router() {
    let scope = Scope::open();
    let (reporter, service) = health_reporter();
    reporter
        .set_service_status("orders", ServingStatus::Serving)
        .await;
    let router = tonic::service::Routes::new(service)
        .prepare()
        .into_axum_router()
        .layer(autotel::tonic::layer());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router.into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let channel = Channel::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    check_orders(channel).await;

    let span = span_named(&scope.collector, "grpc.health.v1.Health/Check").await;
    assert_eq!(span.attributes["client.address"], json!("127.0.0.1"));
    assert!(span.attributes["network.peer.port"].as_u64().unwrap() > 0);
    assert_eq!(span.attributes["rpc.status_code"], json!("OK"));
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_is_known_over_tls() {
    let scope = Scope::open();
    let cert = rcgen::generate_simple_self_signed(["localhost".to_owned()]).unwrap();
    let (pem, key) = (cert.cert.pem(), cert.signing_key.serialize_pem());
    let (reporter, service) = health_reporter();
    reporter
        .set_service_status("orders", ServingStatus::Serving)
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let tls = ServerTlsConfig::new().identity(Identity::from_pem(&pem, key));
    tokio::spawn(
        Server::builder()
            .tls_config(tls)
            .unwrap()
            .layer(autotel::tonic::layer())
            .add_service(service)
            .serve_with_incoming(incoming),
    );
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&pem))
        .domain_name("localhost");
    let channel = Channel::from_shared(format!("https://127.0.0.1:{port}"))
        .unwrap()
        .tls_config(tls)
        .unwrap()
        .connect()
        .await
        .unwrap();

    check_orders(channel).await;

    let span = span_named(&scope.collector, "grpc.health.v1.Health/Check").await;
    assert_eq!(span.attributes["client.address"], json!("127.0.0.1"));
}

/// tonic's own TLS also records the TCP peer. A stream the app accepts itself does not,
/// so only `TlsConnectInfo` knows who called.
#[tokio::test(flavor = "multi_thread")]
async fn peer_is_known_for_a_tls_stream_the_app_accepts_itself() {
    use tokio_rustls::{
        TlsAcceptor,
        rustls::{
            ServerConfig,
            pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
        },
    };
    let scope = Scope::open();
    let cert = rcgen::generate_simple_self_signed(["localhost".to_owned()]).unwrap();
    let key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], PrivateKeyDer::Pkcs8(key))
        .unwrap();
    config.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = TlsAcceptor::from(std::sync::Arc::new(config));
    let (reporter, service) = health_reporter();
    reporter
        .set_service_status("orders", ServingStatus::Serving)
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (accepted, incoming) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(stream).await;
            accepted.send(tls).await.unwrap();
        }
    });
    tokio::spawn(
        Server::builder()
            .layer(autotel::tonic::layer())
            .add_service(service)
            .serve_with_incoming(tokio_stream::wrappers::ReceiverStream::new(incoming)),
    );
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(cert.cert.pem()))
        .domain_name("localhost");
    let channel = Channel::from_shared(format!("https://127.0.0.1:{port}"))
        .unwrap()
        .tls_config(tls)
        .unwrap()
        .connect()
        .await
        .unwrap();

    check_orders(channel).await;

    let span = span_named(&scope.collector, "grpc.health.v1.Health/Check").await;
    assert_eq!(span.attributes["client.address"], json!("127.0.0.1"));
}

/// Middleware under this layer may log or read the context in `call` itself, before
/// its future is ever polled, so the call has to happen inside the span too.
#[tokio::test(flavor = "current_thread")]
async fn the_inner_service_is_called_inside_the_span() {
    let scope = Scope::open();
    let seen = std::sync::Arc::new(Mutex::new(None));
    let captured = seen.clone();
    let service = service_fn(move |_| {
        *captured.lock().unwrap() = autotel::current_trace_id();
        async { Ok::<_, Infallible>(http::Response::new(with_trailers("0"))) }
    });

    let response = autotel::tonic::layer()
        .layer(service)
        .oneshot(grpc_request("/a.B/C"))
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();

    let span = scope.collector.expect_span("a.B/C");
    assert_eq!(*seen.lock().unwrap(), Some(span.trace_id));
}

async fn call_through_layer(
    layer: autotel::tonic::AutotelLayer,
    path: &str,
    status: u16,
    grpc_status: Option<&'static str>,
) -> http::Response<autotel::tonic::TracedBody<Empty<Bytes>>> {
    let service = service_fn(move |_| async move {
        let mut response = http::Response::builder().status(status);
        if let Some(code) = grpc_status {
            response = response.header("grpc-status", code);
        }
        Ok::<_, Infallible>(response.body(Empty::<Bytes>::new()).unwrap())
    });
    layer
        .layer(service)
        .oneshot(grpc_request(path))
        .await
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_method_list_keeps_paths_a_layer_rejected_out_of_span_names() {
    let scope = Scope::open();
    let layer = autotel::tonic::layer().methods(["orders.Orders/Get"]);

    drop(call_through_layer(layer.clone(), "/orders.Orders/Get", 200, Some("16")).await);
    drop(call_through_layer(layer, "/orders.Orders/Made123", 200, Some("16")).await);

    let known = scope.collector.expect_span("orders.Orders/Get");
    assert_eq!(
        known.attributes["rpc.status_code"],
        json!("UNAUTHENTICATED")
    );
    let unknown = scope.collector.expect_span("grpc");
    assert_eq!(unknown.attributes["rpc.method"], json!("_OTHER"));
    assert_eq!(
        unknown.attributes["rpc.method_original"],
        json!("orders.Orders/Made123")
    );
}

/// hyper stops at an empty body without polling it, so the status has to come from the head.
#[tokio::test(flavor = "current_thread")]
async fn plain_http_rejections_use_grpcs_own_mapping_with_the_body_unread() {
    let scope = Scope::open();
    for (status, expected) in [
        (401, "UNAUTHENTICATED"),
        (403, "PERMISSION_DENIED"),
        (429, "UNAVAILABLE"),
        (503, "UNAVAILABLE"),
        (500, "UNKNOWN"),
    ] {
        drop(call_through_layer(autotel::tonic::layer(), "/a.B/C", status, None).await);
        let span = scope.collector.spans().pop().unwrap();
        assert_eq!(span.name, "grpc");
        assert_eq!(span.attributes["rpc.status_code"], json!(expected));
        assert_eq!(span.attributes["rpc.method"], json!("_OTHER"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_handler_that_panics_is_a_server_fault_not_a_cancelled_call() {
    let scope = Scope::open();
    let service = service_fn(|_: http::Request<()>| async {
        if true {
            panic!("handler failed");
        }
        Ok::<_, Infallible>(http::Response::new(Empty::<Bytes>::new()))
    });

    let call = autotel::tonic::layer()
        .layer(service)
        .oneshot(grpc_request("/a.B/C"));
    assert!(tokio::spawn(call).await.is_err());

    let span = scope.collector.expect_span("a.B/C");
    assert_eq!(span.attributes["rpc.status_code"], json!("UNKNOWN"));
    assert_eq!(span.attributes["error.type"], json!("UNKNOWN"));
}

#[tokio::test(flavor = "current_thread")]
async fn grpc_web_passes_through_and_codecs_and_parameters_count_as_grpc() {
    let scope = Scope::open();
    for content_type in ["application/grpc-web", "application/grpc-web+proto"] {
        let request = http::Request::builder()
            .uri("/a.B/C")
            .header("content-type", content_type)
            .body(())
            .unwrap();
        let service = service_fn(|_| async {
            Ok::<_, Infallible>(http::Response::new(Empty::<Bytes>::new()))
        });
        drop(
            autotel::tonic::layer()
                .layer(service)
                .oneshot(request)
                .await,
        );
    }
    assert!(scope.collector.spans().is_empty());

    for content_type in ["application/grpc+proto", "application/grpc; charset=utf-8"] {
        let request = http::Request::builder()
            .uri("/a.B/C")
            .header("content-type", content_type)
            .body(())
            .unwrap();
        let service =
            service_fn(|_| async { Ok::<_, Infallible>(http::Response::new(with_trailers("0"))) });
        let response = autotel::tonic::layer()
            .layer(service)
            .oneshot(request)
            .await
            .unwrap();
        response.into_body().collect().await.unwrap();
    }
    assert_eq!(scope.collector.by_name("a.B/C").len(), 2);
}
