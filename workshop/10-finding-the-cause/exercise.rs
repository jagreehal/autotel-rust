//! Part 10: Finding the cause
//!
//! 200 withdrawals across the estate. A handful are slow. Nobody knows why, and
//! nobody knows in advance which field explains it. That is the situation.
//!
//! This code is well instrumented by the standards of part 5. Run it, then use
//! the devtools search box to find out what the slow withdrawals have in common.
//!
//! TODO: when the data will not answer the question, work out which field is
//!       missing, add it, and try again.
//!
//!   cargo ex 10

// check: name = "atm.withdraw"
use autotel::{AutotelConfig, request_logger, trace, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use serde_json::json;

#[derive(Clone, Copy)]
struct Machine {
    id: &'static str,
    model: &'static str,
    branch: &'static str,
}

const MACHINES: [Machine; 5] = [
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
    Machine {
        id: "atm_1204",
        model: "Diebold-522",
        branch: "canada-water",
    },
    Machine {
        id: "atm_1330",
        model: "Wincor-2100",
        branch: "canada-water",
    },
];

async fn withdraw(
    machine: Machine,
    card_number: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    trace_result("atm.withdraw", |_| async move {
        let log = request_logger();
        log.set(json!({
            "atm": { "id": machine.id, "model": machine.model, "branch": machine.branch },
            "withdrawal": { "amount_pence": amount_pence },
        }));

        let result = async {
            let user =
                trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
            log.set(json!({ "user": { "id": user.id } }));
            trace_result("bank.verify_pin", |_| verify_pin(card_number, "1234")).await?;

            let account = trace("bank.get_account", |_| get_account(&user.id)).await;
            log.set(json!({ "account": { "id": account.id } }));

            let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
            log.set(json!({ "withdrawal": { "result": "dispensed" } }));

            Ok::<_, BankError>(receipt)
        }
        .await;

        if let Err(error) = &result {
            log.set(json!({
                "withdrawal": { "result": "refused", "refusal_code": error.code.as_str() },
            }));
        }
        log.emit_now();

        result
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();

    // Ten machines busy at once, so 200 withdrawals take two seconds rather than
    // twenty. The estate is not polite enough to queue either.
    for start in (0..200).step_by(10) {
        let batch = (start..start + 10).map(|index| {
            let machine = MACHINES[index % MACHINES.len()];
            let user = &users()[index % users().len()];
            let amount_pence = 1000 + (index % 4) as i64 * 500;

            withdraw(machine, &user.card_number, amount_pence)
        });
        // Refusals are expected; part 7 covered them.
        join_all(batch).await;
    }

    autotel::shutdown();

    println!(
        "
200 withdrawals sent. In the devtools search box:

  name = \"atm.withdraw\" and duration > 200

Some of them are slow. Now find out what they have in common. Group by
atm.model, then atm.branch, then withdrawal.amount_pence. Keep going until you
run out of fields."
    );

    Ok(())
}
