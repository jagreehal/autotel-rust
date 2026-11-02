//! GenAI semantic-convention helpers. Serialized around process-wide `init`.

use autotel::{
    GenAiMetrics, GenAiOptions, GenAiUsage, TraceCollector, gen_ai, record_evaluation,
    record_gen_ai_usage,
};
use serde_json::json;
use std::sync::{Mutex, MutexGuard};

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
            collector: TraceCollector::new("genai").unwrap(),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn gen_ai_span_sets_semconv_attributes() {
    let scope = Scope::open();
    let options = GenAiOptions::chat("ollama", "llama3.2")
        .max_tokens(256)
        .temperature(0.2)
        .output_type("json")
        .server("127.0.0.1", 11434);

    let score = gen_ai(options, |context| async move {
        record_gen_ai_usage(
            &context,
            &GenAiUsage {
                input_tokens: 12,
                output_tokens: 4,
                response_model: Some("llama3.2".into()),
                finish_reasons: vec!["stop".into()],
            },
        );
        record_evaluation(&context, "fraud_risk", 0.91, "high");
        0.91_f64
    })
    .await;

    assert!((score - 0.91).abs() < f64::EPSILON);
    let span = scope.collector.expect_span("chat llama3.2");
    assert_eq!(span.attributes["otel.span.kind"], json!("client"));
    assert_eq!(span.attributes["gen_ai.provider.name"], json!("ollama"));
    assert_eq!(span.attributes["gen_ai.operation.name"], json!("chat"));
    assert_eq!(span.attributes["gen_ai.request.model"], json!("llama3.2"));
    assert_eq!(span.attributes["gen_ai.request.max_tokens"], json!(256));
    assert_eq!(span.attributes["gen_ai.request.temperature"], json!(0.2));
    assert_eq!(span.attributes["gen_ai.output.type"], json!("json"));
    assert_eq!(span.attributes["server.address"], json!("127.0.0.1"));
    assert_eq!(span.attributes["server.port"], json!(11434));
    assert_eq!(span.attributes["gen_ai.usage.input_tokens"], json!(12));
    assert_eq!(span.attributes["gen_ai.usage.output_tokens"], json!(4));
    assert_eq!(span.attributes["gen_ai.response.model"], json!("llama3.2"));
    assert_eq!(
        span.attributes["gen_ai.response.finish_reasons"],
        json!(["stop"])
    );
    assert_eq!(
        span.attributes["gen_ai.evaluation.name"],
        json!("fraud_risk")
    );
    assert_eq!(
        span.attributes["gen_ai.evaluation.score.value"],
        json!(0.91)
    );
    assert_eq!(
        span.attributes["gen_ai.evaluation.score.label"],
        json!("high")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn gen_ai_metrics_can_be_created_after_init() {
    let _scope = Scope::open();
    let metrics = GenAiMetrics::new();
    let options = GenAiOptions::chat("anthropic", "claude-haiku");
    let usage = GenAiUsage {
        input_tokens: 10,
        output_tokens: 3,
        ..GenAiUsage::default()
    };
    metrics.record_success(&options, &usage, 0.042);
    metrics.record_failure(&options, 1.5, "timeout");
}
