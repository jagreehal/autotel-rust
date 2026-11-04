//! The span as an exporter receives it. Its own binary, because the tracer provider is
//! process-wide.

#![cfg(feature = "tonic")]

use autotel::AutotelConfig;
use bytes::Bytes;
use http_body_util::Empty;
use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use std::convert::Infallible;
use tower::{Layer, ServiceExt, service_fn};

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_method_exports_one_rpc_method_attribute() {
    let _telemetry = autotel::init(AutotelConfig::new("grpc-export")).unwrap();
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    global::set_tracer_provider(provider.clone());
    let service = service_fn(|_| async {
        let response = http::Response::builder().header("grpc-status", "12");
        Ok::<_, Infallible>(response.body(Empty::<Bytes>::new()).unwrap())
    });
    let request = http::Request::builder()
        .uri("/scan.Me/Random123")
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();

    autotel::tonic::layer()
        .layer(service)
        .oneshot(request)
        .await
        .unwrap();

    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    let methods: Vec<String> = spans[0]
        .attributes
        .iter()
        .filter(|attribute| attribute.key.as_str() == "rpc.method")
        .map(|attribute| attribute.value.to_string())
        .collect();
    assert_eq!(methods, ["_OTHER"]);
}
