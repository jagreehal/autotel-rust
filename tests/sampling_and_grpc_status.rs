#![cfg(feature = "console")]

#[test]
fn console_exporter_honours_tail_sampling() {
    const TEST: &str = "console_exporter_honours_tail_sampling";
    if std::env::var_os("AUTOTEL_CHILD").is_some() {
        let telemetry = autotel::init(
            autotel::AutotelConfig::new("review").sampler(autotel::AdaptiveSampler::errors_only()),
        )
        .unwrap();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                autotel::trace("review.fast.success", |_| async {}).await;
                autotel::trace("review.failure", |context| async move {
                    context.record_error(&"failed");
                })
                .await;
            });
        drop(telemetry);
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env("AUTOTEL_CHILD", "1")
        .env("OTEL_TRACES_EXPORTER", "console")
        .env("OTEL_METRICS_EXPORTER", "none")
        .env("OTEL_LOGS_EXPORTER", "none")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("review.failure"), "{stdout}");
    assert!(!stdout.contains("review.fast.success"), "{stdout}");
}

#[cfg(feature = "tonic")]
#[tokio::test(flavor = "current_thread")]
async fn grpc_client_preserves_service_error_status() {
    use bytes::Bytes;
    use http_body_util::Empty;
    use tower::ServiceExt;

    let collector = autotel::TraceCollector::new("review-grpc").unwrap();
    let service = tower::service_fn(|_: http::Request<Empty<Bytes>>| async {
        Err::<http::Response<Empty<Bytes>>, _>(tonic::Status::deadline_exceeded("deadline"))
    });
    let error = autotel::tonic::client(service)
        .oneshot(
            http::Request::builder()
                .uri("/review.Service/Call")
                .body(Empty::new())
                .unwrap(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), tonic::Code::DeadlineExceeded);
    let span = collector.expect_span("review.Service/Call");
    assert_eq!(
        span.attributes["rpc.status_code"],
        serde_json::json!("DEADLINE_EXCEEDED")
    );
}

#[cfg(feature = "tonic")]
#[tokio::test(flavor = "current_thread")]
async fn grpc_client_timeout_matches_the_status_seen_by_the_caller() {
    use std::time::Duration;
    use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

    let collector = autotel::TraceCollector::new("review-timeout").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(|| async {
        std::future::pending::<()>().await;
        ""
    });
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let channel = tonic::transport::Channel::from_shared(format!("http://{address}"))
        .unwrap()
        .timeout(Duration::from_millis(25))
        .connect()
        .await
        .unwrap();
    let error = HealthClient::new(autotel::tonic::client(channel))
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap_err();
    server.abort();
    assert_eq!(error.code(), tonic::Code::Cancelled);
    let span = collector.expect_span("grpc.health.v1.Health/Check");
    assert_eq!(
        span.attributes["rpc.status_code"],
        serde_json::json!("CANCELLED")
    );
}

#[cfg(feature = "tonic")]
#[tokio::test(flavor = "current_thread")]
async fn grpc_client_body_error_matches_the_status_seen_by_the_caller() {
    use bytes::Bytes;
    use http_body::Frame;
    use http_body_util::StreamBody;
    use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

    let collector = autotel::TraceCollector::new("review-body-error").unwrap();
    let service = tower::service_fn(|_: http::Request<tonic::body::Body>| async {
        let frames = tokio_stream::iter([Err::<Frame<Bytes>, _>(tonic::Status::cancelled(
            "stream cancelled",
        ))]);
        Ok::<_, tonic::Status>(
            http::Response::builder()
                .header("content-type", "application/grpc")
                .body(StreamBody::new(frames))
                .unwrap(),
        )
    });
    let error = HealthClient::new(autotel::tonic::client(service))
        .check(HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::Cancelled);
    let span = collector.expect_span("grpc.health.v1.Health/Check");
    assert_eq!(
        span.attributes["rpc.status_code"],
        serde_json::json!("CANCELLED")
    );
}
