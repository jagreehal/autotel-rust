//! `rpc.server.call.duration`. One test, because `init` and the global meter provider
//! are process-wide.

#![cfg(feature = "tonic")]

use autotel::AutotelConfig;
use bytes::Bytes;
use http_body::{Body, Frame};
use http_body_util::{BodyExt, StreamBody};
use opentelemetry::global;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData},
};
use std::{collections::BTreeMap, convert::Infallible};
use tower::{Layer, ServiceExt, service_fn};

type Labels = BTreeMap<String, String>;

/// A finished call, its status in trailers.
fn finished(status: &'static str) -> impl Body<Data = Bytes, Error = Infallible> {
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", status.parse().unwrap());
    StreamBody::new(tokio_stream::iter([Ok::<_, Infallible>(Frame::trailers(
        trailers,
    ))]))
}

async fn call(path: &str, status: &'static str) {
    let service =
        service_fn(
            move |_| async move { Ok::<_, Infallible>(http::Response::new(finished(status))) },
        );
    let request = http::Request::builder()
        .uri(path)
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    let response = autotel::tonic::layer()
        .layer(service)
        .oneshot(request)
        .await
        .unwrap();
    response.into_body().collect().await.unwrap();
}

/// A router or middleware answering with plain HTTP, before any gRPC handler runs.
async fn rejected(path: &str) {
    let service = service_fn(|_| async {
        let response = http::Response::builder().status(503);
        Ok::<_, Infallible>(
            response
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap(),
        )
    });
    let request = http::Request::builder()
        .uri(path)
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    autotel::tonic::layer()
        .layer(service)
        .oneshot(request)
        .await
        .unwrap();
}

fn points(exporter: &InMemoryMetricExporter) -> Vec<(Labels, u64, Vec<f64>)> {
    let mut points = Vec::new();
    for resource in exporter.get_finished_metrics().unwrap() {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
                    continue;
                };
                assert_eq!(metric.name(), "rpc.server.call.duration");
                assert_eq!(metric.unit(), "s");
                for point in histogram.data_points() {
                    let labels = point
                        .attributes()
                        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                        .collect();
                    points.push((labels, point.count(), point.bounds().collect()));
                }
            }
        }
    }
    points
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_are_measured_by_method_and_status_and_unknown_methods_share_a_series() {
    let _runtime = autotel::init(AutotelConfig::new("rpc-metrics")).unwrap();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    global::set_meter_provider(provider.clone());

    call("/orders.Orders/Get", "0").await;
    call("/orders.Orders/Get", "0").await;
    call("/orders.Orders/Get", "13").await;
    call("/orders.Orders/Get", "5").await;
    call("/scan.Me/One", "12").await;
    call("/scan.Me/Two", "12").await;
    rejected("/scan.Me/Three").await;
    provider.force_flush().unwrap();

    let points = points(&exporter);
    let find = |method: &str, status: &str| {
        points
            .iter()
            .find(|(labels, _, _)| {
                labels["rpc.method"] == method && labels["rpc.status_code"] == status
            })
            .unwrap_or_else(|| panic!("no {method} {status} in {points:#?}"))
    };

    let (ok, count, bounds) = find("orders.Orders/Get", "OK");
    assert_eq!(*count, 2);
    assert_eq!(ok["rpc.system.name"], "grpc");
    assert!(!ok.contains_key("error.type"));
    assert_eq!(bounds.len(), 41);
    assert_eq!(bounds[1], 0.00001);

    let (internal, _, _) = find("orders.Orders/Get", "INTERNAL");
    assert_eq!(internal["error.type"], "INTERNAL");

    let (not_found, _, _) = find("orders.Orders/Get", "NOT_FOUND");
    assert!(!not_found.contains_key("error.type"));

    let (_, count, _) = find("_OTHER", "UNIMPLEMENTED");
    assert_eq!(*count, 2);
    let (rejection, _, _) = find("_OTHER", "UNAVAILABLE");
    assert_eq!(rejection["error.type"], "UNAVAILABLE");
    assert_eq!(
        points.len(),
        5,
        "client paths stay out of series: {points:#?}"
    );
}
