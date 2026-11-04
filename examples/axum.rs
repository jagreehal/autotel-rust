//! `cargo run --example axum --features axum`, then `curl localhost:3000/orders/42`.
use axum::{Router, extract::Path, routing::get};

async fn order(Path(id): Path<u32>) -> String {
    tracing::info!(id, "loading order");
    format!("order {id}")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
    let app = Router::new()
        .route("/orders/{id}", get(order))
        .route("/health", get(|| async { "ok" }))
        .layer(
            autotel::axum::layer()
                .skip_routes(["/health"])
                .trace_response_header(),
        );
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    // Connect info is what fills `client.address`.
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(autotel::shutdown_signal())
        .await?;
    Ok(())
}
