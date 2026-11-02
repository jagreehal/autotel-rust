//! Part 9: Our solution: two instruments, recorded once per withdrawal
//!
//!   cargo sol 09

// check: name = "atm.withdraw"
// check-metric: atm.withdraw.duration
// check-metric: atm.withdrawals
use autotel::{
    AutotelConfig, KeyValue, create_counter, create_histogram, request_logger, trace, trace_result,
};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use serde_json::json;
use std::time::Instant;

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

    // Create instruments after `init`. One created before it records into a
    // no-op meter, and nothing ever arrives.
    //
    // A histogram keeps a count per time range (a bucket), not each duration.
    let duration = create_histogram(
        "atm.withdraw.duration",
        "ms",
        "How long each withdrawal took, start to finish",
    );
    // A counter only goes up. The dashboard turns it into withdrawals per minute.
    let withdrawals = create_counter("atm.withdrawals", "Withdrawals, by branch and result");

    reset_balances();

    // Ten machines busy at once, so 200 withdrawals take two seconds rather than
    // twenty. The estate is not polite enough to queue either.
    for start in (0..200).step_by(10) {
        let batch = (start..start + 10).map(|index| {
            let machine = MACHINES[index % MACHINES.len()];
            let user = &users()[index % users().len()];
            let amount_pence = 1000 + (index % 4) as i64 * 500;

            let (duration, withdrawals) = (&duration, &withdrawals);

            async move {
                let started_at = Instant::now();
                let result = withdraw(machine, &user.card_number, amount_pence).await;

                // Labels are the only way to split a metric later, and each
                // distinct combination becomes its own series. Branch and
                // result have a handful of values between them. A user id
                // here would mean one series per customer.
                let outcome = if result.is_ok() {
                    "dispensed"
                } else {
                    "refused"
                };
                duration.record(
                    started_at.elapsed().as_secs_f64() * 1000.0,
                    &[KeyValue::new("atm.branch", machine.branch)],
                );
                withdrawals.add(
                    1,
                    &[
                        KeyValue::new("atm.branch", machine.branch),
                        KeyValue::new("withdrawal.result", outcome),
                    ],
                );
            }
        });
        // Refusals are expected; part 7 covered them.
        join_all(batch).await;
    }

    autotel::shutdown();

    println!(
        "
200 withdrawals, two metrics. Open devtools' Metrics view:

  atm.withdraw.duration   one series per branch
  atm.withdrawals         one series per branch and result

Compare the branches. Two of them look slow. Part 10 asks whether they are."
    );

    Ok(())
}
