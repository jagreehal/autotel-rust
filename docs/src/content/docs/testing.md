---
title: Testing
description: Assert on the spans your code records.
---

`TraceCollector` records spans so a test can assert on them:

```rust
let collector = autotel::TraceCollector::new("test")?;
run_code_under_test().await;
let span = collector.expect_span("order.create");
assert_eq!(span.attributes["order.id"], serde_json::json!("ord_42"));
```

`TraceCollector` turns on span capture. Production configs leave `capture_spans` off.

## Parallel tests

Tests that use a collector run one at a time, so they need no lock of their own, and each sees only its own spans. `new` waits for the open collector to drop.

Keep to one per test: a second on the same thread returns an error.
