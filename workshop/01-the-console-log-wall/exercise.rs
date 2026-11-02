//! Part 1: The println! wall
//!
//! Five customers withdraw cash at the same moment. This is the
//! instrumentation you already have. Run it, then try to answer the question
//! in the README.
//!
//!   cargo ex 01

use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;

async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    println!("withdrawal started");

    let user = find_user_by_card(card_number).await?;
    println!("user found: {}", user.name);

    verify_pin(card_number, pin).await?;
    println!("pin ok");

    let account = get_account(&user.id).await;
    println!("account found, balance {}", account.balance_pence);

    let receipt = debit(&account, amount_pence).await?;
    println!("debited, new balance {}", receipt.balance_pence);

    println!("withdrawal complete");

    Ok(receipt)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    reset_balances();

    // Five machines, five customers, one process. Exactly as it is in
    // production, where nothing waits its turn.
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
