//! HTTP server/client helpers, Axum layer, reqwest inject, and OTLP endpoint resolution.
//! Serialized: `init` owns a process-wide tracer provider and span buffer.

#![cfg(feature = "axum")]

use autotel::{
    AutotelConfig, FutureExt, OtlpEndpoints, OtlpProtocol, TraceCollector, extract_trace_context,
    fetch, http_client, http_server, inject_request, inject_trace_context, trace,
};
use axum::{Router, http::StatusCode, routing::get};
use serde_json::json;
use std::sync::{Mutex, MutexGuard};
use tower::util::ServiceExt;

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
            collector: TraceCollector::new("http-integrations").unwrap(),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_server_records_server_kind_and_status() {
    let scope = Scope::open();
    let status = http_server("GET", "/orders/{id}", |context| async move {
        context.set_attribute("http.response.status_code", 200_u16);
        200_u16
    })
    .await;
    assert_eq!(status, 200);
    let span = scope.collector.expect_span("GET /orders/{id}");
    assert_eq!(span.attributes["otel.span.kind"], json!("server"));
    assert_eq!(span.attributes["http.request.method"], json!("GET"));
    assert_eq!(span.attributes["http.route"], json!("/orders/{id}"));
    assert_eq!(span.attributes["http.response.status_code"], json!(200));
}

#[tokio::test(flavor = "current_thread")]
async fn http_client_records_client_kind_and_url() {
    let scope = Scope::open();
    http_client("POST", "http://inventory/reserve", |_| async {}).await;
    // Client spans are named after the method only: URLs carry ids and queries.
    let span = scope.collector.expect_span("POST");
    assert_eq!(span.attributes["otel.span.kind"], json!("client"));
    assert_eq!(
        span.attributes["url.full"],
        json!("http://inventory/reserve")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn client_urls_drop_credentials_and_query_values() {
    let scope = Scope::open();
    http_client(
        "GET",
        "https://svc:hunter2@api.example.com/v1/items?token=abc123&page=2#frag",
        |_| async {},
    )
    .await;
    let span = scope.collector.expect_span("GET");
    assert_eq!(
        span.attributes["url.full"],
        json!("https://api.example.com/v1/items?token=REDACTED&page=REDACTED")
    );
    assert_eq!(span.attributes["server.address"], json!("api.example.com"));
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_methods_become_other() {
    let scope = Scope::open();
    http_server("BREW", "/coffee", |_| async {}).await;
    let span = scope.collector.expect_span("_OTHER /coffee");
    assert_eq!(span.attributes["http.request.method"], json!("_OTHER"));
    assert_eq!(
        span.attributes["http.request.method_original"],
        json!("BREW")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_errors_do_not_leak_the_url() {
    let scope = Scope::open();
    // Port 9 (discard) on localhost: nothing listens, so the connect fails fast.
    let result = fetch(
        reqwest::Method::GET,
        "http://127.0.0.1:9/reset?token=s3cr3t",
        |request| request,
    )
    .await;
    assert!(result.is_err());
    let span = scope.collector.expect_span("GET");
    let error = span.error.expect("error recorded");
    assert!(!error.contains("s3cr3t"), "{error}");
    assert_eq!(span.attributes["error.type"], json!("connect"));
}

#[tokio::test(flavor = "current_thread")]
async fn inject_request_writes_traceparent() {
    let _scope = Scope::open();
    let (headers, request) = trace("parent", |_| async {
        let headers = inject_trace_context();
        let request = inject_request(reqwest::Client::new().get("http://example.test/"))
            .build()
            .unwrap();
        (headers, request)
    })
    .await;
    assert!(headers.contains_key("traceparent"));
    assert!(request.headers().contains_key("traceparent"));
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_joins_caller_trace_and_records_route() {
    let scope = Scope::open();

    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());

    trace("gateway", |_| async {
        let mut request = axum::http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        *request.headers_mut() = inject_trace_context();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    })
    .await;

    let server = scope.collector.expect_span("GET /health");
    assert_eq!(server.attributes["otel.span.kind"], json!("server"));
    assert_eq!(server.attributes["http.response.status_code"], json!(200));
    let gateway = scope.collector.expect_span("gateway");
    assert_eq!(
        server.parent_span_id.as_deref(),
        Some(gateway.span_id.as_str())
    );
    assert_eq!(server.trace_id, gateway.trace_id);
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_marks_5xx_as_error() {
    let scope = Scope::open();
    let app = Router::new()
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .layer(autotel::axum::layer());

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/boom")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let span = scope.collector.expect_span("GET /boom");
    assert!(
        span.error
            .as_deref()
            .is_some_and(|error| error.contains("500"))
    );
    assert_eq!(span.attributes["http.response.status_code"], json!(500));
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_propagates_to_local_server() {
    let scope = Scope::open();

    let app = Router::new().route(
        "/ping",
        get(|request: axum::extract::Request| async move {
            let incoming = extract_trace_context(request.headers());
            async move {
                http_server("GET", "/ping", |ctx| async move {
                    ctx.set_attribute("seen", true);
                    "pong"
                })
                .await
            }
            .with_context(incoming)
            .await
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("http://{addr}/ping");
    let response = fetch(reqwest::Method::GET, &url, |request| request)
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "pong");

    let client = scope
        .collector
        .by_name("GET")
        .into_iter()
        .next()
        .expect("client span");
    assert_eq!(client.attributes["otel.span.kind"], json!("client"));
    assert_eq!(client.attributes["http.response.status_code"], json!(200));
    assert_eq!(client.attributes["server.address"], json!("127.0.0.1"));

    let server = scope.collector.expect_span("GET /ping");
    assert_eq!(server.attributes["seen"], json!(true));
    assert_eq!(
        server.parent_span_id.as_deref(),
        Some(client.span_id.as_str())
    );
    assert_eq!(server.trace_id, client.trace_id);
}

#[test]
fn otlp_endpoints_append_http_paths() {
    let config = AutotelConfig::new("svc").devtools();
    assert_eq!(
        config.otlp_endpoints(),
        Some(OtlpEndpoints {
            traces: "http://127.0.0.1:4318/v1/traces".into(),
            metrics: "http://127.0.0.1:4318/v1/metrics".into(),
            logs: "http://127.0.0.1:4318/v1/logs".into(),
            protocol: OtlpProtocol::HttpJson,
        })
    );
}

#[test]
fn otlp_per_signal_overrides_win() {
    let config = AutotelConfig::new("svc")
        .devtools()
        .traces_endpoint("http://jaeger:4318/v1/traces")
        .metrics_endpoint("http://prom:9090/api/v1/otlp/v1/metrics")
        .logs_endpoint("http://loki:3100/otlp/v1/logs");
    let endpoints = config.otlp_endpoints().unwrap();
    assert_eq!(endpoints.traces, "http://jaeger:4318/v1/traces");
    assert_eq!(endpoints.metrics, "http://prom:9090/api/v1/otlp/v1/metrics");
    assert_eq!(endpoints.logs, "http://loki:3100/otlp/v1/logs");
}

#[cfg(feature = "grpc")]
#[test]
fn otlp_grpc_with_devtools_falls_back_to_http() {
    let config = AutotelConfig::new("svc").devtools().grpc();
    let endpoints = config.otlp_endpoints().unwrap();
    assert_eq!(endpoints.protocol, OtlpProtocol::HttpJson);
    assert_eq!(endpoints.traces, "http://127.0.0.1:4318/v1/traces");
}

#[cfg(feature = "grpc")]
#[test]
fn otlp_grpc_keeps_base_endpoint() {
    let config = AutotelConfig {
        otlp_endpoint: Some("http://collector:4317".into()),
        otlp_protocol: OtlpProtocol::Grpc,
        ..AutotelConfig::new("svc")
    };
    let endpoints = config.otlp_endpoints().unwrap();
    assert_eq!(endpoints.traces, "http://collector:4317");
    assert_eq!(endpoints.metrics, "http://collector:4317");
    assert_eq!(endpoints.logs, "http://collector:4317");
    assert_eq!(endpoints.protocol, OtlpProtocol::Grpc);
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_does_not_use_raw_path_for_unmatched_routes() {
    let scope = Scope::open();
    let app = Router::new()
        .route("/users/{id}", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());

    for uri in ["/users/42", "/wp-admin.php"] {
        let request = axum::http::Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap();
        app.clone().oneshot(request).await.unwrap();
    }

    let matched = scope.collector.expect_span("GET /users/{id}");
    assert_eq!(matched.attributes["http.route"], json!("/users/{id}"));
    let unmatched = scope.collector.expect_span("GET");
    assert!(!unmatched.attributes.contains_key("http.route"));
    assert_eq!(
        unmatched.attributes["http.response.status_code"],
        json!(404)
    );
}

fn get_request(uri: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .uri(uri)
        .header("host", "api.example.com:8443")
        .header("user-agent", "curl/8.0")
        .body(axum::body::Body::empty())
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_records_http_semconv_and_no_query_values() {
    let scope = Scope::open();
    let app = Router::new()
        .route("/orders/{id}", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());

    app.oneshot(get_request("/orders/42?token=secret"))
        .await
        .unwrap();

    let span = scope.collector.expect_span("GET /orders/{id}");
    assert_eq!(span.attributes["url.path"], json!("/orders/42"));
    assert_eq!(span.attributes["url.scheme"], json!("http"));
    assert_eq!(span.attributes["server.address"], json!("api.example.com"));
    assert_eq!(span.attributes["server.port"], json!(8443));
    assert_eq!(span.attributes["user_agent.original"], json!("curl/8.0"));
    assert_eq!(span.attributes["network.protocol.version"], json!("1.1"));
    assert!(!format!("{:?}", span.attributes).contains("secret"));
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_names_nested_routes_by_their_full_template() {
    let scope = Scope::open();
    let api = Router::new().route("/orders/{id}", get(|| async { StatusCode::OK }));
    let app = Router::new().nest("/v1", api).layer(autotel::axum::layer());

    app.oneshot(get_request("/v1/orders/42")).await.unwrap();

    scope.collector.expect_span("GET /v1/orders/{id}");
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_skips_named_routes_only() {
    let scope = Scope::open();
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/orders", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer().skip_routes(["/health"]));

    let health = app.clone().oneshot(get_request("/health")).await.unwrap();
    app.oneshot(get_request("/orders")).await.unwrap();

    assert_eq!(health.status(), StatusCode::OK);
    assert!(scope.collector.by_name("GET /health").is_empty());
    scope.collector.expect_span("GET /orders");
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_answers_with_traceparent_only_when_asked_and_never_baggage() {
    let scope = Scope::open();
    let handler = || async { autotel::current_trace_id().unwrap() };
    let routes = Router::new().route("/id", get(handler));
    let caller = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let request = || {
        let mut request = get_request("/id");
        request
            .headers_mut()
            .insert("traceparent", caller.parse().unwrap());
        request
            .headers_mut()
            .insert("baggage", "tenant=acme".parse().unwrap());
        request
    };

    let plain = routes
        .clone()
        .layer(autotel::axum::layer())
        .oneshot(request())
        .await
        .unwrap();
    assert!(!plain.headers().contains_key("traceparent"));

    let app = routes.layer(autotel::axum::layer().trace_response_header());
    let response = app.oneshot(request()).await.unwrap();
    assert!(!response.headers().contains_key("baggage"));
    let header = response.headers()["traceparent"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(header.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
    assert!(
        !header.contains("00f067aa0ba902b7"),
        "must name our span, not the caller's"
    );
    let body = axum::body::to_bytes(response.into_body(), 64)
        .await
        .unwrap();
    assert_eq!(&body[..], b"4bf92f3577b34da6a3ce929d0e0e4736");
    drop(scope);
}

fn from_peer(uri: &str, peer: &str) -> axum::http::Request<axum::body::Body> {
    let mut request = get_request(uri);
    let peer: std::net::SocketAddr = peer.parse().unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    request
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_keeps_query_keys_and_hides_values_unless_told_otherwise() {
    let scope = Scope::open();
    let routes = Router::new().route("/search", get(|| async { StatusCode::OK }));

    let redacted = routes.clone().layer(autotel::axum::layer());
    redacted
        .oneshot(get_request("/search?q=rust&token=s3cret"))
        .await
        .unwrap();
    let span = scope.collector.expect_span("GET /search");
    assert_eq!(
        span.attributes["url.query"],
        json!("q=REDACTED&token=REDACTED")
    );
    drop(scope);

    let scope = Scope::open();
    let raw = routes.layer(autotel::axum::layer().raw_query());
    raw.oneshot(get_request("/search?q=rust")).await.unwrap();
    let span = scope.collector.expect_span("GET /search");
    assert_eq!(span.attributes["url.query"], json!("q=rust"));
}

#[tokio::test(flavor = "current_thread")]
async fn client_address_is_the_peer_and_a_forged_header_cannot_change_it() {
    let scope = Scope::open();
    let app = Router::new()
        .route("/a", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());
    let mut request = from_peer("/a", "203.0.113.9:5000");
    request
        .headers_mut()
        .insert("x-forwarded-for", "6.6.6.6".parse().unwrap());

    app.oneshot(request).await.unwrap();

    let span = scope.collector.expect_span("GET /a");
    assert_eq!(span.attributes["client.address"], json!("203.0.113.9"));
    assert_eq!(
        span.attributes["network.peer.address"],
        json!("203.0.113.9")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn trusted_forwarded_for_names_the_client_and_ignores_junk() {
    let scope = Scope::open();
    let routes = Router::new()
        .route("/a", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer().trust_forwarded_for());

    let mut request = from_peer("/a", "10.0.0.1:5000");
    let chain = "198.51.100.7, 10.0.0.2".parse().unwrap();
    request.headers_mut().insert("x-forwarded-for", chain);
    routes.clone().oneshot(request).await.unwrap();
    let span = scope.collector.expect_span("GET /a");
    assert_eq!(span.attributes["client.address"], json!("198.51.100.7"));
    assert_eq!(span.attributes["network.peer.address"], json!("10.0.0.1"));
    drop(scope);

    let scope = Scope::open();
    let mut request = from_peer("/a", "10.0.0.1:5000");
    request
        .headers_mut()
        .insert("x-forwarded-for", "<script>".parse().unwrap());
    routes.oneshot(request).await.unwrap();
    let span = scope.collector.expect_span("GET /a");
    assert_eq!(span.attributes["client.address"], json!("10.0.0.1"));
}

#[tokio::test(flavor = "current_thread")]
async fn redact_pii_drops_ip_addresses() {
    let _scope = Scope::open();
    let mut config = AutotelConfig::new("pii").capture_spans();
    config.redact_pii = true;
    let _runtime = autotel::init(config).unwrap();
    let app = Router::new()
        .route("/a", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());

    app.oneshot(from_peer("/a", "203.0.113.9:5000"))
        .await
        .unwrap();

    let spans = autotel::completed_spans();
    let span = spans.iter().find(|span| span.name == "GET /a").unwrap();
    assert!(!span.attributes.contains_key("client.address"));
    assert!(!span.attributes.contains_key("network.peer.address"));
}

/// Middleware may log or read the context in `call` itself, before its future is polled.
#[tokio::test(flavor = "current_thread")]
async fn axum_layer_calls_the_inner_service_inside_the_span() {
    use tower::{Layer, service_fn};
    let scope = Scope::open();
    let seen = std::sync::Arc::new(Mutex::new(None));
    let captured = seen.clone();
    let service = service_fn(move |_: axum::extract::Request| {
        *captured.lock().unwrap() = autotel::current_trace_id();
        async {
            Ok::<_, std::convert::Infallible>(axum::response::Response::new(
                axum::body::Body::empty(),
            ))
        }
    });

    autotel::axum::layer()
        .layer(service)
        .oneshot(get_request("/a"))
        .await
        .unwrap();

    let span = scope.collector.expect_span("GET");
    assert_eq!(*seen.lock().unwrap(), Some(span.trace_id));
}

#[tokio::test(flavor = "current_thread")]
async fn query_text_with_no_key_is_a_value_and_stays_hidden() {
    let scope = Scope::open();
    let app = Router::new()
        .route("/verify", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());

    app.oneshot(get_request("/verify?alice@example.com&page=2"))
        .await
        .unwrap();

    let span = scope.collector.expect_span("GET /verify");
    assert_eq!(
        span.attributes["url.query"],
        json!("REDACTED&page=REDACTED")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn url_full_hides_bare_query_values_too() {
    let scope = Scope::open();
    http_client(
        "GET",
        "https://api.example.com/v?alice@example.com",
        |_| async {},
    )
    .await;

    let span = scope.collector.expect_span("GET");
    assert_eq!(
        span.attributes["url.full"],
        json!("https://api.example.com/v?REDACTED")
    );
}

async fn scheme_with(layer: autotel::axum::AutotelLayer, forwarded: &str) -> serde_json::Value {
    let scope = Scope::open();
    let app = Router::new()
        .route("/a", get(|| async { StatusCode::OK }))
        .layer(layer);
    let mut request = get_request("/a");
    let forwarded = forwarded.parse().unwrap();
    request.headers_mut().insert("x-forwarded-proto", forwarded);

    app.oneshot(request).await.unwrap();

    scope.collector.expect_span("GET /a").attributes["url.scheme"].clone()
}

#[tokio::test(flavor = "current_thread")]
async fn forwarded_scheme_needs_a_trusted_proxy_and_a_real_scheme() {
    let trusted = || autotel::axum::layer().trust_forwarded_for();

    assert_eq!(
        scheme_with(autotel::axum::layer(), "https").await,
        json!("http")
    );
    assert_eq!(scheme_with(trusted(), "https").await, json!("https"));
    assert_eq!(scheme_with(trusted(), "gopher").await, json!("http"));
}
