//! Part 11: One trace, three services
//!
//! Nothing about the withdrawal changed. The four steps that used to be
//! function calls are now HTTP calls to two other processes, each with its own
//! init() and its own service name.
//!
//! TODO: run it, look at devtools, and work out why one withdrawal is now three
//!       unrelated traces, then make it one again.
//!
//!   cargo ex 11

// check: name = "atm.withdraw"
#[path = "harness.rs"]
mod harness;

use autotel::{AutotelConfig, request_logger, trace_result};
use bank::{reset_balances, users};
use harness::{
    AccountReply, CardReply, DebitReply, json_or_throw, ok_or_throw, start_services, stop_services,
};
use serde_json::json;

const AUTH: &str = "http://127.0.0.1:7101";

const ACCOUNTS: &str = "http://127.0.0.1:7102";

async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> anyhow::Result<DebitReply> {
    trace_result("atm.withdraw", |ctx| async move {
        let log = request_logger();
        log.set(json!({ "withdrawal": { "amount_pence": amount_pence } }));
        println!("[atm]              trace {}", ctx.trace_id());

        let http = reqwest::Client::new();

        let user: CardReply = json_or_throw(
            http.post(format!("{AUTH}/card"))
                .json(&json!({ "card": card_number }))
                .send()
                .await?,
        )
        .await?;

        log.set(json!({ "user": { "id": user.id } }));

        ok_or_throw(
            http.post(format!("{AUTH}/verify"))
                .json(&json!({ "card": card_number, "pin": pin }))
                .send()
                .await?,
        )
        .await?;

        let account: AccountReply = json_or_throw(
            http.get(format!("{ACCOUNTS}/account?userId={}", user.id))
                .send()
                .await?,
        )
        .await?;

        log.set(json!({ "account": { "id": account.id, "shard": account.shard } }));

        let receipt: DebitReply = json_or_throw(
            http.post(format!(
                "{ACCOUNTS}/debit?userId={}&amountPence={amount_pence}",
                user.id
            ))
            .send()
            .await?,
        )
        .await?;

        log.set(json!({ "withdrawal": { "result": "dispensed" } }));
        log.emit_now();

        Ok(receipt)
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();
    start_services("services", 7101, 7102).await?;

    for user in users().iter().take(3) {
        withdraw(&user.card_number, "1234", 5_000).await?;
    }

    stop_services(&[7101, 7102]).await;
    autotel::shutdown();

    println!(
        "
3 withdrawals. Now look at devtools and count the traces.

  service = atm
  service = auth-service
  service = accounts-service

Each of those has its own traces, and none of them contains the others."
    );

    Ok(())
}
