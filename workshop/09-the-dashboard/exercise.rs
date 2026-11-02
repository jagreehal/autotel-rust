//! Part 9: The dashboard
//!
//! Before anyone can investigate slow withdrawals, someone has to notice them.
//! Operations want a chart of withdrawal time per branch and a count of
//! withdrawals by outcome, kept for a year, at a cost that does not grow with
//! traffic. The spans here are too detailed and too expensive to keep that long.
//!
//! TODO: record a histogram of how long each withdrawal took, split by branch,
//!       and a counter of withdrawals, split by branch and result.
//!
//!   cargo ex 09

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
200 withdrawals sent, and nothing in devtools' Metrics view yet. Operations
want two charts: time per branch, and withdrawals by branch and result."
    );

    Ok(())
}
