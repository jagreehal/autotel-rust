//! SQLx transaction + pool metric helpers. Requires `--features sqlx`.

#![cfg(feature = "sqlx")]

use autotel::{TraceCollector, acquire, register_pool_metrics, trace, with_transaction};
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData},
};
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

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
            collector: TraceCollector::new("sqlx-helpers").unwrap(),
        }
    }
}

async fn memory_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[tokio::test(flavor = "current_thread")]
async fn with_transaction_commits_and_records_outcome() {
    let scope = Scope::open();
    let pool = memory_pool().await;

    let id = with_transaction(&pool, "insert_item", "demo", |tx| {
        Box::pin(async move {
            let result = sqlx::query("INSERT INTO items (name) VALUES (?)")
                .bind("widget")
                .execute(&mut **tx)
                .await?;
            Ok::<_, sqlx::Error>(result.last_insert_rowid())
        })
    })
    .await
    .unwrap();

    assert!(id > 0);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let span = scope.collector.expect_span("transaction insert_item");
    assert_eq!(span.attributes["otel.span.kind"], json!("client"));
    assert_eq!(span.attributes["db.system.name"], json!("sqlite"));
    assert_eq!(span.attributes["db.operation.name"], json!("transaction"));
    assert_eq!(span.attributes["db.transaction.name"], json!("insert_item"));
    assert_eq!(span.attributes["db.transaction.outcome"], json!("commit"));
}

#[tokio::test(flavor = "current_thread")]
async fn with_transaction_rolls_back_on_error() {
    let scope = Scope::open();
    let pool = memory_pool().await;

    let err = with_transaction(&pool, "failing", "demo", |tx| {
        Box::pin(async move {
            sqlx::query("INSERT INTO items (name) VALUES (?)")
                .bind("ghost")
                .execute(&mut **tx)
                .await?;
            Err::<(), _>(sqlx::Error::Protocol("boom".into()))
        })
    })
    .await
    .expect_err("should roll back");

    assert!(err.to_string().contains("boom"));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let span = scope.collector.expect_span("transaction failing");
    assert_eq!(span.attributes["db.transaction.outcome"], json!("rollback"));
    assert!(span.error.is_some());
}

/// `name{pool.name="main"}` → value for every `i64` sum, and wait-time sample counts.
fn pool_readings(exporter: &InMemoryMetricExporter) -> (BTreeMap<String, i64>, u64) {
    let mut sums = BTreeMap::new();
    let mut waits = 0;
    for resource in exporter.get_finished_metrics().unwrap() {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                match metric.data() {
                    AggregatedMetrics::I64(MetricData::Sum(sum)) => {
                        for point in sum.data_points() {
                            let state = point
                                .attributes()
                                .find(|kv| kv.key.as_str() == "db.client.connection.state")
                                .map(|kv| format!("[{}]", kv.value))
                                .unwrap_or_default();
                            sums.insert(format!("{}{state}", metric.name()), point.value());
                        }
                    }
                    AggregatedMetrics::F64(MetricData::Histogram(histogram))
                        if metric.name() == "db.client.connection.wait_time" =>
                    {
                        waits = histogram
                            .data_points()
                            .filter(|point| {
                                point.attributes().any(|kv| {
                                    kv.key.as_str() == "db.client.connection.pool.name"
                                        && kv.value.as_str() == "main"
                                })
                            })
                            .map(|point| point.count())
                            .sum();
                    }
                    _ => {}
                }
            }
        }
    }
    (sums, waits)
}

#[tokio::test(flavor = "current_thread")]
async fn acquire_and_pool_metrics_expose_pool_starvation() {
    let scope = Scope::open();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    opentelemetry::global::set_meter_provider(provider.clone());

    // One connection, so a second caller has to wait for it.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(200))
        .connect("sqlite::memory:")
        .await
        .unwrap();
    register_pool_metrics(pool.clone(), "main");

    let held = trace("hold", |_| acquire(&pool, "main")).await.unwrap();
    let waiter = tokio::spawn({
        let pool = pool.clone();
        async move { acquire(&pool, "main").await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    provider.force_flush().unwrap();
    let (starved, _) = pool_readings(&exporter);
    assert_eq!(starved["db.client.connection.max"], 1);
    assert_eq!(starved["db.client.connection.count[used]"], 1);
    assert_eq!(starved["db.client.connection.count[idle]"], 0);
    assert_eq!(starved["db.client.connection.pending_requests"], 1);

    // The waiter times out: the failed acquire must still leave the pending count.
    assert!(waiter.await.unwrap().is_err());
    drop(held);
    exporter.reset();
    provider.force_flush().unwrap();
    let (recovered, waits) = pool_readings(&exporter);
    assert_eq!(recovered["db.client.connection.pending_requests"], 0);
    assert_eq!(waits, 2, "both acquires, failed one included, are timed");

    let span = scope.collector.expect_span("hold");
    assert!(span.attributes["autotel.db.pool.wait_ms"].is_f64());
}
