use autotel::{TraceCollector, trace};
use tokio::task::yield_now;

async fn job(name: &'static str) {
    trace(name, |_| async {
        yield_now().await;
        trace(format!("{name}.child"), |_| async { yield_now().await }).await;
    })
    .await;
}

// One test per binary: `init` resets the shared span buffer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_and_spawned_traces_keep_their_parents() {
    let collector = TraceCollector::new("concurrency").unwrap();
    tokio::join!(job("a"), job("b"));
    // Spawning needs the traced future to be Send.
    tokio::spawn(job("c")).await.unwrap();

    for name in ["a", "b", "c"] {
        let parent = collector.expect_span(name);
        let child = collector.expect_span(&format!("{name}.child"));
        assert_eq!(parent.parent_span_id, None, "{name} should be a root");
        assert_eq!(
            child.parent_span_id.as_deref(),
            Some(parent.span_id.as_str())
        );
        assert_eq!(child.trace_id, parent.trace_id);
    }
    assert_ne!(
        collector.expect_span("a").trace_id,
        collector.expect_span("b").trace_id
    );
}
