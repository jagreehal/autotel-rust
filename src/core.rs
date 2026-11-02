use crate::{Attributes, EventPayload, Subscriber};
use opentelemetry::{
    Context, KeyValue,
    baggage::BaggageExt,
    context::FutureExt,
    global,
    trace::{SpanKind, Status, TraceContextExt, Tracer},
};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::SdkMeterProvider,
    trace::{BatchSpanProcessor, SdkTracerProvider},
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    sync::{
        Arc, LazyLock, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

pub const INSTRUMENTATION_NAME: &str = "io.github.jagreehal.autotel";
/// Baggage key that marks one request for full fidelity, whatever the sampler decides.
pub const AUTOTEL_DEBUG_BAGGAGE_KEY: &str = "autotel.debug";
/// A span's own keep-or-drop verdict, read by the tail sampler for its trace.
pub const AUTOTEL_SAMPLING_TAIL_KEEP: &str = "autotel.sampling.tail.keep";
/// Set beside `AUTOTEL_SAMPLING_TAIL_KEEP` once a verdict was made.
pub const AUTOTEL_SAMPLING_TAIL_EVALUATED: &str = "autotel.sampling.tail.evaluated";
/// Where `autotel-devtools` listens for OTLP/HTTP by default.
pub const DEVTOOLS_ENDPOINT: &str = "http://127.0.0.1:4318";
// Caps retained spans so a long-running service stays within bounded memory.
const MAX_COMPLETED_SPANS: usize = 10_000;

/// How finished telemetry leaves the process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OtlpProtocol {
    /// OTLP/HTTP with JSON payloads (ports like `4318`). Default.
    #[default]
    HttpJson,
    /// OTLP/gRPC (ports like `4317`). Needs the `grpc` crate feature.
    #[cfg(feature = "grpc")]
    Grpc,
}

#[derive(Clone, Default)]
pub struct AutotelConfig {
    pub service_name: String,
    pub service_version: String,
    pub environment: String,
    /// OTLP base URL, such as `http://collector:4318`. `OTEL_EXPORTER_OTLP_ENDPOINT` wins over it.
    pub otlp_endpoint: Option<String>,
    /// Override the traces signal URL. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` wins over it.
    pub otlp_traces_endpoint: Option<String>,
    /// Override the metrics signal URL. `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` wins over it.
    pub otlp_metrics_endpoint: Option<String>,
    /// Override the logs signal URL. `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` wins over it.
    pub otlp_logs_endpoint: Option<String>,
    /// Wire protocol for OTLP exporters. Defaults to HTTP JSON.
    pub otlp_protocol: OtlpProtocol,
    /// Export to a local `autotel-devtools` when no other endpoint is set.
    pub devtools: bool,
    /// Copy baggage onto every span this process starts, each key behind this prefix.
    pub baggage_prefix: Option<String>,
    /// Decides which finished traces to export. `None` exports every trace.
    pub sampler: Option<Arc<dyn crate::TailSampler>>,
    pub resource_attributes: Attributes,
    pub subscribers: Vec<Subscriber>,
    /// Redact PII before export: values under keys containing `email`, `card`,
    /// `password` or `token`, plus emails, card numbers, `Bearer` tokens and
    /// `password=…` pairs inside messages and other text. Pattern-based: keep PII in
    /// named fields where you can.
    pub redact_pii: bool,
    /// Keep a copy of the last 10,000 finished spans in memory for [`completed_spans`],
    /// [`crate::TraceCollector`] and the embedded [`crate::Devtools`]. Off by default:
    /// in production it costs a copy and a lock per span for nothing.
    pub capture_spans: bool,
    /// Install a default `tracing` subscriber (fmt + [`crate::log_bridge`]) during [`init`].
    /// Off by default so apps that set their own subscriber keep working; opt in with
    /// [`AutotelConfig::with_tracing_subscriber`].
    pub install_tracing_subscriber: bool,
}

impl AutotelConfig {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: "unknown".into(),
            environment: "development".into(),
            ..Self::default()
        }
    }

    /// Install a global `tracing` subscriber (fmt + [`crate::log_bridge`], `RUST_LOG` filter,
    /// default `info`) during [`init`]. Skipped silently if one is already set.
    #[must_use]
    pub fn with_tracing_subscriber(mut self) -> Self {
        self.install_tracing_subscriber = true;
        self
    }

    /// Send traces, metrics and logs to `autotel-devtools` on `127.0.0.1:4318`.
    #[must_use]
    pub fn devtools(mut self) -> Self {
        self.devtools = true;
        self
    }

    /// Copy incoming baggage onto spans as attributes; `""` keeps the keys as they are.
    /// Baggage comes from the caller, so `""` trusts upstream services with your
    /// attribute names.
    #[must_use]
    pub fn baggage(mut self, prefix: impl Into<String>) -> Self {
        self.baggage_prefix = Some(prefix.into());
        self
    }

    /// Keep finished spans in memory for [`completed_spans`] (tests, local devtools).
    #[must_use]
    pub fn capture_spans(mut self) -> Self {
        self.capture_spans = true;
        self
    }

    /// Keep or drop this process's spans of each trace once they finish
    /// (process-local tail sampling), such as
    /// `.sampler(AdaptiveSampler::production())`. `autotel.debug` baggage and
    /// `force_keep()` override it.
    #[must_use]
    pub fn sampler(mut self, sampler: impl crate::TailSampler + 'static) -> Self {
        self.sampler = Some(Arc::new(sampler));
        self
    }

    #[must_use]
    pub fn subscriber(mut self, subscriber: impl crate::EventSubscriber + 'static) -> Self {
        self.subscribers.push(Arc::new(subscriber) as Subscriber);
        self
    }

    /// Prefer OTLP/gRPC. Requires the `grpc` feature.
    #[cfg(feature = "grpc")]
    #[must_use]
    pub fn grpc(mut self) -> Self {
        self.otlp_protocol = OtlpProtocol::Grpc;
        self
    }

    /// Per-signal OTLP URL override for traces.
    #[must_use]
    pub fn traces_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_traces_endpoint = Some(endpoint.into());
        self
    }

    /// Per-signal OTLP URL override for metrics.
    #[must_use]
    pub fn metrics_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_metrics_endpoint = Some(endpoint.into());
        self
    }

    /// Per-signal OTLP URL override for logs.
    #[must_use]
    pub fn logs_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_logs_endpoint = Some(endpoint.into());
        self
    }

    fn base_endpoint(&self) -> Option<String> {
        std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| self.otlp_endpoint.clone())
            .or_else(|| self.devtools.then(|| DEVTOOLS_ENDPOINT.to_owned()))
            .map(|base| base.trim_end_matches('/').to_owned())
    }

    /// `autotel-devtools` only speaks OTLP/HTTP, so it is the export target only when
    /// neither `OTEL_EXPORTER_OTLP_ENDPOINT` nor `otlp_endpoint` is set.
    fn targets_devtools(&self) -> bool {
        self.devtools
            && self.otlp_endpoint.is_none()
            && std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").map_or(true, |value| value.is_empty())
    }

    /// Resolved OTLP URLs for each signal, or `None` when export is disabled.
    pub fn otlp_endpoints(&self) -> Option<OtlpEndpoints> {
        let base = self.base_endpoint();
        let protocol = if self.targets_devtools() {
            OtlpProtocol::HttpJson
        } else {
            self.otlp_protocol
        };
        let traces = signal_endpoint(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            self.otlp_traces_endpoint.as_deref(),
            base.as_deref(),
            protocol,
            "/v1/traces",
        )?;
        let metrics = signal_endpoint(
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
            self.otlp_metrics_endpoint.as_deref(),
            base.as_deref(),
            protocol,
            "/v1/metrics",
        )?;
        let logs = signal_endpoint(
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            self.otlp_logs_endpoint.as_deref(),
            base.as_deref(),
            protocol,
            "/v1/logs",
        )?;
        Some(OtlpEndpoints {
            traces,
            metrics,
            logs,
            protocol,
        })
    }
}

/// Fully resolved OTLP URLs after env and config overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtlpEndpoints {
    pub traces: String,
    pub metrics: String,
    pub logs: String,
    pub protocol: OtlpProtocol,
}

fn signal_endpoint(
    env_key: &str,
    configured: Option<&str>,
    base: Option<&str>,
    protocol: OtlpProtocol,
    http_path: &str,
) -> Option<String> {
    let explicit = std::env::var(env_key)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| configured.map(str::to_owned))
        .map(|value| value.trim_end_matches('/').to_owned());
    if let Some(endpoint) = explicit {
        return Some(endpoint);
    }
    let base = base?;
    Some(match protocol {
        OtlpProtocol::HttpJson if !base.ends_with(http_path) => format!("{base}{http_path}"),
        OtlpProtocol::HttpJson => base.to_owned(),
        #[cfg(feature = "grpc")]
        OtlpProtocol::Grpc => base.to_owned(),
    })
}

#[derive(Clone, Debug)]
pub struct SpanEvent {
    pub name: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug)]
pub struct SpanData {
    pub name: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub attributes: Attributes,
    pub events: Vec<SpanEvent>,
    pub error: Option<String>,
    pub duration: Duration,
}

/// The active span, as the code inside `trace` sees it.
#[derive(Clone)]
pub struct TraceContext {
    inner: Arc<Mutex<SpanData>>,
    baggage: Arc<Mutex<BTreeMap<String, String>>>,
    otel: Context,
}

impl TraceContext {
    pub fn set_attribute(&self, key: impl Into<String>, value: impl Serialize) -> &Self {
        let key = key.into();
        let mut value = serde_json::to_value(value).unwrap_or(Value::Null);
        redact_value(&key, &mut value);
        self.inner
            .lock()
            .expect("span mutex")
            .attributes
            .insert(key.clone(), value.clone());
        self.otel
            .span()
            .set_attribute(KeyValue::new(key, otel_value(&value)));
        self
    }

    /// Record `db.query.text`, with literals replaced by `?` via [`crate::sanitize_query`],
    /// using this span's `db.system.name` (set by [`crate::database`]) as the dialect.
    pub fn set_query_text(&self, sql: &str) -> &Self {
        let system = match self.snapshot().attributes.get("db.system.name") {
            Some(Value::String(system)) => system.clone(),
            _ => String::new(),
        };
        self.set_attribute("db.query.text", crate::sanitize_query(sql, &system))
    }

    /// A copy of the span as recorded so far.
    pub(crate) fn snapshot(&self) -> SpanData {
        self.inner.lock().expect("span mutex").clone()
    }

    pub fn set_attributes(&self, values: impl IntoIterator<Item = (String, Value)>) -> &Self {
        for (key, value) in values {
            self.set_attribute(key, value);
        }
        self
    }

    pub fn add_event(&self, name: impl Into<String>, attributes: Attributes) -> &Self {
        let name = name.into();
        self.inner
            .lock()
            .expect("span mutex")
            .events
            .push(SpanEvent {
                name: name.clone(),
                attributes: attributes.clone(),
            });
        self.otel.span().add_event(
            name,
            attributes
                .iter()
                .map(|(key, value)| KeyValue::new(key.clone(), otel_value(value)))
                .collect(),
        );
        self
    }

    pub fn trace_id(&self) -> String {
        self.otel.span().span_context().trace_id().to_string()
    }

    pub fn span_id(&self) -> String {
        self.otel.span().span_context().span_id().to_string()
    }

    pub fn baggage(&self, key: &str) -> Option<String> {
        self.baggage
            .lock()
            .expect("baggage mutex")
            .get(key)
            .cloned()
    }

    /// Baggage travels with every span started below this one, and in the
    /// `baggage` header that `inject_trace_context` writes.
    pub fn set_baggage(&self, key: impl Into<String>, value: impl Into<String>) -> &Self {
        self.baggage
            .lock()
            .expect("baggage mutex")
            .insert(key.into(), value.into());
        self
    }

    pub(crate) fn baggage_entries(&self) -> BTreeMap<String, String> {
        self.baggage.lock().expect("baggage mutex").clone()
    }

    pub(crate) fn otel_context(&self) -> &Context {
        &self.otel
    }

    /// Keep this trace whatever the sampler decides: a payment, an
    /// audit-relevant action, or a request you are debugging now.
    pub fn force_keep(&self) -> &Self {
        self.set_attribute(AUTOTEL_SAMPLING_TAIL_EVALUATED, true);
        self.set_attribute(AUTOTEL_SAMPLING_TAIL_KEEP, true)
    }

    /// True when the request in flight asked for full-fidelity capture.
    pub fn debug_requested(&self) -> bool {
        self.baggage(AUTOTEL_DEBUG_BAGGAGE_KEY)
            .is_some_and(|value| !value.is_empty() && value != "0")
    }

    pub fn record_error(&self, error: &dyn std::fmt::Display) {
        let message = error.to_string();
        let mut span = self.inner.lock().expect("span mutex");
        span.error = Some(message.clone());
        span.events.push(SpanEvent {
            name: "exception".into(),
            attributes: BTreeMap::from([(
                "exception.message".into(),
                Value::String(message.clone()),
            )]),
        });
        self.otel.span().add_event(
            "exception",
            vec![KeyValue::new("exception.message", message.clone())],
        );
        self.otel.span().set_status(Status::error(message));
    }
}

pub(crate) struct RuntimeState {
    config: AutotelConfig,
    provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            config: AutotelConfig::new("autotel"),
            provider: None,
            meter_provider: None,
            logger_provider: None,
        }
    }
}

/// Settings read on every span, kept out of the runtime mutex so concurrent spans
/// don't serialise on one lock. Written only by `init`.
pub(crate) static REDACT_PII: AtomicBool = AtomicBool::new(false);
static CAPTURE_SPANS: AtomicBool = AtomicBool::new(false);
type SpanSettings = (Option<String>, Option<Arc<dyn crate::TailSampler>>);
static SPAN_SETTINGS: RwLock<SpanSettings> = RwLock::new((None, None));
static COMPLETED: Mutex<VecDeque<SpanData>> = Mutex::new(VecDeque::new());

fn span_settings() -> std::sync::RwLockReadGuard<'static, SpanSettings> {
    SPAN_SETTINGS.read().unwrap_or_else(|e| e.into_inner())
}

/// Turn on the in-memory span buffer (the embedded devtools needs it).
pub(crate) fn enable_span_capture() {
    CAPTURE_SPANS.store(true, Ordering::Relaxed);
}

static RUNTIME: LazyLock<Mutex<RuntimeState>> =
    LazyLock::new(|| Mutex::new(RuntimeState::default()));

pub(crate) fn runtime() -> &'static Mutex<RuntimeState> {
    &RUNTIME
}

pub(crate) fn logger_provider() -> Option<SdkLoggerProvider> {
    runtime()
        .lock()
        .expect("runtime mutex")
        .logger_provider
        .clone()
}

pub(crate) fn subscribers() -> Vec<Subscriber> {
    runtime()
        .lock()
        .expect("runtime mutex")
        .config
        .subscribers
        .clone()
}

/// Keeps telemetry running; dropping it flushes and shuts the exporters down.
#[must_use = "dropping this shuts telemetry down at once: bind it with `let _telemetry = autotel::init(...)?`"]
pub struct AutotelRuntime(());

impl AutotelRuntime {
    pub fn force_flush(&self) -> bool {
        flush()
    }
}

impl Drop for AutotelRuntime {
    fn drop(&mut self) {
        shutdown();
    }
}

pub fn init(config: AutotelConfig) -> anyhow::Result<AutotelRuntime> {
    anyhow::ensure!(
        !config.service_name.trim().is_empty(),
        "service_name is required"
    );
    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attributes(
            [
                KeyValue::new("service.version", config.service_version.clone()),
                KeyValue::new("deployment.environment.name", config.environment.clone()),
            ]
            .into_iter()
            .chain(
                config
                    .resource_attributes
                    .iter()
                    .map(|(key, value)| KeyValue::new(key.clone(), otel_value(value))),
            ),
        )
        .build();
    let mut builder = SdkTracerProvider::builder().with_resource(resource.clone());
    let mut meter_builder = SdkMeterProvider::builder().with_resource(resource.clone());
    let mut logger_builder = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_log_processor(crate::logs::StampTimestamp);
    if let Some(endpoints) = config.otlp_endpoints() {
        let timeout = Duration::from_secs(5);
        // Redact on the way out so spans that skip `set_attribute` (the `tracing`
        // bridge, raw OTel API calls) honour `redact_pii` too.
        let batch = RedactPiiProcessor(
            BatchSpanProcessor::builder(build_span_exporter(&endpoints, timeout)?).build(),
        );
        builder = match &config.sampler {
            Some(sampler) => builder.with_span_processor(crate::TailSamplingProcessor::new(
                batch,
                Arc::clone(sampler),
            )),
            None => builder.with_span_processor(batch),
        };
        meter_builder =
            meter_builder.with_periodic_exporter(build_metric_exporter(&endpoints, timeout)?);
        logger_builder = logger_builder
            .with_log_processor(crate::logs::RedactPii)
            .with_batch_exporter(build_log_exporter(&endpoints, timeout)?);
    }
    let provider = builder.build();
    let meter_provider = meter_builder.build();
    let logger_provider = logger_builder.build();
    global::set_tracer_provider(provider.clone());
    global::set_meter_provider(meter_provider.clone());
    // Instruments bind to the provider current when they are built; rebuild them
    // against this one so metrics recorded before (or across) `init` aren't lost.
    crate::instruments::reset();
    let install_subscriber = config.install_tracing_subscriber;
    REDACT_PII.store(config.redact_pii, Ordering::Relaxed);
    CAPTURE_SPANS.store(config.capture_spans, Ordering::Relaxed);
    *SPAN_SETTINGS.write().unwrap_or_else(|e| e.into_inner()) =
        (config.baggage_prefix.clone(), config.sampler.clone());
    COMPLETED.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *runtime().lock().expect("runtime mutex") = RuntimeState {
        config,
        provider: Some(provider),
        meter_provider: Some(meter_provider),
        logger_provider: Some(logger_provider),
    };
    if install_subscriber {
        install_default_tracing_subscriber();
    }
    Ok(AutotelRuntime(()))
}

type LogFilterHandle =
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;

static LOG_FILTER: std::sync::OnceLock<LogFilterHandle> = std::sync::OnceLock::new();

fn install_default_tracing_subscriber() {
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt, reload, util::SubscriberInitExt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter, handle) = reload::Layer::new(filter);
    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(crate::span_bridge())
        .with(crate::log_bridge())
        .try_init();
    // `try_init` succeeds at most once per process, so this is the only write.
    if installed.is_ok() {
        LOG_FILTER.get_or_init(|| handle);
    }
}

/// Change the log filter at runtime (`"info,orders=debug"`), without a redeploy.
/// Works with the subscriber installed by [`AutotelConfig::with_tracing_subscriber`];
/// with your own subscriber, keep your own `tracing_subscriber::reload` handle instead.
pub fn set_log_filter(directives: &str) -> anyhow::Result<()> {
    let handle = LOG_FILTER.get().ok_or_else(|| {
        anyhow::anyhow!(
            "set_log_filter needs the subscriber from AutotelConfig::with_tracing_subscriber"
        )
    })?;
    let filter = tracing_subscriber::EnvFilter::try_new(directives)?;
    handle.reload(filter)?;
    Ok(())
}

/// Send everything batched so far without stopping the exporters.
pub fn flush() -> bool {
    let runtime = runtime().lock().expect("runtime mutex");
    runtime
        .provider
        .as_ref()
        .is_none_or(|provider| provider.force_flush().is_ok())
        && runtime
            .meter_provider
            .as_ref()
            .is_none_or(|provider| provider.force_flush().is_ok())
        && runtime
            .logger_provider
            .as_ref()
            .is_none_or(|provider| provider.force_flush().is_ok())
}

/// Flush and stop the exporters. Spans are batched, so a process that exits
/// without this loses the last batch.
pub fn shutdown() {
    let mut runtime = runtime().lock().expect("runtime mutex");
    let provider = runtime.provider.take();
    let meter_provider = runtime.meter_provider.take();
    let logger_provider = runtime.logger_provider.take();
    let endpoint = runtime
        .config
        .otlp_endpoints()
        .map(|endpoints| endpoints.traces);
    drop(runtime);
    if let Some(provider) = provider {
        // Flush before shutdown so a failed export reaches stderr.
        if let (Err(_), Some(endpoint)) = (provider.force_flush(), endpoint) {
            // Endpoints and SDK error text can carry credentials: print the redacted URL only.
            let endpoint = crate::instrumentation::redact_url(&endpoint);
            eprintln!(
                "[autotel] could not export spans to {endpoint}. Is autotel-devtools running? \
                 Start it, or set OTEL_EXPORTER_OTLP_ENDPOINT to match its banner."
            );
        }
        report_shutdown("traces", provider.shutdown());
    }
    if let Some(provider) = meter_provider {
        report_shutdown("metrics", provider.shutdown());
    }
    if let Some(provider) = logger_provider {
        report_shutdown("logs", provider.shutdown());
    }
}

/// `shutdown` runs from `Drop`, so it can't return an error; say so instead of
/// losing the last batch silently.
fn report_shutdown(signal: &str, result: opentelemetry_sdk::error::OTelSdkResult) {
    if let Err(error) = result {
        eprintln!(
            "[autotel] {signal} did not shut down cleanly; the last batch may be lost ({error})"
        );
    }
}

fn build_span_exporter(
    endpoints: &OtlpEndpoints,
    timeout: Duration,
) -> anyhow::Result<opentelemetry_otlp::SpanExporter> {
    match endpoints.protocol {
        OtlpProtocol::HttpJson => Ok(opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .with_endpoint(endpoints.traces.clone())
            .with_timeout(timeout)
            .build()?),
        #[cfg(feature = "grpc")]
        OtlpProtocol::Grpc => Ok(opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoints.traces.clone())
            .with_timeout(timeout)
            .build()?),
    }
}

fn build_metric_exporter(
    endpoints: &OtlpEndpoints,
    timeout: Duration,
) -> anyhow::Result<opentelemetry_otlp::MetricExporter> {
    match endpoints.protocol {
        OtlpProtocol::HttpJson => Ok(opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .with_endpoint(endpoints.metrics.clone())
            .with_timeout(timeout)
            .build()?),
        #[cfg(feature = "grpc")]
        OtlpProtocol::Grpc => Ok(opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoints.metrics.clone())
            .with_timeout(timeout)
            .build()?),
    }
}

fn build_log_exporter(
    endpoints: &OtlpEndpoints,
    timeout: Duration,
) -> anyhow::Result<opentelemetry_otlp::LogExporter> {
    match endpoints.protocol {
        OtlpProtocol::HttpJson => Ok(opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpJson)
            .with_endpoint(endpoints.logs.clone())
            .with_timeout(timeout)
            .build()?),
        #[cfg(feature = "grpc")]
        OtlpProtocol::Grpc => Ok(opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .with_endpoint(endpoints.logs.clone())
            .with_timeout(timeout)
            .build()?),
    }
}

pub async fn trace<T, F, Fut>(name: impl Into<String>, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    trace_kind(name, SpanKind::Internal, work).await
}

/// Like [`trace`], but sets the OpenTelemetry span kind (server, client, …).
pub async fn trace_kind<T, F, Fut>(name: impl Into<String>, kind: SpanKind, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    let name = name.into();
    // The active span travels with the future via the OTel Context, not the thread,
    // so concurrent and work-stolen tasks keep their own parents. The parent may
    // also be remote: a span context extracted from another service's headers.
    let parent_cx = Context::current();
    let parent_span = parent_cx.span().span_context().clone();
    let parent_span_id = parent_span
        .is_valid()
        .then(|| parent_span.span_id().to_string());
    let baggage = parent_cx.get::<TraceContext>().map_or_else(
        || {
            parent_cx
                .baggage()
                .iter()
                .map(|(key, (value, _))| (key.to_string(), value.to_string()))
                .collect()
        },
        TraceContext::baggage_entries,
    );
    let tracer = global::tracer(INSTRUMENTATION_NAME);
    let span = tracer
        .span_builder(name.clone())
        .with_kind(kind.clone())
        .start_with_context(&tracer, &parent_cx);
    let otel = parent_cx.with_span(span);
    let span_context = otel.span().span_context().clone();
    let started = Instant::now();
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "otel.span.kind".into(),
        Value::String(span_kind_name(&kind).into()),
    );
    let inner = Arc::new(Mutex::new(SpanData {
        name,
        trace_id: span_context.trace_id().to_string(),
        span_id: span_context.span_id().to_string(),
        parent_span_id,
        attributes,
        events: Vec::new(),
        error: None,
        duration: Duration::ZERO,
    }));
    let context = TraceContext {
        inner: Arc::clone(&inner),
        baggage: Arc::new(Mutex::new(baggage)),
        otel: otel.clone(),
    };
    if let Some(prefix) = &span_settings().0 {
        for (key, value) in context.baggage_entries() {
            context.set_attribute(format!("{prefix}{key}"), value);
        }
    }
    let scope = otel.with_value(context.clone());
    let output = work(context.clone()).with_context(scope).await;
    let duration = started.elapsed();
    if let Some(sampler) = &span_settings().1 {
        let (failed, force_kept) = {
            let span = inner.lock().expect("span mutex");
            (
                span.error.is_some(),
                span.attributes.get(AUTOTEL_SAMPLING_TAIL_KEEP) == Some(&Value::Bool(true)),
            )
        };
        // A debug request is a verdict too: an unevaluated span leaves the
        // trace to the processor, which would sample it out.
        if !force_kept {
            let keep = context.debug_requested()
                || sampler.should_keep_trace(&context.trace_id(), !failed, duration);
            context.set_attribute(AUTOTEL_SAMPLING_TAIL_KEEP, keep);
            context.set_attribute(AUTOTEL_SAMPLING_TAIL_EVALUATED, true);
        }
    }
    otel.span().end();
    if CAPTURE_SPANS.load(Ordering::Relaxed) {
        let mut span = inner.lock().expect("span mutex").clone();
        span.duration = duration;
        let mut completed = COMPLETED.lock().unwrap_or_else(|e| e.into_inner());
        if completed.len() == MAX_COMPLETED_SPANS {
            completed.pop_front();
        }
        completed.push_back(span);
    }
    output
}

fn span_kind_name(kind: &SpanKind) -> &'static str {
    match kind {
        SpanKind::Internal => "internal",
        SpanKind::Server => "server",
        SpanKind::Client => "client",
        SpanKind::Producer => "producer",
        SpanKind::Consumer => "consumer",
    }
}

pub async fn trace_result<T, E, F, Fut>(name: impl Into<String>, work: F) -> Result<T, E>
where
    E: std::fmt::Display,
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    trace(name, |context| async move {
        let result = work(context.clone()).await;
        if let Err(error) = &result {
            context.record_error(error);
        }
        result
    })
    .await
}

pub async fn with_baggage<T, F, Fut>(values: BTreeMap<String, String>, work: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let current = current_context().expect("baggage requires an active trace");
    let mut merged = current.baggage_entries();
    merged.extend(values);
    let replacement = TraceContext {
        inner: Arc::clone(&current.inner),
        baggage: Arc::new(Mutex::new(merged)),
        otel: current.otel.clone(),
    };
    work()
        .with_context(Context::current().with_value(replacement))
        .await
}

/// A business event, sent to every subscriber, carrying the active trace's ids.
pub fn track(name: impl Into<String>, mut attributes: Attributes) {
    for (key, value) in &mut attributes {
        redact_value(key, value);
    }
    if let Some(context) = current_context() {
        attributes.insert("traceId".into(), Value::String(context.trace_id()));
        attributes.insert("spanId".into(), Value::String(context.span_id()));
    }
    let payload = EventPayload {
        kind: "event".into(),
        name: name.into(),
        attributes,
        timestamp: SystemTime::now(),
    };
    for subscriber in subscribers() {
        subscriber.send(&payload);
    }
}

/// The last finished spans, oldest first. Empty unless span capture is on
/// ([`AutotelConfig::capture_spans`], [`crate::TraceCollector`], [`crate::Devtools`]).
pub fn completed_spans() -> Vec<SpanData> {
    COMPLETED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

pub fn current_context() -> Option<TraceContext> {
    Context::current().get::<TraceContext>().cloned()
}

/// The active span. Panics outside `trace`, the way a missing span would
/// otherwise fail silently.
pub fn ctx() -> TraceContext {
    current_context().expect("autotel::ctx() needs an active span: call it inside trace()")
}

/// Keep the active trace whatever the sampler decides. Does nothing outside `trace`.
pub fn force_keep() {
    if let Some(context) = current_context() {
        context.force_keep();
    }
}

pub fn counter(name: &str, value: u64) {
    global::meter(INSTRUMENTATION_NAME)
        .u64_counter(name.to_owned())
        .build()
        .add(value, &[]);
}
pub fn histogram(name: &str, value: f64, unit: &str) {
    global::meter(INSTRUMENTATION_NAME)
        .f64_histogram(name.to_owned())
        .with_unit(unit.to_owned())
        .build()
        .record(value, &[]);
}
pub fn up_down_counter(name: &str, value: i64) {
    global::meter(INSTRUMENTATION_NAME)
        .i64_up_down_counter(name.to_owned())
        .build()
        .add(value, &[]);
}

/// A histogram to record into with attributes. Create it after `init`.
pub fn create_histogram(
    name: &str,
    unit: &str,
    description: &str,
) -> opentelemetry::metrics::Histogram<f64> {
    global::meter(INSTRUMENTATION_NAME)
        .f64_histogram(name.to_owned())
        .with_unit(unit.to_owned())
        .with_description(description.to_owned())
        .build()
}

/// A counter to add to with attributes. Create it after `init`.
pub fn create_counter(name: &str, description: &str) -> opentelemetry::metrics::Counter<u64> {
    global::meter(INSTRUMENTATION_NAME)
        .u64_counter(name.to_owned())
        .with_description(description.to_owned())
        .build()
}

/// Span processor that replaces PII-looking attribute values (keys containing
/// `email`, `card`, `password`, `token`; span and event attributes) with
/// `[REDACTED]` before they reach `inner`, while [`AutotelConfig::redact_pii`] is on.
///
/// `init` installs it in its own export pipeline. Use it when you build your own
/// tracer provider, so `tracing`-bridged and raw OTel spans are redacted too.
#[derive(Debug)]
pub struct RedactPiiProcessor<P>(P);

impl<P> RedactPiiProcessor<P> {
    pub fn new(inner: P) -> Self {
        Self(inner)
    }
}

fn redact_attributes(attributes: &mut [KeyValue]) {
    for attribute in attributes {
        if pii_key(attribute.key.as_str()) {
            attribute.value = "[REDACTED]".into();
        } else if let opentelemetry::Value::String(text) = &attribute.value
            && let Some(clean) = scrub_text(text.as_str())
        {
            attribute.value = clean.into();
        }
    }
}

impl<P: opentelemetry_sdk::trace::SpanProcessor> opentelemetry_sdk::trace::SpanProcessor
    for RedactPiiProcessor<P>
{
    fn on_start(&self, span: &mut opentelemetry_sdk::trace::Span, cx: &Context) {
        self.0.on_start(span, cx);
    }

    fn on_end(&self, mut span: opentelemetry_sdk::trace::SpanData) {
        if REDACT_PII.load(Ordering::Relaxed) {
            redact_attributes(&mut span.attributes);
            for event in span.events.events.iter_mut() {
                // Bridged `tracing` events carry the log message as the event name.
                if let Some(clean) = scrub_text(&event.name) {
                    event.name = clean.into();
                }
                redact_attributes(&mut event.attributes);
            }
            if let Status::Error { description } = &span.status
                && let Some(clean) = scrub_text(description)
            {
                span.status = Status::error(clean);
            }
        }
        self.0.on_end(span);
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.0.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.0.set_resource(resource);
    }
}

/// Applies [`AutotelConfig::redact_pii`] to one key/value pair.
fn redact_value(key: &str, value: &mut Value) {
    if !REDACT_PII.load(Ordering::Relaxed) {
        return;
    }
    if pii_key(key) {
        *value = Value::String("[REDACTED]".into());
    } else if let Some(clean) = value.as_str().and_then(scrub_text) {
        *value = Value::String(clean);
    }
}

pub(crate) fn pii_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    // Token counts (`gen_ai.usage.input_tokens`, `gen_ai.request.max_tokens`) are not
    // secrets; any other `*_tokens` key (`access_tokens`) still is.
    let token_count =
        lower == "tokens" || lower.starts_with("gen_ai.usage.") || lower.ends_with("max_tokens");
    ["email", "card", "password", "token"]
        .iter()
        .any(|part| lower.contains(part))
        && !token_count
}

/// Redacts PII inside free text (log messages, string values): email addresses,
/// Luhn-valid card numbers, `Bearer` tokens and the value in `password=…`-style
/// pairs whose key [`pii_key`] matches. `None` when nothing matched.
///
/// Matches common shapes; named fields get exact key-based redaction.
pub(crate) fn scrub_text(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut hits: Vec<(usize, usize)> = Vec::new();

    let local = |b: u8| b.is_ascii_alphanumeric() || b"._%+-".contains(&b);
    let host = |b: u8| b.is_ascii_alphanumeric() || b".-".contains(&b);
    for (at, _) in text.match_indices('@') {
        let start = bytes[..at]
            .iter()
            .rposition(|&b| !local(b))
            .map_or(0, |i| i + 1);
        let mut end = bytes[at + 1..]
            .iter()
            .position(|&b| !host(b))
            .map_or(bytes.len(), |i| at + 1 + i);
        while end > at + 1 && bytes[end - 1] == b'.' {
            end -= 1; // a sentence's full stop, not the domain
        }
        let domain = &text[at + 1..end];
        if start < at && domain.contains('.') && !domain.starts_with('.') {
            hits.push((start, end));
        }
    }

    // Card numbers: 13-19 digits, optionally grouped by single spaces or dashes.
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() || (i > 0 && bytes[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let (mut j, mut end, mut digits) = (i, i, Vec::new());
        while j < bytes.len() {
            if bytes[j].is_ascii_digit() {
                digits.push(bytes[j] - b'0');
                j += 1;
                end = j;
            } else if matches!(bytes[j], b' ' | b'-')
                && bytes.get(j + 1).is_some_and(u8::is_ascii_digit)
            {
                j += 1;
            } else {
                break;
            }
        }
        let bounded = bytes.get(end).is_none_or(|b| !b.is_ascii_alphanumeric());
        if (13..=19).contains(&digits.len()) && bounded && luhn(&digits) {
            hits.push((i, end));
        }
        i = end.max(i + 1);
    }

    // `password=…`, `api_token: …`, `Authorization: Bearer …`.
    let word = |b: u8| b.is_ascii_alphanumeric() || b"_.-".contains(&b);
    let value_end = |from: usize| {
        bytes[from..]
            .iter()
            .position(|b| b.is_ascii_whitespace() || b",;&\"'".contains(b))
            .map_or(bytes.len(), |n| from + n)
    };
    for (sep, _) in text.match_indices(['=', ':']) {
        // `"password": "x"` (JSON): step over the key's closing quote.
        let mut key_end = bytes[..sep]
            .iter()
            .rposition(|&b| b != b' ')
            .map_or(0, |n| n + 1);
        if key_end > 0 && matches!(bytes[key_end - 1], b'"' | b'\'') {
            key_end -= 1;
        }
        let key_start = bytes[..key_end]
            .iter()
            .rposition(|&b| !word(b))
            .map_or(0, |n| n + 1);
        if key_start == key_end || !pii_key(&text[key_start..key_end]) {
            continue;
        }
        let mut start = sep + 1;
        while bytes.get(start) == Some(&b' ') {
            start += 1;
        }
        // A quoted value runs to its closing quote, spaces included.
        let end = match bytes.get(start) {
            Some(&quote @ (b'"' | b'\'')) => {
                start += 1;
                bytes[start..]
                    .iter()
                    .position(|&b| b == quote)
                    .map_or(bytes.len(), |n| start + n)
            }
            _ => value_end(start),
        };
        if end > start {
            hits.push((start, end));
        }
    }
    let lower = text.to_ascii_lowercase();
    for (at, _) in lower.match_indices("bearer ") {
        let start = at + "bearer ".len();
        let end = value_end(start);
        if end > start {
            hits.push((start, end));
        }
    }

    if hits.is_empty() {
        return None;
    }
    hits.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, end) in hits {
        if end <= copied {
            continue;
        }
        if start >= copied {
            out.push_str(&text[copied..start]);
            out.push_str("[REDACTED]");
        }
        copied = end;
    }
    out.push_str(&text[copied..]);
    Some(out)
}

fn luhn(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(n, &d)| {
            let d = u32::from(d);
            if !n.is_multiple_of(2) {
                if d > 4 { d * 2 - 9 } else { d * 2 }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

pub(crate) fn otel_value(value: &Value) -> opentelemetry::Value {
    match value {
        Value::Bool(value) => (*value).into(),
        Value::Number(value) => value
            .as_i64()
            .map_or_else(|| value.as_f64().unwrap_or_default().into(), Into::into),
        Value::String(value) => value.clone().into(),
        // Homogeneous string arrays (`gen_ai.response.finish_reasons`) export as OTel arrays.
        Value::Array(items) if items.iter().all(Value::is_string) => opentelemetry::Value::Array(
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.to_owned().into()))
                .collect::<Vec<opentelemetry::StringValue>>()
                .into(),
        ),
        other => other.to_string().into(),
    }
}

pub(crate) fn hash_hex(bytes: &[u8], length: usize) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..length]
        .to_owned()
}
