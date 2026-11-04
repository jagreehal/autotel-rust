---
name: autotel-rust
description: >
  Instruments Rust services with the autotel crate: init, trace spans, tracing
  bridges, HTTP/database/GenAI helpers, wide events, tail sampling and PII
  redaction. Use when adding OpenTelemetry to a Rust service, calling
  autotel::init, choosing between autotel APIs, or testing emitted spans.
---

# autotel for Rust

`autotel` wraps the OpenTelemetry SDK. Call `init` once, wrap work in `trace`, and traces, metrics and logs go out over OTLP.

Add `autotel` as the only telemetry dependency. `init` builds the exporters, providers and `tracing` bridges, and W3C propagation is built in, so skip `opentelemetry_sdk`, `opentelemetry-otlp` and `tracing-opentelemetry` setup code.

## Setup

```toml
[dependencies]
autotel = { version = "0.2", features = ["axum"] } # optional: axum, tonic, grpc, sqlx
```

```rust
use autotel::{AdaptiveSampler, init};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = init(
        autotel::config!() // name and version from Cargo.toml
            .with_tracing_subscriber()
            .sampler(AdaptiveSampler::production()),
    )?;
    // run the app with `.with_graceful_shutdown(autotel::shutdown_signal())`;
    // dropping `_telemetry` at the end of main flushes and shuts down
    Ok(())
}
```

- Keep the guard alive. `let _ = init(...)` drops it on the spot and loses every span.
- `OTEL_*` variables beat code: `OTEL_EXPORTER_OTLP_ENDPOINT`, `_PROTOCOL` (`http/json`, `http/protobuf`, `grpc`), `_HEADERS`, `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none`. `init` logs where telemetry goes. `.devtools()` sends to `autotel-devtools` on `127.0.0.1:4318` (`npx autotel-devtools`).
- `.json_logs()` prints JSON lines with `trace_id`. `.with_tracing_subscriber()` installs fmt + `span_bridge()` + `log_bridge()` with a `RUST_LOG` filter. Leave it off if your app builds its own subscriber, and add `span_bridge()` and `log_bridge()` to that one.
- Set `redact_pii: true` on the config to scrub emails, card numbers, tokens and passwords at export.

## Which API

| Need | API |
| --- | --- |
| Span around async work | `trace("name", \|ctx\| async move { .. }).await` |
| Span that records `Err` as an error | `trace_result("name", \|ctx\| async move { .. }).await` |
| Set a span kind | `trace_kind("name", SpanKind::Consumer, \|ctx\| ..)` |
| Attributes and events | `ctx.set_attribute(k, v)`, `ctx.set_attributes(..)`, `ctx.add_event(..)` |
| Current span from anywhere | `autotel::ctx()` or `current_context()` |
| `#[instrument]` spans in the same trace | `span_bridge()` layer |
| Logs linked to spans | `log_bridge()` layer; `JsonWithTraceIds` for stdout |
| Incoming HTTP | `.layer(autotel::axum::layer())` or `http_server(method, route, \|ctx\| ..)` |
| Incoming gRPC (`tonic` feature) | `Server::builder().layer(autotel::tonic::layer())`; behind axum, on the router with connect info |
| Recognised gRPC methods | `autotel::tonic::layer().methods(["package.Service/Method"])` when auth runs before routing |
| Caller's IP | `into_make_service_with_connect_info`; `.trust_forwarded_for()` only behind a proxy |
| Query values on spans | Redacted by default; `.raw_query()` records them as sent |
| Log files | `.log_file(LogFile::new("logs"))` or `AUTOTEL_LOG_FILE_DIR` |
| Keep probes out of traces | `layer().skip_routes(["/health"])` |
| Trace id for callers | `layer().trace_response_header()` or `current_trace_id()` in the body |
| Outgoing HTTP with propagation | `fetch(Method::GET, url, \|req\| req)` or `http_client` + `inject_request` |
| Propagate by hand | `inject_trace_context`, `extract_trace_context` |
| Database call | `database(system, operation, db, collection, \|ctx\| ..)`; `ctx.set_query_text(sql)` |
| SQLx (`sqlx` feature) | `with_transaction`, `acquire`, `register_pool_metrics` |
| Queues and workflows | `produce`, `consume`, `workflow`, `step` |
| LLM calls | `gen_ai(GenAiOptions, \|ctx\| ..)`, `record_gen_ai_usage`, `GenAiMetrics` |
| One wide event per request | `request_logger()` then `.set(json!({..}))` and `.emit_now()` |
| Errors with context for humans | `StructuredError::new(message, status, why, fix)` |
| Business events | `track(name, attrs)` with `.subscriber(FileSubscriber::new(path)?)` |
| Baggage for child spans | `with_baggage(map, \|\| async { .. })`; `.baggage("app.")` copies it onto spans |
| Keep one trace | `ctx.force_keep()` or `autotel::force_keep()` |
| Metrics | `counter`, `histogram`, `up_down_counter`, `register_runtime_metrics()` |
| Change log level at runtime | `set_log_filter("debug")` |

## Spawned tasks

The span context travels with the future. `tokio::spawn` starts a new task without it, so attach it:

```rust
use autotel::FutureExt;
use opentelemetry::Context;

tokio::spawn(work().with_context(Context::current()));
```

## Tail sampling

Sampling is process-local: each service decides on its own spans. For one decision across services, run the Collector's `tail_sampling` processor.

`AdaptiveSampler::production()` keeps errors, traces over one second and 10% of the rest. `errors_only()` drops the baseline. Add rules with `.keep_when("http.response.status_code", |v| ..)`. A request with `autotel.debug=1` baggage or a `force_keep()` call keeps its trace.

## Testing

```rust
let collector = autotel::TraceCollector::new("test")?;
run_code_under_test().await;
let span = collector.expect_span("order.create");
assert_eq!(span.attributes["order.id"], serde_json::json!("ord_42"));
```

`TraceCollector` turns on span capture. Production configs leave `capture_spans` off.
