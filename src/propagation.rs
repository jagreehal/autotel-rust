use crate::TraceContext;
use http::HeaderMap;
use opentelemetry::{
    Context, KeyValue,
    baggage::BaggageExt,
    propagation::{TextMapCompositePropagator, TextMapPropagator},
};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};

fn propagator() -> TextMapCompositePropagator {
    TextMapCompositePropagator::new(vec![
        Box::new(TraceContextPropagator::new()),
        Box::new(BaggagePropagator::new()),
    ])
}

/// W3C `traceparent` and `baggage` headers describing the span active right now.
pub fn inject_trace_context() -> HeaderMap {
    let current = Context::current();
    let context = match current.get::<TraceContext>() {
        Some(active) => active.otel_context().with_baggage(
            active
                .baggage_entries()
                .into_iter()
                .map(|(key, value)| KeyValue::new(key, value)),
        ),
        None => current,
    };
    let mut headers = HeaderMap::new();
    propagator().inject_context(&context, &mut HeaderInjector(&mut headers));
    headers
}

/// The caller's span and baggage, read from incoming headers. Run the handler
/// inside it with `handler.with_context(extract_trace_context(&headers))`, and
/// every span the handler starts becomes a child of the caller's span.
pub fn extract_trace_context(headers: &HeaderMap) -> Context {
    propagator().extract(&HeaderExtractor(headers))
}
