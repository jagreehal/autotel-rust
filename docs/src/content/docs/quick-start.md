---
title: Quick Start
description: Add autotel to a Rust service and see where its telemetry goes.
---

## Install

Add the [`autotel` crate](https://crates.io/crates/autotel):

```sh
cargo add autotel anyhow tracing
cargo add tokio --features macros,rt-multi-thread
# optional autotel features: axum, tonic, grpc, sqlx
```

## Trace some work

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

Keep the guard in a named variable. It flushes on drop, and `let _ = init(...)` drops it on the spot.

## Run it

`cargo run` tells you what to do next:

```text
INFO autotel: orders: no OTLP endpoint, telemetry stays local. Set OTEL_EXPORTER_OTLP_ENDPOINT to export
INFO orders: order created
```

## Export

Point it at any OTLP/HTTP backend. `npx autotel-devtools` gives you one on your machine, and `docker compose up -d` starts Jaeger from the autotel repository.

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 cargo run
```

```text
INFO autotel: orders: exporting traces to http://localhost:4318/v1/traces, metrics to http://localhost:4318/v1/metrics, logs to http://localhost:4318/v1/logs over http/json
INFO orders: order created
```

## Next steps

- [Trace an axum server](/autotel-rust/servers/axum/) or a [gRPC server](/autotel-rust/servers/grpc/)
- [Configure export](/autotel-rust/configuration/) for your backend
- [Work through the workshop](/autotel-rust/workshop/)
