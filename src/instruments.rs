//! Semantic-convention metrics that autotel records on its own: HTTP server and
//! client duration, database operation duration, pool wait time, and Tokio
//! runtime gauges.

use crate::{INSTRUMENTATION_NAME, SpanData};
use opentelemetry::{KeyValue, global, metrics::Histogram};
use serde_json::Value;
use std::{sync::RwLock, time::Duration};

/// Semconv-advised boundaries for `http.*.request.duration` and `db.client.*` (seconds).
/// The SDK default buckets are tuned for milliseconds and would put every request in one bucket.
pub(crate) const DURATION_BOUNDARIES: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

pub(crate) struct Instruments {
    pub http_server_duration: Histogram<f64>,
    pub http_client_duration: Histogram<f64>,
    pub db_operation_duration: Histogram<f64>,
    #[cfg_attr(not(feature = "sqlx"), allow(dead_code))]
    pub db_connection_wait_time: Histogram<f64>,
}

fn seconds_histogram(name: &'static str, description: &'static str) -> Histogram<f64> {
    global::meter(INSTRUMENTATION_NAME)
        .f64_histogram(name)
        .with_unit("s")
        .with_description(description)
        .with_boundaries(DURATION_BOUNDARIES.to_vec())
        .build()
}

impl Instruments {
    fn new() -> Self {
        Self {
            http_server_duration: seconds_histogram(
                "http.server.request.duration",
                "Duration of inbound HTTP requests",
            ),
            http_client_duration: seconds_histogram(
                "http.client.request.duration",
                "Duration of outbound HTTP requests",
            ),
            db_operation_duration: seconds_histogram(
                "db.client.operation.duration",
                "Duration of database client operations",
            ),
            db_connection_wait_time: seconds_histogram(
                "db.client.connection.wait_time",
                "Time spent waiting for a pooled connection",
            ),
        }
    }
}

static INSTRUMENTS: RwLock<Option<Instruments>> = RwLock::new(None);

/// Run `f` with instruments bound to the meter provider that was global when they
/// were first used. Borrows under a read lock: no refcount traffic on the hot path.
pub(crate) fn with<R>(f: impl FnOnce(&Instruments) -> R) -> R {
    if let Some(cached) = INSTRUMENTS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        return f(cached);
    }
    let mut slot = INSTRUMENTS.write().unwrap_or_else(|e| e.into_inner());
    f(slot.get_or_insert_with(Instruments::new))
}

/// Drop cached instruments so the next recording builds them on the current meter provider.
pub(crate) fn reset() {
    *INSTRUMENTS.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// `error.type` for a finished span: an explicit attribute wins, then the status code,
/// then `_OTHER` for any other recorded failure.
pub(crate) fn error_type(
    span: &SpanData,
    status: Option<u16>,
    failed_status: u16,
) -> Option<String> {
    if let Some(Value::String(explicit)) = span.attributes.get("error.type") {
        return Some(explicit.clone());
    }
    match status {
        Some(code) if code >= failed_status => Some(code.to_string()),
        _ => span.error.as_ref().map(|_| "_OTHER".to_owned()),
    }
}

pub(crate) fn status_code(span: &SpanData) -> Option<u16> {
    span.attributes
        .get("http.response.status_code")
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok())
}

pub(crate) fn record_http_server(
    method: &str,
    route: Option<&str>,
    span: &SpanData,
    elapsed: Duration,
) {
    let status = status_code(span);
    let mut attrs = vec![KeyValue::new("http.request.method", method.to_owned())];
    if let Some(route) = route {
        attrs.push(KeyValue::new("http.route", route.to_owned()));
    }
    if let Some(status) = status {
        attrs.push(KeyValue::new(
            "http.response.status_code",
            i64::from(status),
        ));
    }
    // Servers fail on 5xx only: a 404 is the client's mistake, not ours.
    if let Some(error) = error_type(span, status, 500) {
        attrs.push(KeyValue::new("error.type", error));
    }
    with(|i| i.http_server_duration.record(elapsed.as_secs_f64(), &attrs));
}

pub(crate) fn record_http_client(method: &str, url: &str, span: &SpanData, elapsed: Duration) {
    let status = status_code(span);
    let mut attrs = vec![KeyValue::new("http.request.method", method.to_owned())];
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            attrs.push(KeyValue::new("server.address", host.to_owned()));
        }
        if let Some(port) = parsed.port_or_known_default() {
            attrs.push(KeyValue::new("server.port", i64::from(port)));
        }
    }
    if let Some(status) = status {
        attrs.push(KeyValue::new(
            "http.response.status_code",
            i64::from(status),
        ));
    }
    if let Some(error) = error_type(span, status, 400) {
        attrs.push(KeyValue::new("error.type", error));
    }
    with(|i| i.http_client_duration.record(elapsed.as_secs_f64(), &attrs));
}

pub(crate) fn record_db_operation(
    attrs: &[(&'static str, &str)],
    span: &SpanData,
    elapsed: Duration,
) {
    let mut attrs: Vec<KeyValue> = attrs
        .iter()
        .map(|(key, value)| KeyValue::new(*key, (*value).to_owned()))
        .collect();
    if let Some(error) = error_type(span, None, u16::MAX) {
        attrs.push(KeyValue::new("error.type", error));
    }
    with(|i| {
        i.db_operation_duration
            .record(elapsed.as_secs_f64(), &attrs)
    });
}

type RuntimeReading = fn(&tokio::runtime::RuntimeMetrics) -> usize;

/// Register observable gauges for the current Tokio runtime:
/// `tokio.runtime.workers`, `tokio.runtime.alive_tasks`, `tokio.runtime.global_queue_depth`.
///
/// A rising queue depth with flat CPU means tasks are waiting for a worker: the
/// runtime is saturated, often by blocking work on async threads. Call once, inside
/// the runtime, after [`crate::init`].
pub fn register_runtime_metrics() -> anyhow::Result<()> {
    let handle = tokio::runtime::Handle::try_current().map_err(|error| {
        anyhow::anyhow!("register_runtime_metrics needs a running Tokio runtime ({error})")
    })?;
    let meter = global::meter(INSTRUMENTATION_NAME);
    let gauges: [(&str, &str, RuntimeReading); 3] = [
        (
            "tokio.runtime.workers",
            "Worker threads in the runtime",
            |m| m.num_workers(),
        ),
        (
            "tokio.runtime.alive_tasks",
            "Tasks spawned and not yet finished",
            |m| m.num_alive_tasks(),
        ),
        (
            "tokio.runtime.global_queue_depth",
            "Tasks waiting in the global queue for a worker",
            |m| m.global_queue_depth(),
        ),
    ];
    for (name, description, read) in gauges {
        let handle = handle.clone();
        meter
            .u64_observable_gauge(name)
            .with_description(description)
            .with_callback(move |observer| {
                observer.observe(read(&handle.metrics()) as u64, &[]);
            })
            .build();
    }
    Ok(())
}
