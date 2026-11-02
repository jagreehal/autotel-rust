//! Part 4: Which work deserves a span
//!
//! One span per withdrawal tells you a withdrawal was slow. It does not tell
//! you which part of it was slow, and that is the question you actually have.
//!
//! Six operations run inside a withdrawal. Decide which of them get their own
//! span, then wrap those and only those.
//!
//!   1. verify_pin: checks the PIN, counts failures, can lock the card
//!   2. find_user_by_card: looks the customer up
//!   3. get_account: looks the account up
//!   4. debit: moves the money
//!   5. choose_notes: arithmetic over four denominations
//!   6. format_pence: turns 4000 into "£40.00"
//!
//!   cargo ex 04

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, ctx, trace};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;

const DENOMINATIONS: [i64; 4] = [5000, 2000, 1000, 500];

/// Greedy note selection. Pure arithmetic over a four-element array.
fn choose_notes(amount_pence: i64) -> Vec<i64> {
    let mut notes = Vec::new();
    let mut remaining = amount_pence;

    let mut denominations = DENOMINATIONS;
    denominations.sort_unstable_by(|a, b| b.cmp(a));
    for note in denominations {
        while remaining >= note {
            notes.push(note);
            remaining -= note;
        }
    }

    notes
}

fn format_pence(pence: i64) -> String {
    format!("£{}.{:02}", pence / 100, pence % 100)
}

// TODO: give a span to the operations that earn one, and to nothing else.
async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    trace("atm.withdraw", |_| async move {
        ctx().set_attribute("withdrawal.amount_pence", amount_pence);

        let user = find_user_by_card(card_number).await?;
        verify_pin(card_number, pin).await?;
        let account = get_account(&user.id).await;
        let receipt = debit(&account, amount_pence).await?;

        let notes = choose_notes(amount_pence);
        println!(
            "dispensing {} as {} notes",
            format_pence(amount_pence),
            notes.len()
        );

        Ok(receipt)
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    let withdrawals = users()
        .iter()
        .take(8)
        .zip(1..)
        .map(|(user, n)| withdraw(&user.card_number, "1234", n * 2000));

    for result in join_all(withdrawals).await {
        result?;
    }

    Ok(())
}
