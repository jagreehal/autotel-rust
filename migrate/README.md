# Migrating from the OpenTelemetry SDK

The [opentelemetry-rust examples](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples), rewritten with autotel. Open each original
next to its rewrite to see what the SDK wiring turns into. The links are pinned to
upstream commit `83e9926`.

| Original | autotel version | Lines of code (original → autotel) |
| --- | --- | --- |
| [logs-basic](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/logs-basic) | [`logs`](src/bin/logs.rs) | 32 → 5 |
| [metrics-basic](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/metrics-basic) | [`metrics`](src/bin/metrics.rs) | 108 → 21 |
| [tracing-http-propagator](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/tracing-http-propagator) | [`http-server`](src/bin/http-server.rs), [`http-client`](src/bin/http-client.rs) | 336 → 55 |
| [tracing-grpc](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/tracing-grpc) | [`grpc-server`](src/bin/grpc-server.rs), [`grpc-client`](src/bin/grpc-client.rs) | 158 → 52 |

The counts leave out comments and blank lines.

## Run them

From this folder:

```sh
cargo run --bin logs
cargo run --bin metrics
cargo run --bin http-server   # then, in another terminal: cargo run --bin http-client
cargo run --bin grpc-server   # then, in another terminal: cargo run --bin grpc-client
```

[`.cargo/config.toml`](.cargo/config.toml) sets `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=console`,
so every signal prints to stdout like the stdout exporter in the originals. This needs
autotel's `console` feature. To send to a backend instead:

```sh
OTEL_TRACES_EXPORTER=otlp OTEL_METRICS_EXPORTER=otlp OTEL_LOGS_EXPORTER=otlp \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 cargo run --bin logs
```

## What replaces what

| In the original | In the autotel version |
| --- | --- |
| `SdkTracerProvider`, `SdkMeterProvider`, `SdkLoggerProvider`, `Resource`, `global::set_*` | `autotel::init` |
| `OpenTelemetryTracingBridge`, `EnvFilter`, fmt layer | `.with_tracing_subscriber()` (filter from `RUST_LOG`) |
| `provider.shutdown()` on every exit path | The guard `init` returns |
| `TraceContextPropagator`, `BaggagePropagator`, `HeaderInjector`/`HeaderExtractor` | Built in: `autotel::fetch`, `autotel::inject_trace_context` and the server layers |
| A hyper `router` that extracts context and starts a SERVER span with `http.route` | `.layer(autotel::axum::layer())` |
| `EnrichWithBaggageSpanProcessor` | `.baggage("")` |
| tonic `Extractor`/`Injector` impls for `MetadataMap`, and a client span opened by hand | `.layer(autotel::tonic::layer())` on the server, `autotel::tonic::client(channel)` on the client |

## Not carried over

- **logs-advanced** and **metrics-advanced** show custom `LogProcessor`s and metric
  Views. Those are SDK extension points, so they stay plain OpenTelemetry code.
- `tracing-http-propagator`'s server includes a `RouteConcurrencyCounterSpanProcessor`
  and its unit test to demonstrate a custom processor. The rewrite leaves them out, which
  accounts for most of that row's line difference.

`build.rs` compiles `proto/helloworld.proto`, a copy of the one in `tracing-grpc`,
with a bundled `protoc`. The proto file is Apache-2.0, from the OpenTelemetry Authors.
