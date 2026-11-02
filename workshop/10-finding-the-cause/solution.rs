//! Part 10: Our solution: record the shard, then let the data rank itself
//!
//!   cargo sol 10

// check: name = "atm.withdraw"
// check: account.shard = "legacy-core"
// check: name = "atm.withdraw" and duration > 200
use autotel::{
    Attributes, AutotelConfig, bucket, compare_cohorts, request_logger, trace, trace_result,
};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use futures::future::join_all;
use serde_json::json;
use std::{sync::Mutex, time::Instant};

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

/// Kept alongside the spans so the analysis loop can run in this file too:
/// each withdrawal's fields, and how long it took in milliseconds.
static OBSERVED: Mutex<Vec<(Attributes, f64)>> = Mutex::new(Vec::new());

async fn withdraw(
    machine: Machine,
    card_number: &str,
    amount_pence: i64,
) -> Result<Receipt, BankError> {
    trace_result("atm.withdraw", |_| async move {
        let log = request_logger();
        let started_at = Instant::now();

        let mut fields = Attributes::from([
            ("atm.id".into(), json!(machine.id)),
            ("atm.model".into(), json!(machine.model)),
            ("atm.branch".into(), json!(machine.branch)),
            // Raw amounts never repeat often enough to describe a group, so the
            // bucket is what makes the number groupable at all.
            (
                "withdrawal.amount_band".into(),
                json!(bucket(amount_pence as f64, &[1000.0, 2000.0, 5000.0])),
            ),
        ]);

        log.set(json!({
            "atm": { "id": machine.id, "model": machine.model, "branch": machine.branch },
            "withdrawal": {
                "amount_pence": amount_pence,
                "amount_band": fields["withdrawal.amount_band"],
            },
        }));

        let result = async {
            let user =
                trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
            fields.insert("user.id".into(), json!(user.id));
            log.set(json!({ "user": { "id": user.id } }));
            trace_result("bank.verify_pin", |_| verify_pin(card_number, "1234")).await?;

            let account = trace("bank.get_account", |_| get_account(&user.id)).await;
            // The field that was missing. It is a property of the account, not
            // of the request, which is exactly why nobody thought to record it.
            fields.insert("account.shard".into(), json!(account.shard));
            log.set(json!({ "account": { "id": account.id, "shard": account.shard } }));

            let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
            fields.insert("withdrawal.result".into(), json!("dispensed"));
            log.set(json!({ "withdrawal": { "result": "dispensed" } }));

            Ok::<_, BankError>(receipt)
        }
        .await;

        if let Err(error) = &result {
            fields.insert("withdrawal.result".into(), json!("refused"));
            fields.insert("withdrawal.refusal_code".into(), json!(error.code.as_str()));
            log.set(json!({
                "withdrawal": { "result": "refused", "refusal_code": error.code.as_str() },
            }));
        }
        log.emit_now();
        let duration_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        OBSERVED
            .lock()
            .expect("observed mutex")
            .push((fields, duration_ms));

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
        // Refusals are expected.
        join_all(batch).await;
    }

    autotel::shutdown();

    let observed = OBSERVED.lock().expect("observed mutex");

    // Step 2 of the core analysis loop: name the group that looks wrong. The
    // duration stays outside the fields, so it cannot rank itself.
    let (slow, normal): (Vec<_>, Vec<_>) = observed.iter().partition(|(_, ms)| *ms > 200.0);
    let fields_of = |events: &[&(Attributes, f64)]| -> Vec<Attributes> {
        events.iter().map(|(fields, _)| fields.clone()).collect()
    };

    // Step 3, which is the part worth automating: what separates them?
    let differences = compare_cohorts(&fields_of(&slow), &fields_of(&normal));

    println!(
        "\n{} slow of {}. What separates them:\n",
        slow.len(),
        observed.len()
    );
    println!("  field                        value          in slow   in normal");

    for d in differences.iter().take(6) {
        println!(
            "  {:<28} {:<14} {:>6.0}%   {:>7.0}%",
            d.field,
            d.value,
            d.outlier_fraction * 100.0,
            d.baseline_fraction * 100.0
        );
    }

    println!(
        "
The top row is the answer, and it is a field the exercise never recorded.
Confirm it against individual traces before believing it:

  account.shard = \"legacy-core\"
  name = \"atm.withdraw\" and duration > 200"
    );

    Ok(())
}
