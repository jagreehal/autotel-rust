//! The accounts service. Owns the ledger, and the shard from part 10.

#[path = "../harness.rs"]
mod harness;

use anyhow::Context;
use autotel::{AutotelConfig, trace, trace_result};
use axum::{
    Json, Router,
    extract::Query,
    response::Response,
    routing::{get, post},
};
use bank::{BankError, debit, get_account};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Params {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    amount_pence: i64,
}

async fn account(Query(params): Query<Params>) -> Response {
    harness::reply(trace("accounts.get", |ctx| async move {
        println!("[accounts-service] trace {}", ctx.trace_id());
        let found = get_account(&params.user_id).await;
        ctx.set_attribute("account.id", &found.id);
        ctx.set_attribute("account.shard", &found.shard);

        Ok::<_, BankError>(Json(found))
    }))
    .await
}

async fn debit_account(Query(params): Query<Params>) -> Response {
    harness::reply(trace_result("accounts.debit", |ctx| async move {
        let account = get_account(&params.user_id).await;
        ctx.set_attribute("account.id", &account.id);
        ctx.set_attribute("account.shard", &account.shard);
        ctx.set_attribute("withdrawal.amount_pence", params.amount_pence);

        debit(&account, params.amount_pence).await.map(Json)
    }))
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("accounts-service").devtools())?;

    let port: u16 = std::env::args()
        .nth(1)
        .context("usage: p11-accounts <port>")?
        .parse()?;

    let app = Router::new()
        .route("/ping", get(|| async { "ok" }))
        .route("/shutdown", get(harness::shutdown))
        .route("/account", get(account))
        .route("/debit", post(debit_account));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("accounts-service :{port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(harness::shutdown_requested())
        .await?;

    // `_telemetry` drops here, which flushes this service's last spans.
    Ok(())
}
