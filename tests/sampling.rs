//! Tail sampling end to end: `trace()` verdicts through `TailSamplingProcessor`.
//! One test, because `init` and the tracer provider are process-wide.

use autotel::{
    AUTOTEL_DEBUG_BAGGAGE_KEY, AdaptiveSampler, AutotelConfig, TailSamplingProcessor, force_keep,
    trace, trace_result,
};
use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use std::{collections::BTreeSet, sync::Arc};

#[tokio::test]
async fn whole_traces_are_kept_or_dropped() {
    // No baseline: only failures, slow traces and explicit keeps survive.
    let _telemetry =
        autotel::init(AutotelConfig::new("sampling-test").sampler(AdaptiveSampler::errors_only()))
            .expect("init");
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(TailSamplingProcessor::new(
            SimpleSpanProcessor::new(exporter.clone()),
            Arc::new(AdaptiveSampler::errors_only()),
        ))
        .build();
    global::set_tracer_provider(provider.clone());

    trace("dull.root", |_| async {
        trace("dull.child", |_| async {}).await;
    })
    .await;

    // The root succeeds; the child's error alone keeps the whole trace.
    trace("failing.root", |_| async {
        let child = trace_result("failing.child", |_| async { Err::<(), _>("refused") }).await;
        assert!(child.is_err());
    })
    .await;

    trace("debug.root", |ctx| async move {
        ctx.set_baggage(AUTOTEL_DEBUG_BAGGAGE_KEY, "investigating");
        trace("debug.child", |_| async {}).await;
    })
    .await;

    trace("kept.root", |_| async {
        trace("kept.child", |_| async { force_keep() }).await;
    })
    .await;

    provider.force_flush().expect("flush");
    let exported: BTreeSet<String> = exporter
        .get_finished_spans()
        .expect("spans")
        .into_iter()
        .map(|span| span.name.into_owned())
        .collect();
    assert_eq!(
        exported,
        BTreeSet::from(
            [
                "debug.child",
                "debug.root",
                "failing.child",
                "failing.root",
                "kept.child",
                "kept.root",
            ]
            .map(String::from)
        )
    );
}

#[test]
fn a_later_local_root_gets_its_own_decision() {
    use opentelemetry::trace::{
        Span as _, SpanContext, SpanId, Status, TraceContextExt, TraceFlags, TraceId, TraceState,
        Tracer, TracerProvider,
    };
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(TailSamplingProcessor::new(
            SimpleSpanProcessor::new(exporter.clone()),
            Arc::new(AdaptiveSampler::errors_only()),
        ))
        .build();
    let tracer = provider.tracer("test");
    let remote = opentelemetry::Context::new().with_remote_span_context(SpanContext::new(
        TraceId::from(7),
        SpanId::from(9),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    ));

    // Two requests from the same upstream trace: the first succeeds and is dropped,
    // the second fails and is kept.
    drop(tracer.start_with_context("first.request", &remote));
    let mut second = tracer.start_with_context("second.request", &remote);
    second.set_status(Status::error("refused"));
    drop(second);

    provider.force_flush().expect("flush");
    let names: Vec<String> = exporter
        .get_finished_spans()
        .expect("spans")
        .into_iter()
        .map(|span| span.name.into_owned())
        .collect();
    assert_eq!(names, ["second.request"]);
}
