---
title: Axum
description: Trace an axum server with one layer.
---

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

`http.route` works on nested routers too. Run it from the repository with `cargo run --example axum --features axum`.

## Layer options

| Option | Effect |
| --- | --- |
| `.skip_routes(["/health"])` | Leaves probes out of traces. |
| `.trace_response_header()` | Answers with `traceparent`. |
| `.raw_query()` | Records query values as sent. |
| `.trust_forwarded_for()` | Takes `client.address` from `X-Forwarded-For`. |

`current_trace_id()` puts the trace id in an error body, so a customer can quote it in a support ticket.

## What the span records about the caller

`url.query` keeps its keys and hides its values (`token=REDACTED&expand=REDACTED`), because query strings carry tokens and search terms. `.raw_query()` records them as sent.

`client.address` is the TCP peer, which needs `into_make_service_with_connect_info::<SocketAddr>()`. Behind a proxy that sets `X-Forwarded-For`, add `.trust_forwarded_for()` to use its first address. Keep it to that setup, because any client can send the header. `redact_pii` drops both IP attributes.
