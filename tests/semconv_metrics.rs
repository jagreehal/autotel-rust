//! Metrics autotel records itself: names, labels, and bucket boundaries.
//! One test, because `init` and the global meter provider are process-wide.

use autotel::{
    AutotelConfig, GenAiMetrics, GenAiOptions, GenAiUsage, database, http_client, http_server,
    register_runtime_metrics,
};
use opentelemetry::global;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData},
};
use std::collections::BTreeMap;

type Labels = BTreeMap<String, String>;

/// Every exported data point: metric name → (labels, histogram bounds if any).
fn collect(exporter: &InMemoryMetricExporter) -> Vec<(String, Labels, Vec<f64>)> {
    let mut points = Vec::new();
    for resource in exporter.get_finished_metrics().unwrap() {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                let name = metric.name().to_owned();
                let mut push = |attrs: Vec<&opentelemetry::KeyValue>, bounds: Vec<f64>| {
                    let labels = attrs
                        .into_iter()
                        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                        .collect();
                    points.push((name.clone(), labels, bounds));
                };
                match metric.data() {
                    AggregatedMetrics::F64(MetricData::Histogram(h)) => {
                        for dp in h.data_points() {
                            push(dp.attributes().collect(), dp.bounds().collect());
                        }
                    }
                    AggregatedMetrics::U64(MetricData::Gauge(g)) => {
                        for dp in g.data_points() {
                            push(dp.attributes().collect(), Vec::new());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    points
}

fn find<'a>(
    points: &'a [(String, Labels, Vec<f64>)],
    name: &str,
    label: (&str, &str),
) -> &'a (String, Labels, Vec<f64>) {
    points
        .iter()
        .find(|(n, labels, _)| {
            n == name && labels.get(label.0).map(String::as_str) == Some(label.1)
        })
        .unwrap_or_else(|| panic!("no {name} with {label:?} in {points:#?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semconv_metrics_have_names_labels_and_buckets() {
    let _runtime = autotel::init(AutotelConfig::new("semconv-metrics")).unwrap();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    global::set_meter_provider(provider.clone());

    http_server("GET", "/orders/{id}", |ctx| async move {
        ctx.set_attribute("http.response.status_code", 200_u16);
    })
    .await;
    http_server("POST", "/checkout", |ctx| async move {
        ctx.set_attribute("http.response.status_code", 503_u16);
    })
    .await;
    http_client(
        "GET",
        "https://inventory.internal/stock",
        |ctx| async move {
            ctx.set_attribute("http.response.status_code", 404_u16);
        },
    )
    .await;
    database("postgresql", "SELECT", "shop", "orders", |_| async {}).await;
    // Failure is read from the span: an error recorded on it labels the metric.
    database("postgresql", "DELETE", "shop", "carts", |ctx| async move {
        ctx.record_error(&"lock timeout");
    })
    .await;
    register_runtime_metrics().unwrap();
    let genai = GenAiMetrics::new();
    genai.record_success(
        &GenAiOptions::chat("openai", "gpt-4o"),
        &GenAiUsage {
            input_tokens: 120,
            output_tokens: 30,
            ..GenAiUsage::default()
        },
        1.5,
    );

    provider.force_flush().unwrap();
    let points = collect(&exporter);
    let seconds = vec![
        0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
    ];

    let ok = find(
        &points,
        "http.server.request.duration",
        ("http.route", "/orders/{id}"),
    );
    assert_eq!(ok.1["http.response.status_code"], "200");
    assert!(!ok.1.contains_key("error.type"));
    assert_eq!(ok.2, seconds);

    let failed = find(
        &points,
        "http.server.request.duration",
        ("http.route", "/checkout"),
    );
    assert_eq!(failed.1["error.type"], "503");

    let client = find(
        &points,
        "http.client.request.duration",
        ("server.address", "inventory.internal"),
    );
    assert_eq!(client.1["server.port"], "443");
    assert_eq!(client.1["error.type"], "404");
    assert_eq!(client.2, seconds);

    let select = find(
        &points,
        "db.client.operation.duration",
        ("db.operation.name", "SELECT"),
    );
    assert_eq!(select.1["db.system.name"], "postgresql");
    assert_eq!(select.1["db.namespace"], "shop");
    assert!(!select.1.contains_key("error.type"));
    assert_eq!(select.2, seconds);
    let delete = find(
        &points,
        "db.client.operation.duration",
        ("db.operation.name", "DELETE"),
    );
    assert_eq!(delete.1["error.type"], "_OTHER");

    for gauge in [
        "tokio.runtime.workers",
        "tokio.runtime.alive_tasks",
        "tokio.runtime.global_queue_depth",
    ] {
        assert!(points.iter().any(|(n, _, _)| n == gauge), "missing {gauge}");
    }

    let duration = find(
        &points,
        "gen_ai.client.operation.duration",
        ("gen_ai.request.model", "gpt-4o"),
    );
    assert_eq!(duration.2.first(), Some(&0.01));
    let tokens = find(
        &points,
        "gen_ai.client.token.usage",
        ("gen_ai.token.type", "input"),
    );
    assert_eq!(tokens.2.first(), Some(&1.0));
}

#[test]
fn register_runtime_metrics_needs_a_runtime() {
    assert!(register_runtime_metrics().is_err());
}
