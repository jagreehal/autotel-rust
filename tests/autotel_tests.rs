use autotel::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
async fn trace_collector_records_hierarchy() {
    let collector = TraceCollector::new("tests").unwrap();
    trace("parent", |_| async {
        trace("child", |_| async {}).await;
    })
    .await;
    let parent = collector.expect_span("parent");
    assert_eq!(collector.descendants(&parent.span_id).len(), 1);
}

#[test]
fn deterministic_trace_ids_are_stable() {
    assert_eq!(
        deterministic_trace_id("order"),
        deterministic_trace_id("order")
    );
}

#[test]
fn metadata_is_flattened() {
    let input = Attributes::from([("user".into(), json!({"id": "42"}))]);
    assert_eq!(flatten_metadata(&input)["metadata.user.id"], json!("42"));
}

#[test]
fn cohort_analysis_finds_separator() {
    let event = |provider: &str, duration: i64| {
        Attributes::from([
            ("provider".into(), json!(provider)),
            ("duration".into(), json!(duration)),
        ])
    };
    let outliers: Vec<_> = (0..4).map(|i| event("beta", 900 + i)).collect();
    let baseline: Vec<_> = (0..4).map(|i| event("alpha", 300 + i)).collect();
    let top = &compare_cohorts(&outliers, &baseline)[0];
    // Durations never repeat, so they cannot describe a cohort and are skipped.
    assert_eq!(
        (top.field.as_str(), top.value.as_str()),
        ("provider", "beta")
    );
    assert_eq!(top.difference, 1.0);
}

#[test]
fn bucket_labels_ranges() {
    let bounds = [1000.0, 2000.0, 5000.0];
    assert_eq!(bucket(500.0, &bounds), "<1000");
    assert_eq!(bucket(1500.0, &bounds), "1000-2000");
    assert_eq!(bucket(5000.0, &bounds), ">=5000");
    assert_eq!(bucket(f64::NAN, &bounds), "unknown");
}

#[test]
fn slo_reports_burn() {
    let mut tracker = SloTracker::new("checkout", 0.99, Duration::from_secs(60));
    for _ in 0..90 {
        tracker.record(true);
    }
    for _ in 0..10 {
        tracker.record(false);
    }
    assert!(tracker.snapshot().burn_rate > 1.0);
}

#[test]
fn slo_forecast_detects_risk() {
    let mut tracker = SloTracker::new("checkout", 0.99, Duration::from_secs(60));
    tracker.record(true);
    assert!(tracker.forecast(100, 0.2).alerting);
}

#[test]
fn deterministic_sampling_is_consistent() {
    let key = |context: &SamplingContext| context.operation_name.clone();
    let mut first = DeterministicSampler::new(0.5, key);
    let mut second = DeterministicSampler::new(0.5, key);
    let context = SamplingContext::new("checkout");
    assert_eq!(
        first.should_sample(&context),
        second.should_sample(&context)
    );
}

#[test]
fn always_sampler_keeps_everything() {
    let mut sampler = AlwaysSampler;
    assert!(sampler.should_sample(&SamplingContext::new("anything")));
}

#[test]
fn adaptive_sampler_keeps_failures() {
    assert!(
        AdaptiveSampler::new(0.01, Duration::from_secs(1)).should_keep(
            "trace",
            false,
            Duration::ZERO
        )
    );
}

#[test]
fn structured_error_preserves_remediation() {
    let error = StructuredError::new("missing", 404, "not found", "check ID")
        .with_link("https://example.com");
    assert_eq!(error.status, 404);
    assert!(error.link.is_some());
}
