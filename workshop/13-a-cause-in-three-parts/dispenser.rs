//! The note dispenser driver, which ships with the ATM software.
//!
//! Do not read this file until you have found the cause from the data. It is
//! here for the same reason `bank/ledger.rs` was in part 10: the code that
//! explains your latency is usually somewhere you were not looking.

use bank::sleep;

#[derive(Clone, Copy)]
pub struct Machine {
    pub id: &'static str,
    pub model: &'static str,
    pub branch: &'static str,
    pub software_version: &'static str,
}

/// Count out the notes. Build 4.2.0 replaced the note-mix calculation, and on
/// Diebold hardware a mixed-denomination amount (£50 and over) now recalibrates
/// the cassettes before every dispense. Each condition alone is harmless.
pub async fn dispense(machine: Machine, amount_pence: i64) {
    let recalibrates = machine.software_version == "4.2.0"
        && machine.model == "Diebold-522"
        && amount_pence >= 5000;

    sleep(if recalibrates { 600 } else { 30 }).await;
}
