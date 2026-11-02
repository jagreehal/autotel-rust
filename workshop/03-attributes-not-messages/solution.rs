//! Part 3: Our solution: the facts go on the span
//!
//!   cargo sol 03

// check: name = "atm.withdraw"
// check: atm.branch = "bridge-street"
// check: user.id = "u_007"
use autotel::{AutotelConfig, ctx, trace};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;

struct Machine {
    id: &'static str,
    model: &'static str,
    branch: &'static str,
}

async fn withdraw(
    machine: &Machine,
    card_number: &str,
    pin: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    trace("atm.withdraw", |_| async move {
        // `ctx()` is ambient: it resolves to whichever span is active right
        // here, so nothing has to be threaded through the signature.
        ctx()
            .set_attribute("atm.id", machine.id)
            .set_attribute("atm.model", machine.model)
            .set_attribute("atm.branch", machine.branch)
            .set_attribute("withdrawal.amount_pence", amount_pence);

        let user = find_user_by_card(card_number).await?;
        ctx().set_attribute("user.id", &user.id);

        verify_pin(card_number, pin).await?;

        let account = get_account(&user.id).await;
        ctx()
            .set_attribute("account.id", &account.id)
            .set_attribute("account.balance_pence", account.balance_pence);

        let receipt = debit(&account, amount_pence).await?;
        ctx().set_attribute("withdrawal.transaction_id", &receipt.transaction_id);

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

    println!(
        "
9 withdrawals traced. In the devtools search box, try:

  atm.branch = \"bridge-street\"
  name = \"atm.withdraw\" and duration > 100
  user.id = \"u_007\"

None of those queries were written into the code. That is the point."
    );

    Ok(())
}
