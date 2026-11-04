//! `TraceCollector` in tests that run at once, as `cargo test` runs them.

use autotel::{TraceCollector, trace};
use std::thread;

#[test]
fn collectors_on_parallel_threads_each_keep_their_spans() {
    let threads: Vec<_> = (0..8)
        .map(|index| {
            thread::spawn(move || {
                for round in 0..25 {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .build()
                        .unwrap();
                    let collector = TraceCollector::new("parallel").unwrap();
                    let name = format!("work-{index}-{round}");
                    runtime.block_on(trace(name.clone(), |_| async {}));
                    assert_eq!(collector.by_name(&name).len(), 1);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn a_second_collector_on_one_thread_is_an_error() {
    let _first = TraceCollector::new("first").unwrap();
    let error = TraceCollector::new("second").err().unwrap().to_string();
    assert!(error.contains("already open"), "{error}");
}

#[test]
fn a_collector_can_cross_threads() {
    fn is_send<T: Send>(_: &T) {}
    let collector = TraceCollector::new("send").unwrap();
    is_send(&collector);
}
