//! Part 2: Your first span
//!
//! This is where part 1 left you: ids and JSON, maintained by hand.
//!
//! TODO 1: call autotel::init() once, at the top of main, before anything
//!         else runs.
//! TODO 2: wrap the body of withdraw() so each call becomes one span.
//! TODO 3: make sure the spans leave the process before it exits.
//!
//!   npx autotel-devtools   # in another terminal, left open all workshop
//!   cargo ex 02            # then look at http://127.0.0.1:4318

use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use serde::Serialize;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The facts a line can carry. Named fields, so a typo fails to compile.
#[derive(Default, Serialize)]
struct LogFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_pence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
}

fn log(withdrawal_id: &str, event: &str, fields: LogFields) {
    #[derive(Serialize)]
    struct Line<'a> {
        t: u128,
        withdrawal_id: &'a str,
        event: &'a str,
        #[serde(flatten)]
        fields: LogFields,
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
    verify_pin(card_number, pin).await?;
    let account = get_account(&user.id).await;
    let receipt = debit(&account, amount_pence).await?;

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

    Ok(())
}
