//! Part 7: Errors that explain themselves
//!
//! Five different refusals reach the screen as the same sentence. Last month
//! the call centre took 400 calls from customers the machine could have told
//! what to do.
//!
//! TODO: give each refusal a message the customer can act on, a reason, a fix,
//!       and a status the caller can branch on. Keep off the screen anything a
//!       shoulder-surfer should not see.
//!
//!   cargo ex 07

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, request_logger, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_auth_attempts, reset_balances,
    users, verify_pin,
};
use serde_json::json;

async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    trace_result("atm.withdraw", |_| async move {
        let log = request_logger();
        log.set(json!({ "withdrawal": { "amount_pence": amount_pence } }));

        let user =
            trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
        log.set(json!({ "user": { "id": user.id } }));

        trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;

        let account = autotel::trace("bank.get_account", |_| get_account(&user.id)).await;
        let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
        log.set(json!({ "withdrawal": { "result": "dispensed" } }));
        log.emit_now();

        Ok(receipt)
    })
    .await
}

/// What the customer standing at the machine actually sees.
fn screen(thrown: &str) {
    println!("┌──────────────────────────────────────┐");
    println!("│ Transaction failed.                  │");
    println!("│ Contact your bank.                   │");
    println!("└──────────────────────────────────────┘");
    println!("  (thrown: {thrown})\n");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();
    reset_auth_attempts();

    let card = users()[1].card_number.as_str();

    let scenarios = [
        ("an unknown card", "4000999999", "1234", 10_000),
        ("a wrong PIN", card, "9999", 10_000),
        ("a second wrong PIN", card, "9999", 10_000),
        (
            "a third wrong PIN, and the machine keeps the card",
            card,
            "9999",
            10_000,
        ),
        (
            "more than the balance",
            users()[2].card_number.as_str(),
            "1234",
            999_999,
        ),
        (
            "over the daily limit",
            users()[3].card_number.as_str(),
            "1234",
            40_000,
        ),
    ];

    for (label, card_number, pin, amount) in scenarios {
        println!("\n── {label} ──");

        if let Err(error) = withdraw(card_number, pin, amount).await {
            screen(&error.to_string());
        }
    }

    Ok(())
}
