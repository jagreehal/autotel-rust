//! `redact_pii` covers spans that bypass `set_attribute`: `tracing`-bridged spans and
//! span events. One test, because `init` and the tracer provider are process-wide.

use autotel::{AutotelConfig, EventPayload, EventSubscriber, RedactPiiProcessor, span_bridge};
use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use std::sync::{Arc, Mutex};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

struct Capture(Arc<Mutex<Vec<EventPayload>>>);

impl EventSubscriber for Capture {
    fn name(&self) -> &str {
        "capture"
    }
    fn send(&self, payload: &EventPayload) {
        self.0.lock().unwrap().push(payload.clone());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bridged_spans_are_redacted() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let _runtime = autotel::init(
        AutotelConfig {
            redact_pii: true,
            ..AutotelConfig::new("redaction")
        }
        .subscriber(Capture(Arc::clone(&events))),
    )
    .unwrap();
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(RedactPiiProcessor::new(SimpleSpanProcessor::new(
            exporter.clone(),
        )))
        .build();
    global::set_tracer_provider(provider.clone());
    let _subscriber = tracing_subscriber::registry()
        .with(span_bridge())
        .set_default();

    {
        let span = tracing::info_span!(
            "signup",
            user.email = "jo@example.com",
            api_token = "abc123",
            access_tokens = "abc,def",
            gen_ai.usage.input_tokens = 12,
            plan = "pro"
        );
        let _entered = span.enter();
        tracing::info!(card_number = "4111111111111111", "charged jo@example.com");
    }

    provider.force_flush().unwrap();
    let span = exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .find(|span| span.name == "signup")
        .expect("bridged span exported");
    let value = |key: &str| {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    };
    assert_eq!(value("user.email").as_deref(), Some("[REDACTED]"));
    assert_eq!(value("api_token").as_deref(), Some("[REDACTED]"));
    assert_eq!(value("access_tokens").as_deref(), Some("[REDACTED]"));
    assert_eq!(value("gen_ai.usage.input_tokens").as_deref(), Some("12"));
    assert_eq!(value("plan").as_deref(), Some("pro"));
    let event = span.events.iter().next().expect("log became a span event");
    let card = event
        .attributes
        .iter()
        .find(|kv| kv.key.as_str() == "card_number")
        .map(|kv| kv.value.to_string());
    assert_eq!(card.as_deref(), Some("[REDACTED]"));
    assert_eq!(event.name, "charged [REDACTED]");

    autotel::track(
        "signup",
        [
            ("email".to_string(), serde_json::json!("jo@example.com")),
            ("plan".to_string(), serde_json::json!("pro")),
            (
                "user".to_string(),
                serde_json::json!({"password": "x", "notes": ["mail jo@example.com"]}),
            ),
        ]
        .into(),
    );
    let events = events.lock().unwrap();
    assert_eq!(
        events[0].attributes["email"],
        serde_json::json!("[REDACTED]")
    );
    assert_eq!(events[0].attributes["plan"], serde_json::json!("pro"));
    assert_eq!(
        events[0].attributes["user"],
        serde_json::json!({"password": "[REDACTED]", "notes": ["mail [REDACTED]"]})
    );
}
