//! Span throughput under concurrency. Ignored by default (timing-sensitive); run with
//! `cargo test --release --test throughput -- --ignored --nocapture`.
//!
//! Guards against a process-wide lock creeping back onto the span path: with one,
//! more tasks make `trace()` slower overall instead of faster.

use autotel::{AutotelConfig, trace};
use std::time::Instant;

const SPANS_PER_TASK: usize = 20_000;

async fn spans_per_second(tasks: usize) -> f64 {
    let started = Instant::now();
    let handles: Vec<_> = (0..tasks)
        .map(|_| {
            tokio::spawn(async {
                for i in 0..SPANS_PER_TASK {
                    trace("op", |ctx| async move {
                        for key in ["a", "b", "c", "d", "e"] {
                            ctx.set_attribute(key, i);
                        }
                    })
                    .await;
                }
            })
        })
        .collect();
    for handle in handles {
        handle.await.unwrap();
    }
    (tasks * SPANS_PER_TASK) as f64 / started.elapsed().as_secs_f64()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "timing-sensitive; run in release"]
async fn throughput_scales_with_tasks() {
    let _runtime = autotel::init(AutotelConfig::new("throughput")).unwrap();
    spans_per_second(1).await; // warm up
    let single = spans_per_second(1).await;
    let parallel = spans_per_second(8).await;
    println!("1 task: {single:.0} spans/s, 8 tasks: {parallel:.0} spans/s");
    assert!(
        parallel > single * 1.5,
        "8 tasks ({parallel:.0}/s) should beat 1 task ({single:.0}/s): a global lock is back"
    );
}
