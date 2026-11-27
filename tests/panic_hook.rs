//! `install_panic_hook` marks the span a panic unwinds through as failed, and the span
//! still reaches exporters and `TraceCollector`. One test, because the panic hook and
//! the tracer provider are process-wide.

use opentelemetry::{global, trace::Status};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};

#[tokio::test(flavor = "current_thread")]
async fn panic_hook_fails_the_span_a_panic_cuts_short() {
    // `TraceCollector::new` runs `init`, so install the exporter after it.
    let collector = autotel::TraceCollector::new("panic").unwrap();
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .build();
    global::set_tracer_provider(provider.clone());
    autotel::install_panic_hook();

    let joined = tokio::spawn(autotel::trace("panicking.span", |_| async {
        panic!("boom");
    }))
    .await;
    assert!(joined.is_err());

    provider.force_flush().unwrap();
    let span = exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .find(|span| span.name == "panicking.span")
        .expect("unwound span still exported");
    assert!(matches!(&span.status, Status::Error { description } if description.ends_with("boom")));
    let error_type = span
        .events
        .iter()
        .flat_map(|event| &event.attributes)
        .find(|kv| kv.key.as_str() == "exception.type")
        .map(|kv| kv.value.to_string());
    assert_eq!(error_type.as_deref(), Some("panic"));
    // The hook and the unwinding span both see the panic; it is recorded once.
    let exceptions = span
        .events
        .iter()
        .filter(|event| event.name == "exception")
        .count();
    assert_eq!(exceptions, 1);

    let captured = collector.expect_span("panicking.span");
    assert!(
        captured
            .error
            .as_deref()
            .is_some_and(|error| error.ends_with("boom"))
    );
}
