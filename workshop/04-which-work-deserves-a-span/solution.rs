//! Part 4: Our solution: four spans, not six
//!
//!   cargo sol 04

// check: name = "atm.withdraw"
// check: name = "bank.debit"
// check: name = "bank.verify_pin"
use autotel::{AutotelConfig, ctx, trace, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;

const DENOMINATIONS: [i64; 4] = [5000, 2000, 1000, 500];

/// No span: sub-millisecond, cannot fail, nothing to group by.
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

/// No span either. It is a `format!` call.
fn format_pence(pence: i64) -> String {
    format!("£{}.{:02}", pence / 100, pence % 100)
}

async fn withdraw(card_number: &str, pin: &str, amount_pence: i64) -> Result<Receipt, BankError> {
    trace("atm.withdraw", |_| async move {
        ctx().set_attribute("withdrawal.amount_pence", amount_pence);

        // `trace_result(name, work)` wraps a function you did not write: the
        // closure hands back the bank's own future. The bank modules stay free
        // of instrumentation; the seam is here, at the point of use.
        let user =
            trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
        trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;
        // `get_account` cannot fail, so plain `trace` is enough.
        let account = trace("bank.get_account", |_| get_account(&user.id)).await;

        // The interesting one. Money moved, so the span carries what moved.
        let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
        ctx()
            .set_attribute("account.id", &account.id)
            .set_attribute("withdrawal.transaction_id", &receipt.transaction_id);

        // The two operations without spans still leave a trace: their result
        // is an attribute on the span that is already here.
        let notes = choose_notes(amount_pence);
        ctx()
            .set_attribute("withdrawal.note_count", notes.len())
            .set_attribute("withdrawal.display_amount", format_pence(amount_pence));

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

    println!(
        "
8 withdrawals, 5 spans each. Open one trace in devtools: the waterfall now says
where the time went. Compare the u_007 trace with any other one."
    );

    Ok(())
}
