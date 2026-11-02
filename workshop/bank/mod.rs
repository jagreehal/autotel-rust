mod auth;
mod data;
mod errors;
mod ledger;

pub use auth::{find_user_by_card, reset_auth_attempts, verify_pin};
pub use data::{Account, User, reset_balances, users};
pub use errors::{BankError, BankErrorCode};
pub use ledger::{Receipt, debit, get_account, reset_ledger};

/// Wait, the way a call to another system does.
pub async fn sleep(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}
