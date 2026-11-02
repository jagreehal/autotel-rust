//! Generative-AI spans and metrics following OpenTelemetry GenAI semantic conventions.

use crate::{Attributes, TraceContext, create_counter, trace_kind};
use opentelemetry::trace::SpanKind;
use opentelemetry::{KeyValue, metrics::Counter, metrics::Histogram};
use serde_json::Value;
use std::future::Future;

/// Options for a GenAI operation span (`chat`, `generate_content`, …).
#[derive(Clone, Debug)]
pub struct GenAiOptions {
    pub provider: String,
    pub operation: String,
    pub model: String,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub output_type: Option<String>,
    pub server_address: Option<String>,
    pub server_port: Option<u16>,
}

impl GenAiOptions {
    pub fn chat(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            operation: "chat".into(),
            model: model.into(),
            max_tokens: None,
            temperature: None,
            output_type: None,
            server_address: None,
            server_port: None,
        }
    }

    pub fn generate_content(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            operation: "generate_content".into(),
            model: model.into(),
            max_tokens: None,
            temperature: None,
            output_type: None,
            server_address: None,
            server_port: None,
        }
    }

    #[must_use]
    pub fn max_tokens(mut self, value: u64) -> Self {
        self.max_tokens = Some(value);
        self
    }

    #[must_use]
    pub fn temperature(mut self, value: f64) -> Self {
        self.temperature = Some(value);
        self
    }

    #[must_use]
    pub fn output_type(mut self, value: impl Into<String>) -> Self {
        self.output_type = Some(value.into());
        self
    }

    #[must_use]
    pub fn server(mut self, address: impl Into<String>, port: u16) -> Self {
        self.server_address = Some(address.into());
        self.server_port = Some(port);
        self
    }

    fn attributes(&self) -> Attributes {
        let mut attributes = Attributes::from([
            (
                "gen_ai.provider.name".into(),
                Value::String(self.provider.clone()),
            ),
            (
                "gen_ai.operation.name".into(),
                Value::String(self.operation.clone()),
            ),
            (
                "gen_ai.request.model".into(),
                Value::String(self.model.clone()),
            ),
        ]);
        if let Some(max_tokens) = self.max_tokens {
            attributes.insert("gen_ai.request.max_tokens".into(), Value::from(max_tokens));
        }
        if let Some(temperature) = self.temperature {
            attributes.insert(
                "gen_ai.request.temperature".into(),
                Value::from(temperature),
            );
        }
        if let Some(output_type) = &self.output_type {
            attributes.insert(
                "gen_ai.output.type".into(),
                Value::String(output_type.clone()),
            );
        }
        if let Some(address) = &self.server_address {
            attributes.insert("server.address".into(), Value::String(address.clone()));
        }
        if let Some(port) = self.server_port {
            attributes.insert("server.port".into(), Value::from(port));
        }
        attributes
    }
}

/// Token usage and response metadata recorded after a GenAI call finishes.
#[derive(Clone, Debug, Default)]
pub struct GenAiUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub response_model: Option<String>,
    /// One entry per choice, e.g. `["stop"]`. Exported as a string array.
    pub finish_reasons: Vec<String>,
}

/// Run `work` inside a CLIENT GenAI span with semantic-convention attributes.
pub async fn gen_ai<T, F, Fut>(options: GenAiOptions, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    let name = format!("{} {}", options.operation, options.model);
    let attributes = options.attributes();
    trace_kind(name, SpanKind::Client, |context| async move {
        context.set_attributes(attributes);
        work(context).await
    })
    .await
}

/// Stamp response-side GenAI attributes onto the active span.
pub fn record_gen_ai_usage(context: &TraceContext, usage: &GenAiUsage) {
    context
        .set_attribute("gen_ai.usage.input_tokens", usage.input_tokens)
        .set_attribute("gen_ai.usage.output_tokens", usage.output_tokens);
    if let Some(model) = &usage.response_model {
        context.set_attribute("gen_ai.response.model", model);
    }
    if !usage.finish_reasons.is_empty() {
        context.set_attribute("gen_ai.response.finish_reasons", &usage.finish_reasons);
    }
}

/// Metrics instruments for GenAI operation duration and token usage.
pub struct GenAiMetrics {
    pub operation_duration: Histogram<f64>,
    pub token_usage: Histogram<f64>,
    pub operation_count: Counter<u64>,
}

impl GenAiMetrics {
    /// Build after [`crate::init`]. Names follow GenAI semantic conventions.
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter(crate::INSTRUMENTATION_NAME);
        Self {
            // Boundaries advised by the GenAI semantic conventions; the SDK defaults
            // are tuned for milliseconds and would lump every call into one bucket.
            operation_duration: meter
                .f64_histogram("gen_ai.client.operation.duration")
                .with_unit("s")
                .with_description("GenAI operation duration")
                .with_boundaries(vec![
                    0.01, 0.02, 0.04, 0.08, 0.16, 0.32, 0.64, 1.28, 2.56, 5.12, 10.24, 20.48,
                    40.96, 81.92,
                ])
                .build(),
            token_usage: meter
                .f64_histogram("gen_ai.client.token.usage")
                .with_unit("{token}")
                .with_description("Number of tokens processed")
                .with_boundaries(vec![
                    1.0, 4.0, 16.0, 64.0, 256.0, 1024.0, 4096.0, 16384.0, 65536.0, 262144.0,
                    1048576.0, 4194304.0, 16777216.0, 67108864.0,
                ])
                .build(),
            operation_count: create_counter(
                "gen_ai.client.operation.count",
                "Number of GenAI operations",
            ),
        }
    }

    pub fn record_success(&self, options: &GenAiOptions, usage: &GenAiUsage, duration_secs: f64) {
        let base = [
            KeyValue::new("gen_ai.operation.name", options.operation.clone()),
            KeyValue::new("gen_ai.provider.name", options.provider.clone()),
            KeyValue::new("gen_ai.request.model", options.model.clone()),
        ];
        self.operation_duration.record(duration_secs, &base);
        self.operation_count.add(1, &base);
        self.token_usage.record(
            u64_as_f64(usage.input_tokens),
            &[
                KeyValue::new("gen_ai.operation.name", options.operation.clone()),
                KeyValue::new("gen_ai.provider.name", options.provider.clone()),
                KeyValue::new("gen_ai.token.type", "input"),
                KeyValue::new("gen_ai.request.model", options.model.clone()),
            ],
        );
        self.token_usage.record(
            u64_as_f64(usage.output_tokens),
            &[
                KeyValue::new("gen_ai.operation.name", options.operation.clone()),
                KeyValue::new("gen_ai.provider.name", options.provider.clone()),
                KeyValue::new("gen_ai.token.type", "output"),
                KeyValue::new("gen_ai.request.model", options.model.clone()),
            ],
        );
    }

    pub fn record_failure(&self, options: &GenAiOptions, duration_secs: f64, error_type: &str) {
        let attrs = [
            KeyValue::new("gen_ai.operation.name", options.operation.clone()),
            KeyValue::new("gen_ai.provider.name", options.provider.clone()),
            KeyValue::new("gen_ai.request.model", options.model.clone()),
            KeyValue::new("error.type", error_type.to_owned()),
        ];
        self.operation_duration.record(duration_secs, &attrs);
        self.operation_count.add(1, &attrs);
    }
}

impl Default for GenAiMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Evaluation score attributes on the active GenAI span.
pub fn record_evaluation(context: &TraceContext, name: &str, score: f64, label: &str) {
    context
        .set_attribute("gen_ai.evaluation.name", name)
        .set_attribute("gen_ai.evaluation.score.value", score)
        .set_attribute("gen_ai.evaluation.score.label", label);
}

#[allow(clippy::cast_precision_loss)]
fn u64_as_f64(value: u64) -> f64 {
    value as f64
}
