---
title: Introduction
description: The setup autotel replaces, and what you get from one line.
---

`autotel` is ergonomic OpenTelemetry for Rust. One line sends traces, metrics and logs out over OTLP.

```rust
let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
```

That line replaces the 60 lines of provider, exporter, propagator and subscriber wiring a Rust service otherwise starts with. Your existing `tracing` logs and `#[instrument]` spans join the same traces, and the service name and version come from your `Cargo.toml`.

## The setup you skip

| | Wired by hand | autotel |
| --- | --- | --- |
| Crates to add and keep on matching versions | 6 | 1 |
| Non-blank lines after `rustfmt` | 66 | 6 |
| Lines to trace an axum or tonic server | Your own middleware | 1 |

The [home page](/autotel-rust/) shows the 66 lines next to the 6. Each row below is code you leave out:

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

Deployments change behaviour through the [standard `OTEL_*` variables](/autotel-rust/configuration/), with no code change.

## Links

- [Quick start](/autotel-rust/quick-start/)
- [autotel on crates.io](https://crates.io/crates/autotel)
- [API reference on docs.rs](https://docs.rs/autotel)
