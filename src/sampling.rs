use crate::hash_hex;
use rand::RngExt;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct SamplingContext {
    pub operation_name: String,
}
impl SamplingContext {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            operation_name: name.into(),
        }
    }
}
pub trait SamplingStrategy {
    fn should_sample(&mut self, context: &SamplingContext) -> bool;
    fn sample_rate(&self, context: &SamplingContext) -> f64;
}

/// Decides a trace once it has run, when its outcome and duration are known.
/// Pass one to `AutotelConfig::sampler`.
pub trait TailSampler: Send + Sync {
    fn should_keep_trace(&self, trace_id: &str, success: bool, duration: Duration) -> bool;

    /// Keep the whole trace when any one of its spans matches, whatever else is
    /// decided. Sees every span, `tracing`-bridged ones included.
    fn keep_span(&self, _attributes: &[opentelemetry::KeyValue]) -> bool {
        false
    }
}

type KeepRule = (
    opentelemetry::Key,
    Arc<dyn Fn(&opentelemetry::Value) -> bool + Send + Sync>,
);

pub fn hash_unit_interval(key: &str) -> f64 {
    let hex = hash_hex(key.as_bytes(), 16);
    u64::from_str_radix(&hex, 16).expect("hash is hexadecimal") as f64 / u64::MAX as f64
}

pub struct AlwaysSampler;
impl SamplingStrategy for AlwaysSampler {
    fn should_sample(&mut self, _: &SamplingContext) -> bool {
        true
    }
    fn sample_rate(&self, _: &SamplingContext) -> f64 {
        1.0
    }
}

pub struct RandomSampler {
    probability: f64,
}
impl RandomSampler {
    pub fn new(probability: f64) -> Self {
        Self { probability }
    }
}
impl SamplingStrategy for RandomSampler {
    fn should_sample(&mut self, _: &SamplingContext) -> bool {
        rand::rng().random::<f64>() < self.probability
    }
    fn sample_rate(&self, _: &SamplingContext) -> f64 {
        1.0 / self.probability
    }
}

pub struct DeterministicSampler<F> {
    probability: f64,
    key: F,
}
impl<F> DeterministicSampler<F> {
    pub fn new(probability: f64, key: F) -> Self {
        Self { probability, key }
    }
}
impl<F: Fn(&SamplingContext) -> String> SamplingStrategy for DeterministicSampler<F> {
    fn should_sample(&mut self, context: &SamplingContext) -> bool {
        hash_unit_interval(&(self.key)(context)) < self.probability
    }
    fn sample_rate(&self, _: &SamplingContext) -> f64 {
        1.0 / self.probability
    }
}

pub struct AdaptiveSampler {
    baseline_probability: f64,
    slow_threshold: Duration,
    keep_rules: Vec<KeepRule>,
}
impl AdaptiveSampler {
    pub fn new(baseline_probability: f64, slow_threshold: Duration) -> Self {
        Self {
            baseline_probability,
            slow_threshold,
            keep_rules: Vec::new(),
        }
    }
    /// Keep every trace with a span whose `key` attribute matches `predicate`, the
    /// in-process version of a Collector `tail_sampling` attribute policy:
    ///
    /// ```
    /// use autotel::{AdaptiveSampler, AutotelConfig};
    /// use opentelemetry::Value;
    ///
    /// let config = AutotelConfig::new("api").sampler(
    ///     AdaptiveSampler::production()
    ///         .keep_when("auth.success", |v| matches!(v, Value::Bool(false)))
    ///         .keep_when("db.response.returned_rows", |v| matches!(v, Value::I64(n) if *n >= 100))
    ///         .keep_when("security.event.type", |_| true),
    /// );
    /// # let _ = config;
    /// ```
    #[must_use]
    pub fn keep_when(
        mut self,
        key: impl Into<opentelemetry::Key>,
        predicate: impl Fn(&opentelemetry::Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.keep_rules.push((key.into(), Arc::new(predicate)));
        self
    }
    /// Errors, plus anything past one second, plus one in ten of the rest.
    pub fn production() -> Self {
        Self::new(0.1, Duration::from_secs(1))
    }
    /// Only bad outcomes: no baseline, so failures and slow traces alone survive.
    pub fn errors_only() -> Self {
        Self::new(0.0, Duration::from_secs(1))
    }
    /// The baseline hashes the trace id, so every span of a trace, and the
    /// trace-level decision, agree.
    pub fn should_keep(&self, trace_id: &str, success: bool, duration: Duration) -> bool {
        !success
            || duration >= self.slow_threshold
            || hash_unit_interval(trace_id) < self.baseline_probability
    }
}
impl TailSampler for AdaptiveSampler {
    fn should_keep_trace(&self, trace_id: &str, success: bool, duration: Duration) -> bool {
        self.should_keep(trace_id, success, duration)
    }
    fn keep_span(&self, attributes: &[opentelemetry::KeyValue]) -> bool {
        self.keep_rules.iter().any(|(key, predicate)| {
            attributes
                .iter()
                .any(|attribute| attribute.key == *key && predicate(&attribute.value))
        })
    }
}
impl SamplingStrategy for AdaptiveSampler {
    fn should_sample(&mut self, _: &SamplingContext) -> bool {
        rand::rng().random::<f64>() < self.baseline_probability
    }
    fn sample_rate(&self, _: &SamplingContext) -> f64 {
        1.0 / self.baseline_probability
    }
}

struct Bucket {
    started: Instant,
    count: u64,
    rate: f64,
}
pub struct KeyTargetRateSampler<F> {
    key: F,
    target: u64,
    window: Duration,
    max_keys: usize,
    buckets: BTreeMap<String, Bucket>,
}
impl<F> KeyTargetRateSampler<F> {
    pub fn new(key: F, target: u64, window: Duration, max_keys: usize) -> Self {
        Self {
            key,
            target,
            window,
            max_keys,
            buckets: BTreeMap::new(),
        }
    }
}
impl<F: Fn(&SamplingContext) -> String> SamplingStrategy for KeyTargetRateSampler<F> {
    fn should_sample(&mut self, context: &SamplingContext) -> bool {
        let actual = (self.key)(context);
        let bucket_key = if self.buckets.contains_key(&actual) || self.buckets.len() < self.max_keys
        {
            actual
        } else {
            "__overflow__".into()
        };
        let bucket = self.buckets.entry(bucket_key).or_insert(Bucket {
            started: Instant::now(),
            count: 0,
            rate: 1.0,
        });
        if bucket.started.elapsed() >= self.window {
            bucket.rate = (bucket.count as f64 / self.target as f64).max(1.0);
            bucket.count = 0;
            bucket.started = Instant::now();
        }
        bucket.count += 1;
        rand::rng().random::<f64>() < 1.0 / bucket.rate
    }
    fn sample_rate(&self, context: &SamplingContext) -> f64 {
        self.buckets
            .get(&(self.key)(context))
            .map_or(1.0, |bucket| bucket.rate)
    }
}
