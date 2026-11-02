# autotel-rust

[![crates.io](https://img.shields.io/crates/v/autotel.svg)](https://crates.io/crates/autotel)
[![docs.rs](https://docs.rs/autotel/badge.svg)](https://docs.rs/autotel)
[![CI](https://github.com/jagreehal/autotel-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/jagreehal/autotel-rust/actions/workflows/ci.yml)

OpenTelemetry for Rust services. Call `init` once, wrap work in `trace`, and traces, metrics and logs leave over OTLP.

The [Rust OpenTelemetry Workshop](workshop/README.md) teaches it in thirteen parts, taking a bank's ATM from `println!` to one trace across three services. Every part runs against a real receiver in CI and checks what arrived.

## Service instrumentation

- `init` sets up traces, metrics and logs over OTLP/HTTP, or OTLP/gRPC with the `grpc` feature. Per-signal URLs come from `.traces_endpoint` or the `OTEL_EXPORTER_OTLP_*` variables.
- `trace` and `trace_result` create spans whose context travels with the future, so concurrent and spawned work keeps the right parent. `trace_result` records an `Err` as a span error.
- `span_bridge()` puts `#[tracing::instrument]` spans into the same traces as `trace()`, parented both ways. `log_bridge()` links each `tracing` event to its span. `.with_tracing_subscriber()` installs both with a `RUST_LOG` filter that `set_log_filter` changes at runtime.
- W3C `traceparent` and `baggage` propagation: `inject_trace_context`, `extract_trace_context`, `inject_request`, `fetch`.
- Axum middleware (`axum` feature): `.layer(autotel::axum::layer())` joins the caller's trace and records SERVER spans with a bounded `http.route`.
- HTTP, database, messaging and workflow helpers set semconv attributes and record `http.server.request.duration`, `http.client.request.duration` and `db.client.operation.duration`. `ctx.set_query_text` records `db.query.text` with literals removed.
- SQLx helpers (`sqlx` feature): `with_transaction`, `acquire` and `register_pool_metrics`.
- `redact_pii` scrubs emails, card numbers, tokens and passwords from attributes, span events, logs and business events at export. `url.full` drops credentials and query values.
- Process-local tail sampling: `AdaptiveSampler::production()` keeps errors, slow traces and a baseline share of this service's spans, with `force_keep()`, `autotel.debug` baggage and `keep_when` rules. For one decision across services, use the Collector's `tail_sampling` processor.
- Wide events (`request_logger()`), `StructuredError`, and Tokio runtime metrics (`register_runtime_metrics()`).

## Also included

- `TraceCollector` for asserting on spans in tests, and an embedded devtools receiver for local runs.
- GenAI spans and metrics (`gen_ai`, `GenAiMetrics`) following the OpenTelemetry GenAI conventions.
- Business events with subscribers (`track`, `FileSubscriber`).
- Cohort comparison, SLO trackers, burn-rate alerts and forecasting over recorded spans.

## Install

```sh
cargo add autotel
# optional: --features axum,grpc,sqlx
```

## Quick start

```rust
use autotel::{init, trace, AutotelConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _runtime = init(AutotelConfig::new("orders-service"))?;
    let order = trace("order.create", |context| async move {
        context.set_attribute("order.id", "ord_42");
        "created"
    }).await;
    assert_eq!(order, "created");
    Ok(())
}
```

## Agent skill

Install the [`autotel-rust`](skills/autotel-rust/SKILL.md) skill so your coding assistant uses the crate's API:

```sh
npx skills add jagreehal/autotel-rust --skill autotel-rust
```

## Verification

Open the repository in its Rust devcontainer, then run `make quality`. Run one workshop part with `cargo sol 10` (start `npx autotel-devtools` first to see its telemetry), or check every part against a throwaway receiver with `make workshop`. The workshop check needs Node 22 or newer for `autotel-devtools`.

## Releasing

Bump `version` in `Cargo.toml`, move the changelog entries under the new version, then push a tag `v<version>`. The `Release` workflow checks the tag matches, runs the tests and publishes to crates.io (needs the `CARGO_REGISTRY_TOKEN` secret in the `main` environment).
