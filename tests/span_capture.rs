//! The in-memory span buffer is opt-in. One test: `init` is process-wide.

use autotel::{AutotelConfig, completed_spans, trace};

#[tokio::test(flavor = "current_thread")]
async fn spans_are_captured_only_when_asked() {
    let _runtime = autotel::init(AutotelConfig::new("no-capture")).unwrap();
    trace("production.span", |_| async {}).await;
    assert!(
        completed_spans().is_empty(),
        "production must not buffer spans"
    );

    let _runtime = autotel::init(AutotelConfig::new("capture").capture_spans()).unwrap();
    trace("test.span", |_| async {}).await;
    let names: Vec<_> = completed_spans()
        .into_iter()
        .map(|span| span.name)
        .collect();
    assert_eq!(names, ["test.span"]);
}
