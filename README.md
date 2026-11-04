# autotel-rust

[![crates.io](https://img.shields.io/crates/v/autotel.svg)](https://crates.io/crates/autotel)
[![docs.rs](https://docs.rs/autotel/badge.svg)](https://docs.rs/autotel)
[![CI](https://github.com/jagreehal/autotel-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/jagreehal/autotel-rust/actions/workflows/ci.yml)

Write Once, Observe Everywhere: ergonomic OpenTelemetry for Rust.

**One line of setup. Traces, metrics and logs out over OTLP.**

```rust
let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
```

That line replaces the 60 lines of provider, exporter, propagator and subscriber wiring a Rust service otherwise starts with. Your existing `tracing` logs and `#[instrument]` spans join the same traces, and the service name and version come from your `Cargo.toml`.

## The setup you skip

| | Wired by hand | autotel |
| --- | --- | --- |
| Crates to add and keep on matching versions | 6 | 1 |
| Lines from `use` to `Ok(())`, after `rustfmt` | 66 | 6 |
| Lines to trace an axum or tonic server | Your own middleware | 1 |

<details>
<summary>The 66 lines</summary>

This compiles against the same crate versions autotel uses, and covers traces, metrics, logs, propagation and the `tracing` bridges. It has no redaction, sampling or server middleware yet.

```rust
use opentelemetry::{global, propagation::TextMapCompositePropagator, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::SdkMeterProvider,
    propagation::{BaggagePropagator, TraceContextPropagator},
    trace::SdkTracerProvider,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let resource = Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .with_attribute(opentelemetry::KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            SpanExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpJson)
                .build()?,
        )
        .build();
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_periodic_exporter(
            MetricExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpJson)
                .build()?,
        )
        .build();
    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(
            LogExporter::builder()
                .with_http()
                .with_protocol(Protocol::HttpJson)
                .build()?,
        )
        .build();

    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());
    global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
        Box::new(TraceContextPropagator::new()),
        Box::new(BaggagePropagator::new()),
    ]));

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("orders")))
        .with(OpenTelemetryTracingBridge::new(&logger_provider))
        .init();

    // your service runs here

    tracer_provider.force_flush()?;
    tracer_provider.shutdown()?;
    meter_provider.shutdown()?;
    logger_provider.shutdown()?;
    Ok(())
}
```

</details>

With autotel, the same service reads:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
    // your service runs here
    Ok(())
}
```

Each row below is code you leave out:

| Instead of writing | You get |
| --- | --- |
| Three exporters and three providers | `init` |
| A `Resource` with your name and version | `autotel::config!()` reads `Cargo.toml` |
| Global provider registration | `init` |
| W3C propagators for `traceparent` and `baggage` | Built into `fetch`, `inject_trace_context` and the server layers |
| A subscriber with an OpenTelemetry layer and log bridge | `.with_tracing_subscriber()` |
| Flush and shutdown calls on every exit path | The guard flushes on drop |
| SIGTERM handling | `autotel::shutdown_signal()` |
| HTTP middleware for context, spans, routes and duration metrics | `.layer(autotel::axum::layer())` |
| A body wrapper that holds a gRPC span open until the trailers | `.layer(autotel::tonic::layer())` |
| A PII scrubber across spans, logs and events | `redact_pii: true` |

Deployments change behaviour through the [standard `OTEL_*` variables](#configuration), with no code change.

## Quick start

```sh
cargo add autotel anyhow tracing
cargo add tokio --features macros,rt-multi-thread
# optional autotel features: axum, tonic, grpc, sqlx
```

```rust
use autotel::{config, init, trace};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = init(config!().with_tracing_subscriber())?;

    trace("order.create", |ctx| async move {
        ctx.set_attribute("order.id", "ord_42");
        tracing::info!("order created");
    })
    .await;
    Ok(())
}
```

`cargo run` tells you what to do next:

```text
INFO autotel: orders: no OTLP endpoint, telemetry stays local. Set OTEL_EXPORTER_OTLP_ENDPOINT to export
INFO orders: order created
```

Point it at any OTLP/HTTP backend. `npx autotel-devtools` gives you one on your machine, and `docker compose up -d` starts Jaeger from this repository.

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 cargo run
```

```text
INFO autotel: orders: exporting traces to http://localhost:4318/v1/traces, metrics to http://localhost:4318/v1/metrics, logs to http://localhost:4318/v1/logs over http/json
INFO orders: order created
```

The [Rust OpenTelemetry Workshop](workshop/README.md) teaches it in thirteen parts, taking a bank's ATM from `println!` to one trace across three services. Every part runs against a real receiver in CI and checks what arrived.

## Axum

```toml
autotel = { version = "0.2", features = ["axum"] }
```

```rust
use axum::{Router, routing::get};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Named and versioned from your Cargo.toml. Keep the guard: it flushes on drop.
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;

    let app = Router::new()
        .route("/orders/{id}", get(|| async { "order" }))
        .layer(autotel::axum::layer());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    // Ctrl-C or SIGTERM, then the guard sends the last batch.
    axum::serve(listener, app).with_graceful_shutdown(autotel::shutdown_signal()).await?;
    Ok(())
}
```

`http.route` works on nested routers too. Run it with `cargo run --example axum --features axum`.

### What the span records about the caller

`url.query` keeps its keys and hides its values (`token=REDACTED&expand=REDACTED`), because query strings carry tokens and search terms. `.raw_query()` records them as sent.

`client.address` is the TCP peer, which needs `into_make_service_with_connect_info::<SocketAddr>()`. Behind a proxy that sets `X-Forwarded-For`, add `.trust_forwarded_for()` to use its first address. Keep it to that setup, because any client can send the header. `redact_pii` drops both IP attributes.

## gRPC

```toml
autotel = { version = "0.2", features = ["tonic"] }
```

```rust,ignore
tonic::transport::Server::builder()
    .layer(autotel::tonic::layer())
    .add_service(service)
    .serve(address)
    .await?;
```

Each call gets a SERVER span named `package.Service/Method`, joined to the caller's trace. The span ends with the response body, because a successful call reports its status in trailers, so a streaming call lasts as long as its stream and a client that hangs up reads as `CANCELLED`. `rpc.server.call.duration` measures the same span. Only codes that are the server's fault (`INTERNAL`, `UNAVAILABLE` and so on) mark the span as an error and set `error.type`. An unknown method becomes `rpc.method: _OTHER` on a span named `grpc`, and so does a call a router or middleware answers with plain HTTP. If auth or rate-limit layers answer before tonic routes a call, list your methods with `.methods(["package.Service/Method"])` to keep made-up paths out of span names and metric series.

`client.address` works over plain TCP and TLS, including a TLS stream you accept yourself. Pair autotel with tonic's `tls-aws-lc` feature, which matches its HTTP client. For `tls-ring`, install a provider with `CryptoProvider::install_default`. To serve tonic through an axum router, put the layer on the router and serve it with `into_make_service_with_connect_info::<SocketAddr>()`. Run `cargo run --example tonic --features tonic` to see one trace across a client and server.

The `grpc` feature is separate: it exports telemetry over OTLP/gRPC.

## Log files

```rust
use autotel::{AutotelConfig, LogFile, LogRotation};

let config = AutotelConfig::new("orders")
    .log_file(LogFile::new("logs").rotation(LogRotation::HOURLY).max_files(24));
```

Files use the console format without colors, or JSON with `trace_id` after `.json_logs()`. Writes happen on a background thread, which keeps disk latency out of requests but drops lines if the disk can't keep up; `.blocking()` trades the other way. The `AutotelRuntime` guard flushes them on drop. Add `.without_console_logs()` to stop printing to stdout. Give each service its own directory, because pruning matches on the filename prefix.

## Configuration

Set the standard variables and change no code. They beat the values in `AutotelConfig`.

| Variable | Effect |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Turns export on. `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT` override one signal. |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/json` (default), `http/protobuf`, or `grpc` with the `grpc` feature. Anything else fails `init`. |
| `OTEL_EXPORTER_OTLP_HEADERS` | Credentials, as `key=value,key=value`. |
| `OTEL_SERVICE_NAME` | `service.name`. |
| `OTEL_RESOURCE_ATTRIBUTES` | Any resource attribute, such as `service.version=1.2,deployment.environment.name=prod`. |
| `OTEL_TRACES_SAMPLER` | Head sampling, such as `parentbased_traceidratio`. |
| `OTEL_{TRACES,METRICS,LOGS}_EXPORTER` | `none` silences one signal, for backends like Jaeger that take only traces. |
| `RUST_LOG` | Log filter for `.with_tracing_subscriber()`. |
| `AUTOTEL_LOG_FILE_DIR` | Turns on log files in this directory, no code needed. |
| `AUTOTEL_LOG_FILE_ROTATION` | `minutely`, `hourly`, `daily` (default) or `never`. |
| `AUTOTEL_LOG_FILE_MAX_FILES` | Files to keep: 2 or more, or 0 for all. Default 7. |

`init` logs where telemetry is going, or that it stays local, so a missing endpoint shows at startup. For a hosted backend such as Grafana Cloud:

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=https://otlp-gateway-<region>.grafana.net/otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic%20<base64 instance:token>"
```

Use `.json_logs()` in place of `.with_tracing_subscriber()` to print JSON lines with `trace_id` and `span_id`.

## What you can record

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

- `TraceCollector` for asserting on spans in tests, and an embedded devtools receiver for local runs.
- GenAI spans and metrics (`gen_ai`, `GenAiMetrics`) following the OpenTelemetry GenAI conventions.
- Business events with subscribers (`track`, `FileSubscriber`).
- Cohort comparison, SLO trackers, burn-rate alerts and forecasting over recorded spans.

## Agent skill

Install the [`autotel-rust`](skills/autotel-rust/SKILL.md) skill so your coding assistant uses the crate's API:

```sh
npx skills add jagreehal/autotel-rust --skill autotel-rust
```

## Verification

Open the repository in its Rust devcontainer, then run `make quality`. Run one workshop part with `cargo sol 10` (start `npx autotel-devtools` first to see its telemetry), or check every part against a throwaway receiver with `make workshop`. The workshop check needs Node 22 or newer for `autotel-devtools`.

## Releasing

Bump `version` in `Cargo.toml`, move the changelog entries under the new version, then push a tag `v<version>`. The `Release` workflow checks the tag matches, runs the tests, publishes to crates.io (needs the `CARGO_REGISTRY_TOKEN` secret in the `main` environment) and creates a GitHub release from that changelog section.
