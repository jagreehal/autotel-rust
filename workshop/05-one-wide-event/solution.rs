//! Part 5: Our solution: one snapshot per withdrawal
//!
//!   cargo sol 05

// check: name = "atm.withdraw"
// check: withdrawal.result = "refused"
// check: withdrawal.refusal_code = "DAILY_LIMIT_EXCEEDED"
use autotel::{AutotelConfig, request_logger, trace, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use serde_json::json;

const BRANCHES: [&str; 2] = ["bridge-street", "kings-cross"];

async fn withdraw(
    branch: &str,
    card_number: &str,
    pin: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    trace_result("atm.withdraw", |_| async move {
        // One logger for this withdrawal. It reads the active span, so it does
        // not need to be passed anywhere, and every `set` lands on the same
        // event.
        let log = request_logger();
        log.set(json!({
            "atm": { "branch": branch },
            "withdrawal": { "amount_pence": amount_pence },
        }));

        // The fallible steps run in their own block, so `?` leaves the block
        // and not the withdrawal: whichever way it goes, we still get to
        // record the outcome below.
        let result: Result<Receipt, BankError> = async {
            let user =
                trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
            log.set(json!({ "user": { "id": user.id } }));

            trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;

            let account = trace("bank.get_account", |_| get_account(&user.id)).await;
            log.set(json!({
                "account": {
                    "id": account.id,
                    "balance_pence": account.balance_pence,
                    "withdrawn_today_pence": account.withdrawn_today_pence,
                    "daily_limit_pence": account.daily_limit_pence,
                },
            }));

            trace_result("bank.debit", |_| debit(&account, amount_pence)).await
        }
        .await;

        match &result {
            Ok(receipt) => log.set(json!({
                "withdrawal": {
                    "result": "dispensed",
                    "transaction_id": receipt.transaction_id,
                },
            })),
            // The refusal is a fact about the withdrawal, so it goes on the
            // same event as everything else the withdrawal knew.
            Err(error) => log.set(json!({
                "withdrawal": {
                    "result": "refused",
                    "refusal_code": error.code.as_str(),
                },
            })),
        };

        // One line per withdrawal, whichever way it went.
        log.emit_now();

        result
    })
    .await
}

async fn attempt(index: usize, amount_pence: i64) {
    let user = &users()[index];
    let branch = BRANCHES[index % BRANCHES.len()];

    // A refusal is an outcome, not a crash. The wide event already holds it.
    let _ = withdraw(branch, &user.card_number, "1234", amount_pence).await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    for index in 0..8 {
        attempt(index, 10_000).await;
    }
    attempt(0, 25_000).await;
    attempt(3, 25_000).await;

    println!(
        "
10 withdrawals, 10 events. The question that needed three spans is now one
filter:

  withdrawal.result = \"refused\" and atm.branch = \"bridge-street\"

And the one nobody thought to ask in advance, which is the real test:

  withdrawal.refusal_code = \"DAILY_LIMIT_EXCEEDED\" and account.balance_pence > 40000"
    );

    Ok(())
}
