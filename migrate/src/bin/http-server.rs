//! autotel version of `tracing-http-propagator`'s server. Run it, then `http-client`.
use axum::{Router, routing::get};

async fn health_check() -> &'static str {
    autotel::trace("health_check", |_| async {
        tracing::info!(name: "health_check", message = "Health check endpoint hit");
        "Server is up and running!"
    })
    .await
}

async fn echo(body: String) -> String {
    autotel::trace("echo", |_| async move {
        tracing::info!(name: "echo", message = "Echo endpoint hit");
        body
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `.baggage("")` copies the caller's baggage (`is_synthetic=true`) onto spans,
    // the job of the original's EnrichWithBaggageSpanProcessor.
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber().baggage(""))?;

    // The layer extracts `traceparent`/`baggage` and starts a SERVER span with
    // `http.route` and the status code: the original's `router` function.
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/echo", get(echo))
        .layer(autotel::axum::layer());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(autotel::shutdown_signal())
        .await?;
    Ok(())
}
