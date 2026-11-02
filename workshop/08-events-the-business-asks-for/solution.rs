//! Part 8: Our solution: track() alongside the span, not instead of it
//!
//!   cargo sol 08

// check: name = "atm.withdraw"
// check: withdrawal.result = "dispensed"
use autotel::{
    Attributes, AutotelConfig, FileSubscriber, request_logger, trace, trace_result, track,
};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use serde_json::{Value, json};
use std::fs;

const EVENTS_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/08-events-the-business-asks-for/events.ndjson"
);

async fn withdraw(
    branch: &str,
    card_number: &str,
    pin: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    trace_result("atm.withdraw", |_| async move {
        let log = request_logger();
        log.set(
            json!({ "atm": { "branch": branch }, "withdrawal": { "amount_pence": amount_pence } }),
        );

        // Outside the inner block so the refusal event can name the customer
        // too. Without it, "do refused customers come back?" has nothing to
        // join on.
        let mut user_id: Option<String> = None;

        let result: Result<Receipt, BankError> = async {
            let user =
                trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
            user_id = Some(user.id.clone());
            log.set(json!({ "user": { "id": user.id } }));
            trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;

            let account = trace("bank.get_account", |_| get_account(&user.id)).await;
            let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
            log.set(json!({ "withdrawal": { "result": "dispensed" } }));

            // The business event. Named for what happened in the world, not for
            // what the code did, and carrying the numbers a product person
            // would group by.
            track(
                "atm.cash_withdrawn",
                Attributes::from([
                    ("user.id".into(), json!(user.id)),
                    ("atm.branch".into(), json!(branch)),
                    ("withdrawal.amount_pence".into(), json!(amount_pence)),
                    (
                        "account.balance_after_pence".into(),
                        json!(receipt.balance_pence),
                    ),
                ]),
            );

            Ok(receipt)
        }
        .await;

        if let Err(error) = &result {
            let code = error.code.as_str();
            log.set(json!({ "withdrawal": { "result": "refused", "refusal_code": code } }));

            let mut attributes = Attributes::from([
                ("atm.branch".into(), json!(branch)),
                ("withdrawal.amount_pence".into(), json!(amount_pence)),
                ("withdrawal.refusal_code".into(), json!(code)),
            ]);
            if let Some(user_id) = &user_id {
                attributes.insert("user.id".into(), json!(user_id));
            }
            track("atm.withdrawal_refused", attributes);
        }
        log.emit_now();

        result
    })
    .await
}

async fn attempt(index: usize, branch: &str, amount_pence: i64) {
    // Refused; both records above already say why.
    let _ = withdraw(branch, &users()[index].card_number, "1234", amount_pence).await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Start each run with an empty file; the subscriber appends.
    let _ = fs::remove_file(EVENTS_FILE);

    let _telemetry = autotel::init(
        AutotelConfig::new("atm")
            .devtools()
            // Where business events go. In production this is PostHog,
            // Mixpanel, Amplitude, Segment or your own webhook; the `track()`
            // calls do not change.
            .subscriber(FileSubscriber::new(EVENTS_FILE)?),
    )?;

    reset_balances();

    for index in 0..6 {
        let branch = if index % 2 == 0 {
            "bridge-street"
        } else {
            "kings-cross"
        };
        attempt(index, branch, 10_000).await;
    }

    attempt(0, "bridge-street", 25_000).await;
    attempt(0, "bridge-street", 15_000).await;

    // FileSubscriber writes each line inside track(), so the file is already
    // complete. Spans are still batched, and still flushed when `_telemetry`
    // drops at the end of main.
    let events: Vec<Value> = fs::read_to_string(EVENTS_FILE)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    println!(
        "\n{} business events written to events.ndjson\n",
        events.len()
    );

    for event in &events {
        let attributes = &event["attributes"];
        let pence = attributes["withdrawal.amount_pence"].as_i64().unwrap_or(0);
        let trace_id = attributes["traceId"].as_str().unwrap_or_default();
        println!(
            "  {:<28} £{:>7.2}  trace {}",
            event["name"].as_str().unwrap_or_default(),
            pence as f64 / 100.0,
            &trace_id[..trace_id.len().min(12)]
        );
    }

    let refused = events
        .iter()
        .find(|event| event["name"] == "atm.withdrawal_refused")
        .and_then(|event| event["attributes"]["traceId"].as_str())
        .unwrap_or_default();
    println!(
        "
Every event carries the traceId of the request that produced it. Paste this one
into the devtools search box and you land on the exact withdrawal behind the
number:

  trace_id = \"{refused}\""
    );

    Ok(())
}
