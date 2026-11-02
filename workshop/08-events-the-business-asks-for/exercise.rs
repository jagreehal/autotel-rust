//! Part 8: The questions the business asks
//!
//! Three questions arrived this week, none of them from an engineer:
//!
//!   1. How much cash left the estate at bridge-street yesterday?
//!   2. Of the customers we refused on the daily limit, how many had the money?
//!   3. When we refuse someone, do they try again for a smaller amount?
//!
//! Question 2 you can already answer, from part 5. The other two are about what
//! people did, they are asked every week, and the person asking does not have a
//! devtools tab.
//!
//! TODO: emit a business event for each completed and each refused withdrawal,
//!       and send it somewhere the product team could actually read.
//!
//!   cargo ex 08

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, request_logger, trace, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use serde_json::json;

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

        let result: Result<Receipt, BankError> = async {
            let user =
                trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
            log.set(json!({ "user": { "id": user.id } }));
            trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;

            let account = trace("bank.get_account", |_| get_account(&user.id)).await;
            let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
            log.set(json!({ "withdrawal": { "result": "dispensed" } }));

            Ok(receipt)
        }
        .await;

        if let Err(error) = &result {
            log.set(json!({
                "withdrawal": { "result": "refused", "refusal_code": error.code.as_str() }
            }));
        }
        log.emit_now();

        result
    })
    .await
}

async fn attempt(index: usize, branch: &str, amount_pence: i64) {
    // Refused; the event above already recorded why.
    let _ = withdraw(branch, &users()[index].card_number, "1234", amount_pence).await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    for index in 0..6 {
        let branch = if index % 2 == 0 {
            "bridge-street"
        } else {
            "kings-cross"
        };
        attempt(index, branch, 10_000).await;
    }

    // Customer 0 comes back for too much, is refused, and tries again smaller.
    attempt(0, "bridge-street", 25_000).await;
    attempt(0, "bridge-street", 15_000).await;

    Ok(())
}
