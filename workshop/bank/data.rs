//! The bank's data, in memory.
//!
//! Nothing in `bank` uses autotel. The domain is the constant across all
//! thirteen parts; the observability is the variable. Every part instruments
//! these same four operations from the outside, so when a part looks different
//! from the one before it, the difference is the lesson and not a change of
//! subject.

use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub name: String,
    pub card_number: String,
    pub pin_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub user_id: String,
    pub balance_pence: i64,
    pub daily_limit_pence: i64,
    pub withdrawn_today_pence: i64,
    /// Which ledger cluster holds this account. Accounts migrated from the
    /// 1998 core sit on `legacy-core`, which answers the same questions
    /// correctly and roughly eight times slower. Two accounts of twenty-five
    /// sit on it.
    ///
    /// Part 10 is about finding this field without being told it exists.
    pub shard: String,
}

/// Not a hash. A workshop. Never do this.
pub(crate) fn pin_hash_of(pin: &str) -> String {
    format!("pin:{pin}")
}

const NAMES: [&str; 25] = [
    "Ada Okafor",
    "Bruno Salas",
    "Chen Wei",
    "Dara Nolan",
    "Emeka Bello",
    "Farah Haddad",
    "Greta Lind",
    "Hugo Marchetti",
    "Ines Duarte",
    "Jonas Vetter",
    "Kira Blomqvist",
    "Luca Ferrari",
    "Maya Sundaram",
    "Nils Aasen",
    "Omar Riaz",
    "Petra Novak",
    "Quentin Roy",
    "Rosa Iglesias",
    "Sami Ahmed",
    "Tove Karlsen",
    "Ugo Bianchi",
    "Vera Pavlova",
    "Wes Duncan",
    "Xenia Adamou",
    "Yusuf Demir",
];

static USERS: LazyLock<Vec<User>> = LazyLock::new(|| {
    NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| User {
            id: format!("u_{:03}", index + 1),
            name: (*name).to_owned(),
            card_number: format!("4000{}", 1000 + index),
            pin_hash: pin_hash_of("1234"),
        })
        .collect()
});

/// A tiny linear congruential generator, so the fixtures are the same on every
/// machine on every run. A workshop where
/// your numbers differ from the numbers in the README is a workshop that
/// teaches you to distrust the README.
fn opening_accounts() -> Vec<Account> {
    let mut state: u32 = 20_260_904;
    let mut random = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(state) / 4_294_967_296.0
    };
    users()
        .iter()
        .enumerate()
        .map(|(index, user)| Account {
            id: format!("acc_{:03}", index + 1),
            user_id: user.id.clone(),
            balance_pence: 5_000 + (random() * 120_000.0).floor() as i64,
            daily_limit_pence: 30_000,
            withdrawn_today_pence: 0,
            // Accounts 7 and 19 never made it off the old cluster.
            shard: if index == 6 || index == 18 {
                "legacy-core"
            } else {
                "core"
            }
            .to_owned(),
        })
        .collect()
}

pub(crate) static ACCOUNTS: LazyLock<Mutex<Vec<Account>>> =
    LazyLock::new(|| Mutex::new(opening_accounts()));

/// The twenty-five customers, the same on every run.
pub fn users() -> &'static [User] {
    &USERS
}

/// Undo any debits an earlier run in this process left behind.
pub fn reset_balances() {
    *ACCOUNTS.lock().expect("accounts mutex") = opening_accounts();
}
