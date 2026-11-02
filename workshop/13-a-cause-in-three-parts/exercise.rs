//! Part 13: A cause in three parts
//!
//! Build 4.2.0 of the ATM software went out to four machines on Monday. The
//! dashboard below splits the p95 one label at a time, and no split points
//! anywhere. Every field you need is already on the wide event.
//!
//! TODO: find what the slow withdrawals share, then prove it. Use the devtools
//!       search box, or `autotel::compare_cohorts` on `OBSERVED`.
//!
//!   cargo ex 13

// check: name = "atm.withdraw"
// check: name = "atm.dispense"
// check: atm.software_version = "4.2.0"
#[path = "dispenser.rs"]
mod dispenser;

use autotel::{Attributes, AutotelConfig, bucket, request_logger, trace, trace_result};
use bank::{
    BankError, Receipt, debit, find_user_by_card, get_account, reset_balances, users, verify_pin,
};
use dispenser::{Machine, dispense};
use futures::future::join_all;
use serde_json::{Value, json};
use std::{sync::Mutex, time::Instant};

const fn machine(
    id: &'static str,
    model: &'static str,
    branch: &'static str,
    software_version: &'static str,
) -> Machine {
    Machine {
        id,
        model,
        branch,
        software_version,
    }
}

const MACHINES: [Machine; 10] = [
    machine("atm_0431", "NCR-6622", "bridge-street", "4.1.3"),
    machine("atm_0912", "NCR-6622", "kings-cross", "4.1.3"),
    machine("atm_0977", "NCR-6622", "canada-water", "4.2.0"),
    machine("atm_1180", "Diebold-522", "bridge-street", "4.1.3"),
    machine("atm_1204", "Diebold-522", "canada-water", "4.1.3"),
    machine("atm_1251", "Diebold-522", "bridge-street", "4.2.0"),
    machine("atm_1266", "Diebold-522", "kings-cross", "4.2.0"),
    machine("atm_1330", "Wincor-2100", "canada-water", "4.1.3"),
    machine("atm_1342", "Wincor-2100", "kings-cross", "4.1.3"),
    machine("atm_1388", "Wincor-2100", "bridge-street", "4.2.0"),
];

const AMOUNTS: [i64; 5] = [1000, 2000, 5000, 1500, 6000];

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
            (
                "atm.software_version".into(),
                json!(machine.software_version),
            ),
            (
                "withdrawal.amount_band".into(),
                json!(bucket(amount_pence as f64, &[2000.0, 5000.0])),
            ),
        ]);

        log.set(json!({
            "atm": {
                "id": machine.id,
                "model": machine.model,
                "branch": machine.branch,
                "software_version": machine.software_version,
            },
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
            fields.insert("account.shard".into(), json!(account.shard));
            log.set(json!({ "account": { "id": account.id, "shard": account.shard } }));

            let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
            trace("atm.dispense", |_| dispense(machine, amount_pence)).await;
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

/// A field's value as text, the way a dashboard label shows it.
fn label(fields: &Attributes, field: &str) -> String {
    fields
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_owned()
}

/// What a dashboard shows: one label at a time, one number per value.
fn p95_by(observed: &[(Attributes, f64)], field: &str) -> Vec<String> {
    // Groups in the order their value first appears.
    let mut groups: Vec<(String, Vec<f64>)> = Vec::new();

    for (fields, duration_ms) in observed {
        let value = label(fields, field);
        match groups.iter_mut().find(|(seen, _)| *seen == value) {
            Some((_, durations)) => durations.push(*duration_ms),
            None => groups.push((value, vec![*duration_ms])),
        }
    }

    groups
        .into_iter()
        .map(|(value, mut durations)| {
            durations.sort_by(f64::total_cmp);
            let rank = (durations.len() as f64 * 0.95).ceil() as usize;

            format!("{value} {:.0}ms", durations[rank - 1].round())
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    // Twenty mornings, ten withdrawals each. A fresh morning resets the daily
    // limit, so nobody is refused and every withdrawal reaches the dispenser.
    for start in (0..200).step_by(10) {
        reset_balances();
        let batch = (start..start + 10).map(|index| {
            let machine = MACHINES[index % MACHINES.len()];
            let user = &users()[index % users().len()];
            // Each batch of ten uses one amount, so amount and machine vary
            // independently, the way they do in production.
            let amount_pence = AMOUNTS[(start / 10) % AMOUNTS.len()];

            withdraw(machine, &user.card_number, amount_pence)
        });
        // Refusals are expected; part 7 covered them.
        join_all(batch).await;
    }

    autotel::shutdown();

    let observed = OBSERVED.lock().expect("observed mutex");

    println!("\nThe dashboard, p95 by one label at a time:\n");

    for field in [
        "atm.software_version",
        "atm.model",
        "withdrawal.amount_band",
        "atm.branch",
    ] {
        println!("  {field:<24} {}", p95_by(&observed, field).join("   "));
    }

    println!(
        "
200 withdrawals sent. In the devtools search box:

  name = \"atm.withdraw\" and duration > 500

Find what those withdrawals share, then prove it."
    );

    Ok(())
}
