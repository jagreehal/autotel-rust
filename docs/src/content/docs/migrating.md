---
title: Migrating from the SDK
description: The opentelemetry-rust examples, rewritten with autotel, and what each piece of SDK wiring becomes.
---

If your service already sets up OpenTelemetry by hand, most of that code becomes one call to `init`. The [`migrate`](https://github.com/jagreehal/autotel-rust/tree/main/migrate) folder rewrites the [official opentelemetry-rust examples](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples) with autotel, so you can open each original next to its rewrite.

| Original | autotel version | Lines of code |
| --- | --- | --- |
| [logs-basic](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/logs-basic) | [`logs`](https://github.com/jagreehal/autotel-rust/tree/main/migrate/src/bin/logs.rs) | 32 → 5 |
| [metrics-basic](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/metrics-basic) | [`metrics`](https://github.com/jagreehal/autotel-rust/tree/main/migrate/src/bin/metrics.rs) | 108 → 21 |
| [tracing-http-propagator](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/tracing-http-propagator) | [`http-server`](https://github.com/jagreehal/autotel-rust/tree/main/migrate/src/bin/http-server.rs), [`http-client`](https://github.com/jagreehal/autotel-rust/tree/main/migrate/src/bin/http-client.rs) | 336 → 55 |
| [tracing-grpc](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/tracing-grpc) | [`grpc-server`](https://github.com/jagreehal/autotel-rust/tree/main/migrate/src/bin/grpc-server.rs), [`grpc-client`](https://github.com/jagreehal/autotel-rust/tree/main/migrate/src/bin/grpc-client.rs) | 158 → 52 |

## Setup

The providers, resource, global registration, log bridge and filters:

```rust
let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
```

Remove every `provider.shutdown()` call too. The guard flushes and shuts each provider down when it drops, on every exit path.

## What replaces what

| You wrote | Use instead |
| --- | --- |
| `SdkTracerProvider`, `SdkMeterProvider`, `SdkLoggerProvider`, `Resource`, `global::set_*` | `autotel::init` |
| `OpenTelemetryTracingBridge`, `EnvFilter`, fmt layer | `.with_tracing_subscriber()`, filtered by `RUST_LOG` |
| `TraceContextPropagator` and `BaggagePropagator` | Built in |
| `HeaderInjector` on an outgoing request | `autotel::fetch`, or `inject_trace_context()` for headers |
| `HeaderExtractor` and a SERVER span in your router | `.layer(autotel::axum::layer())` |
| tonic `Extractor`/`Injector` impls for `MetadataMap`, and a client span opened by hand | `.layer(autotel::tonic::layer())` on the server, `autotel::tonic::client(channel)` on the client |
| A span processor that copies baggage onto spans | `.baggage("")` |
| `tracer.span_builder(..).start(..)` | `autotel::trace("name", \|ctx\| async { .. })` |
| `meter.u64_counter(..).build().add(..)` | `autotel::counter_with("name", 1, &attributes)`, and likewise `histogram_with`, `up_down_counter_with`, `gauge_with` |
| `meter.f64_observable_gauge(..).with_callback(..)` | `autotel::observe_gauge("name", "unit", &attributes, \|\| value)`, and `observe_counter`, `observe_up_down_counter` |
| `opentelemetry_stdout` exporters | `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=console` with the `console` feature |

## What stays as SDK code

Custom `SpanProcessor`s, `LogProcessor`s and metric Views are extension points of the SDK, so the rewrites leave [logs-advanced](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/logs-advanced) and [metrics-advanced](https://github.com/open-telemetry/opentelemetry-rust/tree/83e99267b22ac9ee1605eb5d1342c3b820f3bf51/examples/metrics-advanced) out. The plain OpenTelemetry API keeps working after `init`, so any `global::meter` or `global::tracer` call you keep reports through the same pipeline.
