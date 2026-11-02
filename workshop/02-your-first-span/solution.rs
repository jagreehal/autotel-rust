//! Part 2: Our solution: init once, wrap once, flush at the end
//!
//!   npx autotel-devtools   # in another terminal
//!   cargo sol 02           # then look at http://127.0.0.1:4318

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, trace};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;

// `trace(name, work)` starts a span, runs `work` inside it, and ends the span
// when `work` finishes. Wrapping the whole body makes each call one span.
async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    trace("atm.withdraw", |_| async move {
        let user = find_user_by_card(card_number).await?;
        verify_pin(card_number, pin).await?;
        let account = get_account(&user.id).await;
        debit(&account, amount_pence).await
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // One call, once per process, before any traced code runs. `.devtools()`
    // points the exporter at http://127.0.0.1:4318, which is the receiver
    // `npx autotel-devtools` is running.
    //
    // Keep the guard it returns. Spans are batched, and dropping the guard is
    // what flushes the last batch. `let _ = ...` would drop it on this line.
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    let withdrawals = users()
        .iter()
        .take(5)
        .zip(1..)
        .map(|(user, n)| withdraw(&user.card_number, "1234", n * 2000));

    for result in join_all(withdrawals).await {
        result?;
    }

    println!("5 withdrawals traced → http://127.0.0.1:4318");

    Ok(())
}
