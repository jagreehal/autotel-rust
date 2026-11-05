---
title: What You Can Record
description: Every signal autotel records, and which API to reach for.
---

- `init` sets up traces, metrics and logs over OTLP/HTTP, or OTLP/gRPC with the `grpc` feature. Per-signal URLs come from `.traces_endpoint` or the `OTEL_EXPORTER_OTLP_*` variables.
- `trace` and `trace_result` create spans whose context travels with the future, so concurrent and spawned work keeps the right parent. `trace_result` records an `Err` as a span error.
- `span_bridge()` puts `#[tracing::instrument]` spans into the same traces as `trace()`, parented both ways. `log_bridge()` links each `tracing` event to its span. `.with_tracing_subscriber()` installs both with a `RUST_LOG` filter that `set_log_filter` changes at runtime.
- W3C `traceparent` and `baggage` propagation: `inject_trace_context`, `extract_trace_context`, `inject_request`, `fetch`.
- Axum middleware (`axum` feature): `.layer(autotel::axum::layer())` joins the caller's trace and records SERVER spans with a bounded `http.route`, plus `url.path`, `url.scheme`, `server.address`, `user_agent.original` and `network.protocol.version`. `.skip_routes(["/health"])` leaves probes out, `.trace_response_header()` answers with `traceparent`, and `current_trace_id()` puts the id in an error body.
- gRPC server spans (`tonic` feature): `.layer(autotel::tonic::layer())` records `rpc.method` and `rpc.status_code` from the stable RPC conventions, with the span open for the whole stream, and `rpc.server.call.duration`.
- Rolling log files through `.log_file(LogFile::new("logs"))` or `AUTOTEL_LOG_FILE_DIR`.
- HTTP, database, messaging and workflow helpers set semconv attributes and record `http.server.request.duration`, `http.client.request.duration` and `db.client.operation.duration`. `ctx.set_query_text` records `db.query.text` with literals removed.
- SQLx helpers (`sqlx` feature): `with_transaction`, `acquire` and `register_pool_metrics`.
- `redact_pii` scrubs emails, card numbers, tokens and passwords from attributes, span events, logs and business events at export. `url.full` drops credentials and query values.
- Process-local tail sampling: `AdaptiveSampler::production()` keeps errors, slow traces and a baseline share of this service's spans, with `force_keep()`, `autotel.debug` baggage and `keep_when` rules. For one decision across services, use the Collector's `tail_sampling` processor.
- Wide events (`request_logger()`), `StructuredError`, and Tokio runtime metrics (`register_runtime_metrics()`).

## Also included

- `TraceCollector` for asserting on spans in tests, safe under `cargo test`'s parallel threads, and an embedded devtools receiver for local runs.
- GenAI spans and metrics (`gen_ai`, `GenAiMetrics`) following the OpenTelemetry GenAI conventions.
- Business events with subscribers (`track`, `FileSubscriber`).
- Cohort comparison, SLO trackers, burn-rate alerts and forecasting over recorded spans.

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
