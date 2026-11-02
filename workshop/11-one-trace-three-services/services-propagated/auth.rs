//! The auth service. Its own process, its own init(), its own service name.

#[path = "../harness.rs"]
mod harness;

use anyhow::Context;
use autotel::{AutotelConfig, FutureExt, extract_trace_context, trace_result};
use axum::{
    Json, Router,
    extract::Request,
    middleware::{self, Next},
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

/// Everything this request does runs inside the context the caller sent, so
/// any span started below is a child of the caller's span rather than a root.
async fn with_caller_context(request: Request, next: Next) -> Response {
    let incoming = extract_trace_context(request.headers());

    next.run(request).with_context(incoming).await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(
        AutotelConfig::new("auth-service")
            .devtools()
            // Copy any baggage the caller sent onto every span this service
            // creates, with no prefix. Nothing sets baggage until part 12; this
            // is the receiver half, and it costs nothing while the header is empty.
            .baggage(""),
    )?;

    let port: u16 = std::env::args()
        .nth(1)
        .context("usage: p11-auth <port>")?
        .parse()?;

    let app = Router::new()
        .route("/ping", get(|| async { "ok" }))
        .route("/shutdown", get(harness::shutdown))
        .route("/card", post(card))
        .route("/verify", post(verify))
        .layer(middleware::from_fn(with_caller_context));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("auth-service :{port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(harness::shutdown_requested())
        .await?;

    // `_telemetry` drops here, which flushes this service's last spans.
    Ok(())
}
