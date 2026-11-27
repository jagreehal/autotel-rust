//! autotel version of `tracing-http-propagator`'s client.
use axum::http::Method;

async fn send_request(url: &str, body: &'static str, span_name: &str) -> anyhow::Result<()> {
    autotel::trace(span_name, |ctx| async move {
        ctx.set_baggage("is_synthetic", "true");
        // `fetch` starts a CLIENT span and injects `traceparent` and `baggage`.
        let response = autotel::fetch(Method::GET, url, |request| request.body(body)).await?;
        tracing::info!(name: "ResponseReceived", status = response.status().to_string(), message = "Response received");
        Ok(())
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
    send_request(
        "http://127.0.0.1:3000/health",
        "Health Request!",
        "server_health_check",
    )
    .await?;
    send_request(
        "http://127.0.0.1:3000/echo",
        "Echo Request!",
        "server_echo_check",
    )
    .await?;
    Ok(())
}
