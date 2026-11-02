use super::{Account, BankError, BankErrorCode, data::ACCOUNTS, sleep};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};

static TRANSACTION_COUNTER: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub transaction_id: String,
    pub balance_pence: i64,
}

/// The account that belongs to a customer, as it stands right now.
pub async fn get_account(user_id: &str) -> Account {
    sleep(10).await;
    ACCOUNTS
        .lock()
        .expect("accounts mutex")
        .iter()
        .find(|candidate| candidate.user_id == user_id)
        .cloned()
        .unwrap_or_else(|| panic!("No account for user {user_id}"))
}

/// Take money out of an account.
///
/// The `legacy-core` shard answers correctly and slowly. Nothing in this file
/// says so out loud, and nothing in the ATM's code knows the shard exists.
/// That is the ordinary situation: the field that explains your latency is a
/// property of the data, not of the request.
pub async fn debit(account: &Account, amount_pence: i64) -> Result<Receipt, BankError> {
    sleep(if account.shard == "legacy-core" {
        320
    } else {
        40
    })
    .await;
    let mut accounts = ACCOUNTS.lock().expect("accounts mutex");
    let stored = accounts
        .iter_mut()
        .find(|candidate| candidate.id == account.id)
        .expect("account exists");

    if amount_pence > stored.balance_pence {
        return Err(BankError::new(
            BankErrorCode::InsufficientFunds,
            "Insufficient funds",
            vec![
                ("account.id", json!(stored.id)),
                ("withdrawal.amount_pence", json!(amount_pence)),
            ],
        ));
    }

    if stored.withdrawn_today_pence + amount_pence > stored.daily_limit_pence {
        return Err(BankError::new(
            BankErrorCode::DailyLimitExceeded,
            "Daily limit exceeded",
            vec![
                ("account.id", json!(stored.id)),
                ("account.daily_limit_pence", json!(stored.daily_limit_pence)),
                (
                    "account.withdrawn_today_pence",
                    json!(stored.withdrawn_today_pence),
                ),
            ],
        ));
    }

    stored.balance_pence -= amount_pence;
    stored.withdrawn_today_pence += amount_pence;
    let number = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;

    Ok(Receipt {
        transaction_id: format!("txn_{number:06}"),
        balance_pence: stored.balance_pence,
    })
}

pub fn reset_ledger() {
    TRANSACTION_COUNTER.store(0, Ordering::Relaxed);
}
