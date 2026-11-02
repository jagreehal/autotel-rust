//! `AdaptiveSampler::keep_when`: attribute rules keep whole traces, as a Collector policy would.
//! One test, because `init` and the tracer provider are process-wide.

use autotel::{AdaptiveSampler, AutotelConfig, TailSamplingProcessor, trace};
use opentelemetry::{Value, global};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use std::{collections::BTreeSet, sync::Arc};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn security_sampler() -> AdaptiveSampler {
    AdaptiveSampler::errors_only()
        .keep_when("auth.success", |value| matches!(value, Value::Bool(false)))
        .keep_when(
            "db.response.returned_rows",
            |value| matches!(value, Value::I64(rows) if *rows >= 100),
        )
}

#[tokio::test(flavor = "current_thread")]
async fn attribute_rules_keep_whole_traces() {
    let _telemetry =
        autotel::init(AutotelConfig::new("sampling-rules").sampler(security_sampler())).unwrap();
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(TailSamplingProcessor::new(
            SimpleSpanProcessor::new(exporter.clone()),
            Arc::new(security_sampler()),
        ))
        .build();
    global::set_tracer_provider(provider.clone());
    let _subscriber = tracing_subscriber::registry()
        .with(autotel::span_bridge())
        .set_default();

    trace("login.ok", |_| async {
        trace("auth.check", |ctx| async move {
            ctx.set_attribute("auth.success", true);
        })
        .await;
    })
    .await;
    trace("login.failed", |_| async {
        trace("auth.reject", |ctx| async move {
            ctx.set_attribute("auth.success", false);
        })
        .await;
    })
    .await;
    trace("export.small", |ctx| async move {
        ctx.set_attribute("db.response.returned_rows", 3);
    })
    .await;
    // The rule sees `tracing`-bridged spans too.
    trace("export.bulk", |_| async {
        let span = tracing::info_span!("bulk.query", db.response.returned_rows = 5000_i64);
        let _entered = span.enter();
    })
    .await;

    provider.force_flush().unwrap();
    let exported: BTreeSet<String> = exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .map(|span| span.name.into_owned())
        .collect();
    assert_eq!(
        exported,
        BTreeSet::from(
            ["auth.reject", "bulk.query", "export.bulk", "login.failed"].map(String::from)
        )
    );
}
