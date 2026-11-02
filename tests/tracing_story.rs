//! executable-stories showcase for autotel.
//!
//! Exercises autotel's public API and publishes the run as living
//! documentation, with the spans the run actually recorded attached to each
//! scenario.

use autotel::{SpanData, TraceCollector, trace, trace_result};
use executable_stories::{Story, write_results};
use serde_json::json;
use std::sync::{Mutex, MutexGuard};

/// `TraceCollector::spans()` returns every span the test binary has recorded so
/// far, and never drains. Scenarios therefore run one at a time and read only
/// the spans that appeared while they held the lock.
static SERIAL: Mutex<()> = Mutex::new(());

struct Scope {
    _guard: MutexGuard<'static, ()>,
    collector: TraceCollector,
    offset: usize,
}

impl Scope {
    fn open() -> Self {
        let guard = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let collector = TraceCollector::new("autotel-stories").unwrap();
        let offset = collector.spans().len();
        Self {
            _guard: guard,
            collector,
            offset,
        }
    }

    fn spans(&self) -> Vec<SpanData> {
        let mut all = self.collector.spans();
        all.split_off(self.offset)
    }
}

/// Converts recorded spans into the shape the story report renders as a trace
/// waterfall.
fn serialize_spans(spans: &[SpanData]) -> Vec<serde_json::Value> {
    spans
        .iter()
        .map(|span| {
            let mut entry = json!({
                "spanId": span.span_id,
                "name": span.name,
                "startTimeMs": 0.0,
                "durationMs": span.duration.as_secs_f64() * 1000.0,
                "status": if span.error.is_some() { "error" } else { "ok" },
            });
            if let Some(parent) = &span.parent_span_id {
                entry["parentSpanId"] = json!(parent);
            }
            if let Some(error) = &span.error {
                entry["statusMessage"] = json!(error);
            }
            entry
        })
        .collect()
}

async fn charge_card(amount: i64) -> Result<i64, String> {
    trace_result("payment.charge", |context| async move {
        context.set_attribute("payment.amount", json!(amount));
        if amount <= 0 {
            return Err(format!("invalid amount: {amount}"));
        }
        Ok(amount)
    })
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn traced_function_records_a_span() {
    let mut story = Story::new("A traced function records a span named after the operation")
        .with_tags(&["otel", "tracing"]);
    let scope = Scope::open();

    story.given("a payment function wrapped once with autotel trace_result");
    story.code(
        "The whole instrumentation",
        r#"trace_result("payment.charge", |context| async move {
    context.set_attribute("payment.amount", json!(amount));
    Ok(amount)
})"#,
        Some("rust"),
    );

    story.when("the function is called");
    let result = charge_card(2500).await;

    story.then("it returns its ordinary value, unchanged by instrumentation");
    story.kv("captured", json!(result.as_ref().ok().copied()));
    assert_eq!(result, Ok(2500));

    let spans = scope.spans();
    story.and("a span was recorded without a single line of tracing code");
    let cells: Vec<Vec<&str>> = spans
        .iter()
        .map(|span| {
            vec![
                span.name.as_str(),
                if span.error.is_some() { "error" } else { "ok" },
            ]
        })
        .collect();
    let rows: Vec<&[&str]> = cells.iter().map(Vec::as_slice).collect();
    story.table(
        "Spans recorded by this scenario",
        &["name", "status"],
        &rows,
    );
    story.attach_spans(serialize_spans(&spans));

    assert!(spans.iter().any(|span| span.name == "payment.charge"));
    story.pass();
}

#[tokio::test(flavor = "current_thread")]
async fn traced_function_records_failure() {
    let mut story = Story::new("A traced function that fails records the failure on its span")
        .with_tags(&["otel", "tracing"]);
    let scope = Scope::open();

    story.given("the same payment function, with no error handling added");

    story.when("it is called with an amount it rejects");
    let result = charge_card(-1).await;
    assert!(result.is_err());

    let spans = scope.spans();
    story.attach_spans(serialize_spans(&spans));
    let span = spans
        .iter()
        .find(|span| span.name == "payment.charge")
        .expect("payment.charge span");

    story.then("the span is marked as failed");
    story.kv("statusMessage", json!(span.error));
    assert!(span.error.is_some());

    story.but("the caller still sees the error it would have seen without autotel");
    story.note("autotel records the failure on the span and returns the error. It never swallows a failure.");
    story.pass();
}

#[tokio::test(flavor = "current_thread")]
async fn nested_traced_calls_join_one_trace() {
    let mut story =
        Story::new("Nested traced calls join a single trace").with_tags(&["otel", "tracing"]);
    let scope = Scope::open();

    story.given("a checkout function that calls the payment function");

    story.when("checkout runs");
    trace("checkout", |_| async {
        assert_eq!(charge_card(4200).await, Ok(4200));
    })
    .await;

    let spans = scope.spans();
    story.attach_spans(serialize_spans(&spans));
    let outer = spans
        .iter()
        .find(|span| span.name == "checkout")
        .expect("checkout span");
    let inner = spans
        .iter()
        .find(|span| span.name == "payment.charge")
        .expect("payment.charge span");

    story.then("the inner span is a child of the outer one");
    story.mermaid(
        "graph TD\n  C[\"checkout\"] --> P[\"payment.charge\"]",
        Some("Trace shape recorded by this run"),
    );
    assert_eq!(
        inner.parent_span_id.as_deref(),
        Some(outer.span_id.as_str())
    );

    story.and("the caller wrote no context propagation code at all");
    story.note("Parenting comes from the ambient context that trace() carries with the future.");
    story.pass();
}

#[dtor::dtor(unsafe)]
fn write_story_results() {
    write_results();
}
