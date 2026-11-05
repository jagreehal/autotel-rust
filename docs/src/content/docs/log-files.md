---
title: Log Files
description: Write rolling log files alongside your OTLP export.
---

```rust
use autotel::{AutotelConfig, LogFile, LogRotation};

let config = AutotelConfig::new("orders")
    .log_file(LogFile::new("logs").rotation(LogRotation::HOURLY).max_files(24));
```

Files use the console format without colors, or JSON with `trace_id` after `.json_logs()`.

Writes happen on a background thread, which keeps disk latency out of requests but drops lines if the disk can't keep up. `.blocking()` trades the other way. The `AutotelRuntime` guard flushes them on drop.

Add `.without_console_logs()` to stop printing to stdout. Give each service its own directory, because pruning matches on the filename prefix.

To turn log files on with no code, set [`AUTOTEL_LOG_FILE_DIR`](/autotel-rust/configuration/).
