//! Part 6: Our solution: set up first, in one place, then run the app
//!
//!   cargo sol 06

// check: name = "atm.withdraw"
// check-log: pin ok
#[path = "instrumentation.rs"]
mod instrumentation;

use autotel::trace;
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use tracing::info;

// The logger calls are the exercise's, unchanged.
async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    trace("atm.withdraw", |_| async move {
        info!(amount_pence, "withdrawal started");

        let user = find_user_by_card(card_number).await?;

        info!(user_id = %user.id, "user found");
        verify_pin(card_number, pin).await?;
        info!("pin ok");

        let account = get_account(&user.id).await;
        let receipt = debit(&account, amount_pence).await?;

        info!(transaction_id = %receipt.transaction_id, "debited");

        Ok(receipt)
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // No init() and no subscriber here. setup() did both, in that order, and
    // the subscriber it built is the only one this process will ever have.
    let _telemetry = instrumentation::setup()?;

    reset_balances();

    let withdrawals = users()
        .iter()
        .take(5)
        .zip(1..)
        .map(|(user, n)| withdraw(&user.card_number, "1234", n * 2000));

    for result in join_all(withdrawals).await {
        result?;
    }

    println!(
        "
Every line above now carries trace_id and span_id. Pick one trace_id and paste
it into the devtools search box:

  trace_id = \"<a trace_id from above>\"

The trace opens with its log lines attached, in order, under the span that
wrote each one."
    );

    Ok(())
}
