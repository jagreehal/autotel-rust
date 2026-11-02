//! Part 1: Our solution: give every withdrawal an id, and log key-values
//!
//! No library yet. This is what you would write yourself once the
//! interleaved output beat you, and it is worth writing because of what it
//! still cannot do.
//!
//!   cargo sol 01

use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use serde::Serialize;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The facts a line can carry. Named fields, so a typo fails to compile.
#[derive(Default, Serialize)]
struct LogFields<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_pence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transaction_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
}

/// One line, one JSON object, one id tying the object to its withdrawal.
fn log(withdrawal_id: &str, event: &str, fields: LogFields<'_>) {
    #[derive(Serialize)]
    struct Line<'a> {
        t: u128,
        withdrawal_id: &'a str,
        event: &'a str,
        #[serde(flatten)]
        fields: LogFields<'a>,
    }
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let line = Line {
        t,
        withdrawal_id,
        event,
        fields,
    };
    println!(
        "{}",
        serde_json::to_string(&line).expect("a line serialises")
    );
}

async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    let withdrawal_id = format!("{:08x}", rand::random::<u32>());
    let started_at = Instant::now();
    log(
        &withdrawal_id,
        "withdrawal.started",
        LogFields {
            amount_pence: Some(amount_pence),
            ..LogFields::default()
        },
    );

    let user = find_user_by_card(card_number).await?;
    log(
        &withdrawal_id,
        "user.found",
        LogFields {
            user_id: Some(&user.id),
            ..LogFields::default()
        },
    );

    verify_pin(card_number, pin).await?;
    log(&withdrawal_id, "pin.verified", LogFields::default());

    let account = get_account(&user.id).await;
    log(
        &withdrawal_id,
        "account.found",
        LogFields {
            account_id: Some(&account.id),
            ..LogFields::default()
        },
    );

    let receipt = debit(&account, amount_pence).await?;
    log(
        &withdrawal_id,
        "account.debited",
        LogFields {
            transaction_id: Some(&receipt.transaction_id),
            ..LogFields::default()
        },
    );

    log(
        &withdrawal_id,
        "withdrawal.completed",
        LogFields {
            duration_ms: Some(started_at.elapsed().as_millis()),
            ..LogFields::default()
        },
    );

    Ok(receipt)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
Now you can grep one withdrawal out of the noise:

  cargo sol 01 | grep <withdrawal_id>

Four things this still will not tell you:

  1. How long verify_pin took. You timed the whole withdrawal, not its parts.
  2. Which step was slow when one withdrawal is slow.
  3. What happened when the process died before the last line was written.
  4. Anything at all once the debit moves to another service.

Every one of those is fixed by the same change, and you have already written
most of it. Part 2."
    );

    Ok(())
}
