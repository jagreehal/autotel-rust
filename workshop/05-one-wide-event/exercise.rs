//! Part 5: One wide event per request
//!
//! Two withdrawals in here fail on the daily limit. To explain one of them you
//! need the branch and the balance (attributes on the parent span) and the
//! refusal code, which exists only inside the error a child span returned.
//!
//! TODO: emit one event per withdrawal carrying everything the withdrawal
//!       knew, so that answering "which customers hit the daily limit at
//!       bridge-street" is one filter and not a join.
//!
//!   cargo ex 05

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, ctx, trace, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};

const BRANCHES: [&str; 2] = ["bridge-street", "kings-cross"];

async fn withdraw(
    branch: &str,
    card_number: &str,
    pin: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    // `trace_result`, not `trace`: from here on withdrawals are refused, and a
    // refusal marks the span as failed.
    trace_result("atm.withdraw", |_| async move {
        ctx()
            .set_attribute("atm.branch", branch)
            .set_attribute("withdrawal.amount_pence", amount_pence);

        let user =
            trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
        ctx().set_attribute("user.id", &user.id);

        trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;

        let account = trace("bank.get_account", |_| get_account(&user.id)).await;
        ctx().set_attribute("account.balance_pence", account.balance_pence);

        trace_result("bank.debit", |_| debit(&account, amount_pence)).await
    })
    .await
}

async fn attempt(index: usize, amount_pence: i64) {
    let user = &users()[index];
    let branch = BRANCHES[index % BRANCHES.len()];

    if let Err(error) = withdraw(branch, &user.card_number, "1234", amount_pence).await {
        println!("refused: {} {}", user.id, error.code);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    for index in 0..8 {
        attempt(index, 10_000).await;
    }
    // Two customers come back for more and hit the £300 daily limit.
    attempt(0, 25_000).await;
    attempt(3, 25_000).await;

    Ok(())
}
