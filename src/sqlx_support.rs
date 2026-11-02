//! SQLx helpers: instrumented transactions and connection-pool metrics.
//!
//! Enable with the `sqlx` feature.

use crate::database;
use opentelemetry::{KeyValue, global};
use sqlx::{Database, Pool, Transaction, pool::PoolConnection};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
    time::Instant,
};

/// Run `work` inside a database transaction wrapped in a CLIENT DB span.
///
/// On success the transaction commits and the span records
/// `db.transaction.outcome=commit`. On error it rolls back and records
/// `db.transaction.outcome=rollback`.
///
/// ```no_run
/// # async fn demo(pool: sqlx::SqlitePool) -> Result<(), sqlx::Error> {
/// let order_id: i64 = autotel::with_transaction(&pool, "checkout", "shop", |tx| {
///     Box::pin(async move {
///         let row: (i64,) = sqlx::query_as("INSERT INTO orders (total) VALUES (42) RETURNING id")
///             .fetch_one(&mut **tx)
///             .await?;
///         Ok::<_, sqlx::Error>(row.0)
///     })
/// })
/// .await?;
/// # let _ = order_id;
/// # Ok(())
/// # }
/// ```
pub async fn with_transaction<DB, T, E, F>(
    pool: &Pool<DB>,
    name: &str,
    namespace: &str,
    work: F,
) -> Result<T, E>
where
    DB: Database,
    E: From<sqlx::Error> + std::fmt::Display,
    F: for<'c> FnOnce(
        &'c mut Transaction<'_, DB>,
    ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>,
{
    let system = db_system_name::<DB>();
    let name = name.to_owned();
    let namespace = namespace.to_owned();
    let span_name = name.clone();
    database(
        system,
        "transaction",
        &namespace,
        &span_name,
        |context| async move {
            context.set_attribute("db.operation.name", "transaction");
            context.set_attribute("db.transaction.name", name.as_str());
            let mut tx = pool.begin().await.map_err(E::from)?;
            match work(&mut tx).await {
                Ok(value) => {
                    if let Err(error) = tx.commit().await {
                        let error = E::from(error);
                        context.set_attribute("db.transaction.outcome", "commit_failed");
                        context.record_error(&error);
                        return Err(error);
                    }
                    context.set_attribute("db.transaction.outcome", "commit");
                    Ok(value)
                }
                Err(error) => {
                    if let Err(rollback) = tx.rollback().await {
                        context.add_event(
                            "db.transaction.rollback_error",
                            crate::Attributes::from([(
                                "exception.message".into(),
                                serde_json::Value::String(rollback.to_string()),
                            )]),
                        );
                    }
                    context.set_attribute("db.transaction.outcome", "rollback");
                    context.record_error(&error);
                    Err(error)
                }
            }
        },
    )
    .await
}

fn db_system_name<DB: Database>() -> &'static str {
    let name = DB::NAME;
    if name.eq_ignore_ascii_case("postgresql") || name.eq_ignore_ascii_case("postgres") {
        "postgresql"
    } else if name.eq_ignore_ascii_case("sqlite") {
        "sqlite"
    } else if name.eq_ignore_ascii_case("mysql") {
        "mysql"
    } else {
        "other_sql"
    }
}

fn pending_counter(pool_name: &str) -> Arc<AtomicI64> {
    static PENDING: OnceLock<Mutex<HashMap<String, Arc<AtomicI64>>>> = OnceLock::new();
    let mut pools = PENDING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    Arc::clone(pools.entry(pool_name.to_owned()).or_default())
}

/// Decrements the pending count on every exit path, `?` and cancellation included.
struct Pending(Arc<AtomicI64>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Acquire a pooled connection, timing the wait separately from the query.
///
/// Records `db.client.connection.wait_time` (seconds, failed acquires included:
/// they are pool pressure too), counts the caller in
/// `db.client.connection.pending_requests` while it waits, and sets
/// `autotel.db.pool.wait_ms` on the active span so a slow span says whether it
/// waited for the pool or for the database.
pub async fn acquire<DB: Database>(
    pool: &Pool<DB>,
    pool_name: &str,
) -> Result<PoolConnection<DB>, sqlx::Error> {
    let counter = pending_counter(pool_name);
    counter.fetch_add(1, Ordering::Relaxed);
    let pending = Pending(counter);
    let started = Instant::now();
    let result = pool.acquire().await;
    drop(pending);
    let waited = started.elapsed();
    let pool_attr = [KeyValue::new(
        "db.client.connection.pool.name",
        pool_name.to_owned(),
    )];
    crate::instruments::with(|instruments| {
        instruments
            .db_connection_wait_time
            .record(waited.as_secs_f64(), &pool_attr);
    });
    if let Some(context) = crate::current_context() {
        context.set_attribute("autotel.db.pool.wait_ms", waited.as_secs_f64() * 1000.0);
    }
    result
}

/// Register the semantic-convention pool instruments for a SQLx pool.
///
/// Each is tagged `db.client.connection.pool.name=pool_name` so several pools in
/// one process stay distinct:
/// - `db.client.connection.count` with `db.client.connection.state=used|idle`
/// - `db.client.connection.max`
/// - `db.client.connection.pending_requests` (callers waiting in [`acquire`])
/// - `db.client.connection.utilization` (used / max, 0.0–1.0)
///
/// Call after [`crate::init`]; instruments bind to the meter provider current at call time.
pub fn register_pool_metrics<DB: Database>(pool: Pool<DB>, pool_name: &str) {
    let meter = global::meter(crate::INSTRUMENTATION_NAME);
    let pool_attr = KeyValue::new("db.client.connection.pool.name", pool_name.to_owned());
    let used_attrs = [
        pool_attr.clone(),
        KeyValue::new("db.client.connection.state", "used"),
    ];
    let idle_attrs = [
        pool_attr.clone(),
        KeyValue::new("db.client.connection.state", "idle"),
    ];

    let counted = pool.clone();
    meter
        .i64_observable_up_down_counter("db.client.connection.count")
        .with_unit("{connection}")
        .with_description("Connections in the pool by state")
        .with_callback(move |observer| {
            let idle = counted.num_idle() as i64;
            let used = i64::from(counted.size()) - idle;
            observer.observe(used.max(0), &used_attrs);
            observer.observe(idle, &idle_attrs);
        })
        .build();

    let maxed = pool.clone();
    let max_attrs = [pool_attr.clone()];
    meter
        .i64_observable_up_down_counter("db.client.connection.max")
        .with_unit("{connection}")
        .with_description("Maximum connections the pool allows")
        .with_callback(move |observer| {
            observer.observe(i64::from(maxed.options().get_max_connections()), &max_attrs);
        })
        .build();

    let pending = pending_counter(pool_name);
    let pending_attrs = [pool_attr.clone()];
    meter
        .i64_observable_up_down_counter("db.client.connection.pending_requests")
        .with_unit("{request}")
        .with_description("Callers waiting for a connection in autotel::acquire")
        .with_callback(move |observer| {
            observer.observe(pending.load(Ordering::Relaxed), &pending_attrs);
        })
        .build();

    let util = pool;
    meter
        .f64_observable_gauge("db.client.connection.utilization")
        .with_description("Fraction of the pool currently in use")
        .with_unit("1")
        .with_callback(move |observer| {
            let idle = util.num_idle() as u32;
            let used = util.size().saturating_sub(idle);
            let max = util.options().get_max_connections();
            let value = if max == 0 {
                0.0
            } else {
                f64::from(used) / f64::from(max)
            };
            observer.observe(value, std::slice::from_ref(&pool_attr));
        })
        .build();
}
