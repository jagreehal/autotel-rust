//! Part 11: Our solution: inject on the way out, extract on the way in
//!
//!   cargo sol 11

// check: name = "atm.withdraw"
// check: name = "accounts.debit"
// check: service = auth-service
#[path = "harness.rs"]
mod harness;

use autotel::{AutotelConfig, inject_trace_context, request_logger, trace_result};
use bank::{reset_balances, users};
use harness::{
    AccountReply, CardReply, DebitReply, json_or_throw, ok_or_throw, start_services, stop_services,
};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

const AUTH: &str = "http://127.0.0.1:7201";

const ACCOUNTS: &str = "http://127.0.0.1:7202";

static SHOWN_HEADER: AtomicBool = AtomicBool::new(false);

async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> anyhow::Result<DebitReply> {
    trace_result("atm.withdraw", |ctx| async move {
        let log = request_logger();
        log.set(json!({ "withdrawal": { "amount_pence": amount_pence } }));
        println!("[atm]              trace {}", ctx.trace_id());

        // `inject_trace_context()` writes the W3C `traceparent` header (and
        // baggage, in part 12) describing the span that is active right now.
        let headers = inject_trace_context();

        if !SHOWN_HEADER.swap(true, Ordering::Relaxed) {
            println!(
                "                   traceparent: {}",
                headers["traceparent"].to_str()?
            );
        }

        let http = reqwest::Client::new();

        let user: CardReply = json_or_throw(
            http.post(format!("{AUTH}/card"))
                .headers(headers.clone())
                .json(&json!({ "card": card_number }))
                .send()
                .await?,
        )
        .await?;

        log.set(json!({ "user": { "id": user.id } }));

        ok_or_throw(
            http.post(format!("{AUTH}/verify"))
                .json(&json!({ "card": card_number, "pin": pin }))
                .headers(headers.clone())
                .send()
                .await?,
        )
        .await?;

        let account: AccountReply = json_or_throw(
            http.get(format!("{ACCOUNTS}/account?userId={}", user.id))
                .headers(headers.clone())
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
            .headers(headers)
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
    start_services("services-propagated", 7201, 7202).await?;

    for user in users().iter().take(3) {
        withdraw(&user.card_number, "1234", 5_000).await?;
    }

    stop_services(&[7201, 7202]).await;
    autotel::shutdown();

    println!(
        "
Three services print the same trace id per withdrawal. In devtools, open one
trace: atm.withdraw on top, auth and accounts spans nested underneath, three
service names in one waterfall.

  name = \"atm.withdraw\""
    );

    Ok(())
}
