use opentelemetry::{
    Context, InstrumentationScope,
    logs::{AnyValue, LogRecord, Logger, LoggerProvider},
    trace::TraceContextExt,
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    logs::{LogProcessor, SdkLogRecord},
};
use serde_json::Value;
use std::{
    fmt,
    sync::{OnceLock, atomic::Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    Layer,
    filter::filter_fn,
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
    registry::LookupSpan,
};

/// Crates the exporter itself logs through. Forwarding their events would make
/// every export produce more telemetry to export.
const EXPORTER_TARGETS: [&str; 5] = ["opentelemetry", "hyper", "h2", "reqwest", "tower"];

/// A `tracing` layer that exports `tracing` spans (`#[tracing::instrument]`,
/// `info_span!`) as OpenTelemetry spans in the same traces as [`crate::trace`].
///
/// Parenting works both ways through the OpenTelemetry context: a `tracing` span
/// opened inside `trace` is its child, and `trace` inside an entered `tracing`
/// span is the child of that span. Build it after `init`; it uses the tracer
/// provider that is global at that moment.
pub fn span_bridge<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    tracing_opentelemetry::layer()
        .with_tracer(opentelemetry::global::tracer(crate::INSTRUMENTATION_NAME))
        .with_filter(filter_fn(|metadata| {
            !EXPORTER_TARGETS
                .iter()
                .any(|target| metadata.target().starts_with(target))
        }))
}

/// A `tracing` layer that sends every event to the OTLP endpoint as a log
/// record linked to the active span. Build it after `init`.
pub fn log_bridge<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let provider = crate::core::logger_provider()
        .expect("autotel::log_bridge() needs a provider: call autotel::init first");
    OpenTelemetryTracingBridge::<SdkLoggerProvider, SdkLogger>::new(&provider).with_filter(
        filter_fn(|metadata| {
            !EXPORTER_TARGETS
                .iter()
                .any(|target| metadata.target().starts_with(target))
        }),
    )
}

/// The tracing bridge sets only the observed time, so records would leave with
/// a zero timestamp and land at 1970 in a backend. Stamp the event time.
#[derive(Debug)]
pub(crate) struct StampTimestamp;

impl LogProcessor for StampTimestamp {
    fn emit(&self, record: &mut SdkLogRecord, _scope: &InstrumentationScope) {
        if record.timestamp().is_none() {
            record.set_timestamp(record.observed_timestamp().unwrap_or_else(SystemTime::now));
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
}

/// Applies `redact_pii` to log record attributes, which `tracing` fields on a
/// bridged event bypass. Registered before the exporter so it sees records first.
#[derive(Debug)]
pub(crate) struct RedactPii;

impl LogProcessor for RedactPii {
    fn emit(&self, record: &mut SdkLogRecord, _scope: &InstrumentationScope) {
        use crate::core::{pii_key, scrub_text};
        if !crate::core::REDACT_PII.load(Ordering::Relaxed) {
            return;
        }
        let redacted = |key: &opentelemetry::Key, value: &AnyValue| match value {
            _ if pii_key(key.as_str()) => Some(AnyValue::from("[REDACTED]")),
            AnyValue::String(text) => scrub_text(text.as_str()).map(AnyValue::from),
            _ => None,
        };
        let body = match record.body() {
            Some(AnyValue::String(text)) => scrub_text(text.as_str()),
            _ => None,
        };
        if !record
            .attributes_iter()
            .any(|(key, value)| redacted(key, value).is_some())
        {
            if let Some(body) = body {
                record.set_body(body.into());
            }
            return;
        }
        // The SDK can't edit or drop an attribute in place, so rebuild the record.
        static BLANK: OnceLock<SdkLogger> = OnceLock::new();
        let mut clean = BLANK
            .get_or_init(|| {
                SdkLoggerProvider::builder()
                    .build()
                    .logger("autotel.redact")
            })
            .create_log_record();
        if let Some(name) = record.event_name() {
            clean.set_event_name(name);
        }
        if let Some(target) = record.target() {
            clean.set_target(target.clone());
        }
        if let Some(time) = record.timestamp() {
            clean.set_timestamp(time);
        }
        if let Some(time) = record.observed_timestamp() {
            clean.set_observed_timestamp(time);
        }
        if let Some(text) = record.severity_text() {
            clean.set_severity_text(text);
        }
        if let Some(number) = record.severity_number() {
            clean.set_severity_number(number);
        }
        if let Some(original) = record.body() {
            clean.set_body(body.map_or_else(|| original.clone(), AnyValue::from));
        }
        if let Some(trace) = record.trace_context() {
            clean.set_trace_context(trace.trace_id, trace.span_id, trace.trace_flags);
        }
        clean.add_attributes(record.attributes_iter().map(|(key, value)| {
            (
                key.clone(),
                redacted(key, value).unwrap_or_else(|| value.clone()),
            )
        }));
        *record = clean;
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
}

/// One JSON object per line, with the active span's `trace_id` and `span_id`
/// stamped in. Use it as `tracing_subscriber::fmt::layer().event_format(JsonWithTraceIds)`.
pub struct JsonWithTraceIds;

impl<S, N> FormatEvent<S, N> for JsonWithTraceIds
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let mut line = vec![
            (
                "level".to_owned(),
                Value::from(event.metadata().level().as_str().to_ascii_lowercase()),
            ),
            ("time".to_owned(), Value::from(millis as u64)),
        ];
        let span = Context::map_current(|context| context.span().span_context().clone());
        if span.is_valid() {
            line.push(("trace_id".into(), Value::from(span.trace_id().to_string())));
            line.push(("span_id".into(), Value::from(span.span_id().to_string())));
            line.push((
                "trace_flags".into(),
                Value::from(format!("{:02x}", span.trace_flags().to_u8())),
            ));
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        line.extend(fields.values);
        if let Some(message) = fields.message {
            line.push(("msg".into(), Value::from(message)));
        }
        let body = line
            .iter()
            .map(|(key, value)| format!("{}:{value}", Value::from(key.as_str())))
            .collect::<Vec<_>>()
            .join(",");
        writeln!(writer, "{{{body}}}")
    }
}

#[derive(Default)]
struct Fields {
    message: Option<String>,
    values: Vec<(String, Value)>,
}

impl Fields {
    fn push(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.message = Some(
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned),
            );
        } else {
            self.values.push((field.name().to_owned(), value));
        }
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field, Value::from(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, Value::from(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, Value::from(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, Value::from(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push(field, Value::from(format!("{value:?}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SimpleLogProcessor};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    #[test]
    fn bridged_log_fields_are_redacted() {
        crate::core::REDACT_PII.store(true, Ordering::Relaxed);
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_log_processor(RedactPii)
            .with_log_processor(SimpleLogProcessor::new(exporter.clone()))
            .build();
        let _subscriber = tracing_subscriber::registry()
            .with(OpenTelemetryTracingBridge::new(&provider))
            .set_default();

        tracing::warn!(
            user.email = "jo@example.com",
            plan = "pro",
            note = "card 4111 1111 1111 1111",
            "signup for jo@example.com"
        );

        let logs = exporter.get_emitted_logs().unwrap();
        let record = &logs[0].record;
        let value = |name: &str| {
            record
                .attributes_iter()
                .find(|(key, _)| key.as_str() == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(value("user.email"), Some(AnyValue::from("[REDACTED]")));
        assert_eq!(value("plan"), Some(AnyValue::from("pro")));
        assert_eq!(value("note"), Some(AnyValue::from("card [REDACTED]")));
        assert_eq!(
            record.body(),
            Some(&AnyValue::from("signup for [REDACTED]"))
        );
        assert_eq!(record.severity_text(), Some("WARN"));
        assert_eq!(record.attributes_iter().count(), 3);
    }
}

#[cfg(test)]
mod scrub_tests {
    use crate::core::{pii_key, scrub_text};

    #[test]
    fn scrub_text_redacts_pii_shapes_in_free_text() {
        let cases = [
            (
                "user jo.b+x@mail.example.com signed up.",
                "user [REDACTED] signed up.",
            ),
            (
                "paid with 4111-1111-1111-1111 today",
                "paid with [REDACTED] today",
            ),
            ("card 4111111111111111", "card [REDACTED]"),
            (
                "login failed password=hunter2 for id 7",
                "login failed password=[REDACTED] for id 7",
            ),
            (
                r#"{"password":"hunter2","token": "abc123"}"#,
                r#"{"password":"[REDACTED]","token": "[REDACTED]"}"#,
            ),
            ("password=\"hunter 2\" ok", "password=\"[REDACTED]\" ok"),
            (
                "api_token: \"abc.def\", retry",
                "api_token: \"[REDACTED]\", retry",
            ),
            (
                "Authorization: Bearer eyJhbGciOi.x.y",
                "Authorization: Bearer [REDACTED]",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                scrub_text(input).as_deref(),
                Some(expected),
                "input: {input}"
            );
        }
        for clean in [
            "order 1234567890123 shipped", // 13 digits, fails Luhn
            "used 1500 tokens: ok",
            "see https://example.com/a?b=c",
            "@here deploy at 10:30",
            "élan café ✓",
        ] {
            assert_eq!(scrub_text(clean), None, "input: {clean}");
        }
    }

    #[test]
    fn scrub_text_stays_linear_on_long_values() {
        let input = "token=x:".repeat(50_000);
        let started = std::time::Instant::now();
        assert_eq!(scrub_text(&input).as_deref(), Some("token=[REDACTED]"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn token_counts_are_not_secrets() {
        assert!(pii_key("api_token"));
        assert!(pii_key("user.email"));
        assert!(!pii_key("gen_ai.usage.input_tokens"));
        assert!(!pii_key("gen_ai.request.max_tokens"));
        assert!(pii_key("access_tokens"));
    }
}
