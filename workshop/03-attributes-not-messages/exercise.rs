//! Part 3: Attributes, not messages
//!
//! Every trace here is named `atm.withdraw` and most last about 80ms. They are
//! indistinguishable. Support wants to know whether the slow withdrawals come
//! from one machine, one model or one branch, and nothing in this data can
//! answer that.
//!
//! TODO: put the facts of the withdrawal on the span instead of into strings,
//!       then answer the three questions in the README from the devtools search
//!       box rather than by reading code.
//!
//!   cargo ex 03

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, trace};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;

struct Machine {
    id: &'static str,
    model: &'static str,
    // Nothing reads this yet, and it is one of the facts support asks about.
    #[allow(dead_code)]
    branch: &'static str,
}

async fn withdraw(
    machine: &Machine,
    card_number: &str,
    pin: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    trace("atm.withdraw", |_| async move {
        println!("withdrawal at {} ({})", machine.id, machine.model);

        let user = find_user_by_card(card_number).await?;
        println!("customer {}", user.name);

        verify_pin(card_number, pin).await?;

        let account = get_account(&user.id).await;
        println!("account {}, balance {}p", account.id, account.balance_pence);

        let receipt = debit(&account, amount_pence).await?;
        println!("dispensed {amount_pence}p, txn {}", receipt.transaction_id);

        Ok(receipt)
    })
    .await
}

const MACHINES: [Machine; 3] = [
    Machine {
        id: "atm_0431",
        model: "NCR-6622",
        branch: "bridge-street",
    },
    Machine {
        id: "atm_0912",
        model: "NCR-6622",
        branch: "kings-cross",
    },
    Machine {
        id: "atm_1180",
        model: "Diebold-522",
        branch: "bridge-street",
    },
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    let withdrawals = users()
        .iter()
        .take(9)
        .zip(MACHINES.iter().cycle())
        .zip(1..)
        .map(|((user, machine), n)| withdraw(machine, &user.card_number, "1234", n * 2000));

    for result in join_all(withdrawals).await {
        result?;
    }

    Ok(())
}
