//! `span_bridge()`: `#[tracing::instrument]` spans and `trace()` spans share one trace tree.
//! One test, because `init` and the global tracer provider are process-wide.

use autotel::{AutotelConfig, span_bridge, trace};
use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use std::collections::HashMap;
use tracing::Instrument;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tracing::instrument]
async fn instrumented_handler() {
    // `trace` inside an instrumented fn must be its child.
    trace("autotel.inside.tracing", |_| async {}).await;
}

#[tokio::test(flavor = "current_thread")]
async fn tracing_and_autotel_spans_parent_each_other() {
    let _runtime = autotel::init(AutotelConfig::new("span-bridge")).unwrap();
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    global::set_tracer_provider(provider.clone());
    let _subscriber = tracing_subscriber::registry()
        .with(span_bridge())
        .set_default();

    instrumented_handler().await;

    trace("autotel.outer", |_| async {
        // A `tracing` span opened inside `trace` must be its child.
        async {
            tracing::info!("inside the tracing span");
        }
        .instrument(tracing::info_span!("tracing.inside.autotel"))
        .await;
    })
    .await;

    provider.force_flush().unwrap();
    let spans: HashMap<String, _> = exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .map(|span| (span.name.to_string(), span))
        .collect();

    let handler = &spans["instrumented_handler"];
    let inner = &spans["autotel.inside.tracing"];
    assert_eq!(inner.parent_span_id, handler.span_context.span_id());
    assert_eq!(
        inner.span_context.trace_id(),
        handler.span_context.trace_id()
    );

    let outer = &spans["autotel.outer"];
    let bridged = &spans["tracing.inside.autotel"];
    assert_eq!(bridged.parent_span_id, outer.span_context.span_id());
    assert_eq!(
        bridged.span_context.trace_id(),
        outer.span_context.trace_id()
    );
}
