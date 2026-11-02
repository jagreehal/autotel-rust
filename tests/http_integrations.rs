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
