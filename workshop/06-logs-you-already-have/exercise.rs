//! Part 6: Logs you already have
//!
//! The bank's code already logs with `tracing`, and nobody is going to delete
//! four hundred log lines to adopt OpenTelemetry. This file calls
//! autotel::init() on the first line of main, every withdrawal is traced, and
//! still no log line carries a trace id.
//!
//! TODO: make every log line carry the trace_id and span_id of the withdrawal
//!       that wrote it, without adding either field by hand.
//!
//!   cargo ex 06

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, trace};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use tracing::info;

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
    // The first line of main. Every withdrawal below runs inside a span.
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    // The team's logger, set up the way it has been for three years.
    tracing_subscriber::fmt().json().init();

    reset_balances();

    // Part 1's five machines, logging into one stream again.
    let withdrawals = users()
        .iter()
        .take(5)
        .zip(1..)
        .map(|(user, n)| withdraw(&user.card_number, "1234", n * 2000));

    for result in join_all(withdrawals).await {
        result?;
    }

    Ok(())
}
