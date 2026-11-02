//! Default tracing subscriber install + Axum RED metric recording.

#![cfg(feature = "axum")]

use autotel::{AutotelConfig, TraceCollector, init};
use axum::{Router, http::StatusCode, routing::get};
use std::sync::{Mutex, MutexGuard};
use tower::util::ServiceExt;

static SERIAL: Mutex<()> = Mutex::new(());

struct Scope {
    _guard: MutexGuard<'static, ()>,
}

impl Scope {
    fn open() -> Self {
        Self {
            _guard: SERIAL
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        }
    }
}

#[test]
fn tracing_subscriber_install_options() {
    let _scope = Scope::open();

    let skipped = init(AutotelConfig::new("no-sub")).unwrap();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .expect("subscriber should install when autotel skipped it");
    drop(skipped);

    // Opt-in path uses try_init: must not panic when a subscriber is already set.
    let _installed = init(AutotelConfig::new("with-sub").with_tracing_subscriber()).unwrap();
    tracing::info!(target: "autotel_subscriber_test", "default subscriber path");
}

#[tokio::test(flavor = "current_thread")]
async fn axum_layer_records_red_metrics_without_panic() {
    let _scope = Scope::open();
    let _collector = TraceCollector::new("red").unwrap();
    let app = Router::new()
        .route("/ok", get(|| async { StatusCode::OK }))
        .route("/fail", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .layer(autotel::axum::layer());

    let ok = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/ok")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let fail = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/fail")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fail.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test(flavor = "current_thread")]
async fn red_metrics_follow_the_latest_init() {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    let _scope = Scope::open();
    let app = Router::new()
        .route("/ok", get(|| async { StatusCode::OK }))
        .layer(autotel::axum::layer());
    let hit = || {
        app.clone().oneshot(
            axum::http::Request::builder()
                .uri("/ok")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
    };

    // A request before `init` caches instruments on whatever provider is global.
    hit().await.unwrap();
    // `init` must drop that cache; swap in an in-memory provider right after it
    // so the next request's instruments land somewhere we can read.
    let _runtime = init(AutotelConfig::new("red-reinit")).unwrap();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    opentelemetry::global::set_meter_provider(provider.clone());
    hit().await.unwrap();
    provider.force_flush().unwrap();
    let names: Vec<String> = exporter
        .get_finished_metrics()
        .unwrap()
        .iter()
        .flat_map(|rm| {
            rm.scope_metrics()
                .flat_map(|sm| sm.metrics().map(|m| m.name().to_owned()))
        })
        .collect();
    assert!(
        names.iter().any(|n| n == "http.server.request.duration"),
        "{names:?}"
    );
}
