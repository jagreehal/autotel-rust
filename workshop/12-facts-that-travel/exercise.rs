//! Part 12: Facts that travel
//!
//! One trace, three services. Part 10's method works again, right up to the
//! moment you try to group the accounts service by branch.
//!
//! The ATM knows which machine and which branch. The accounts service does not,
//! and no function signature between them carries it.
//!
//! TODO: make `atm.branch` and `atm.id` available on every span in every
//!       service, without adding a parameter to anything.
//!
//!   cargo ex 12

// check: name = "atm.withdraw"
#[path = "../11-one-trace-three-services/harness.rs"]
mod harness;

use autotel::{AutotelConfig, inject_trace_context, request_logger, trace_result};
use bank::{reset_balances, users};
use harness::{
    AccountReply, CardReply, DebitReply, json_or_throw, ok_or_throw, start_services, stop_services,
};
use serde_json::json;

const AUTH: &str = "http://127.0.0.1:7301";

const ACCOUNTS: &str = "http://127.0.0.1:7302";

struct Machine {
    id: &'static str,
    branch: &'static str,
}

const MACHINES: [Machine; 2] = [
    Machine {
        id: "atm_0431",
        branch: "bridge-street",
    },
    Machine {
        id: "atm_1330",
        branch: "canada-water",
    },
];

async fn withdraw(
    machine: &Machine,
    card_number: &str,
    amount_pence: i64,
) -> anyhow::Result<DebitReply> {
    trace_result("atm.withdraw", |ctx| async move {
        let log = request_logger();
        // These two facts exist here and nowhere else.
        ctx.set_attribute("atm.id", machine.id);
        ctx.set_attribute("atm.branch", machine.branch);
        log.set(json!({ "withdrawal": { "amount_pence": amount_pence } }));

        let headers = inject_trace_context();
        let http = reqwest::Client::new();

        let user: CardReply = json_or_throw(
            http.post(format!("{AUTH}/card"))
                .headers(headers.clone())
                .json(&json!({ "card": card_number }))
                .send()
                .await?,
        )
        .await?;

        ok_or_throw(
            http.post(format!("{AUTH}/verify"))
                .json(&json!({ "card": card_number, "pin": "1234" }))
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

        log.set(json!({
            "account": { "id": account.id },
            "withdrawal": { "result": "dispensed" },
        }));
        log.emit_now();

        Ok(receipt)
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();
    start_services("services-propagated", 7301, 7302).await?;

    for (index, user) in users().iter().take(6).enumerate() {
        withdraw(&MACHINES[index % 2], &user.card_number, 5_000).await?;
    }

    stop_services(&[7301, 7302]).await;
    autotel::shutdown();

    println!(
        "
6 withdrawals. Now try this in devtools:

  service = accounts-service and atm.branch = \"canada-water\"

Nothing. The accounts service has never heard of a branch."
    );

    Ok(())
}
