//! The auth service. Its own process, its own init(), its own service name.

#[path = "../harness.rs"]
mod harness;

use anyhow::Context;
use autotel::{AutotelConfig, trace_result};
use axum::{
    Json, Router,
    response::Response,
    routing::{get, post},
};
use bank::{find_user_by_card, verify_pin};
use harness::CardReply;
use serde::Deserialize;

/// The card number and PIN travel in the body rather than the URL, because
/// HTTP instrumentation records the full URL on every span.
#[derive(Deserialize)]
struct CardBody {
    #[serde(default)]
    card: String,
    #[serde(default)]
    pin: String,
}

fn last4(card: &str) -> &str {
    &card[card.len().saturating_sub(4)..]
}

async fn card(Json(body): Json<CardBody>) -> Response {
    harness::reply(trace_result("auth.find_card", |ctx| async move {
        println!("[auth-service]     trace {}", ctx.trace_id());
        ctx.set_attribute("card.last4", last4(&body.card));
        let user = find_user_by_card(&body.card).await?;

        Ok(Json(CardReply {
            id: user.id,
            name: user.name,
        }))
    }))
    .await
}

async fn verify(Json(body): Json<CardBody>) -> Response {
    harness::reply(trace_result("auth.verify_pin", |ctx| async move {
        ctx.set_attribute("card.last4", last4(&body.card));
        verify_pin(&body.card, &body.pin).await?;

        Ok("ok")
    }))
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("auth-service").devtools())?;

    let port: u16 = std::env::args()
        .nth(1)
        .context("usage: p11-auth <port>")?
        .parse()?;

    let app = Router::new()
        .route("/ping", get(|| async { "ok" }))
        .route("/shutdown", get(harness::shutdown))
        .route("/card", post(card))
        .route("/verify", post(verify));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("auth-service :{port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(harness::shutdown_requested())
        .await?;

    // `_telemetry` drops here, which flushes this service's last spans.
    Ok(())
}
