use super::{
    BankError, BankErrorCode, User,
    data::{pin_hash_of, users},
    sleep,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

static FAILED_ATTEMPTS: LazyLock<Mutex<HashMap<String, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Look a customer up by the card they pushed into the slot.
pub async fn find_user_by_card(card_number: &str) -> Result<User, BankError> {
    sleep(15).await;
    users()
        .iter()
        .find(|candidate| candidate.card_number == card_number)
        .cloned()
        .ok_or_else(|| {
            let last4 = &card_number[card_number.len().saturating_sub(4)..];
            BankError::new(
                BankErrorCode::CardNotRecognised,
                "Card not recognised",
                vec![("card.last4", json!(last4))],
            )
        })
}

/// Three wrong PINs and the machine keeps the card. The counter is per card
/// and survives between withdrawals, which is why a customer can be locked out
/// by something that happened ten minutes ago in a different request.
pub async fn verify_pin(card_number: &str, pin: &str) -> Result<User, BankError> {
    sleep(5).await;
    let mut failed = FAILED_ATTEMPTS.lock().expect("attempts mutex");
    let attempts = failed.get(card_number).copied().unwrap_or(0);

    if attempts >= 3 {
        return Err(BankError::new(
            BankErrorCode::CardRetained,
            "Card retained",
            vec![("auth.attempts", json!(attempts))],
        ));
    }

    let user = users()
        .iter()
        .find(|candidate| candidate.card_number == card_number);

    match user {
        Some(user) if user.pin_hash == pin_hash_of(pin) => {
            failed.remove(card_number);
            Ok(user.clone())
        }
        _ => {
            let next = attempts + 1;
            failed.insert(card_number.to_owned(), next);
            let (code, message) = if next >= 3 {
                (BankErrorCode::CardRetained, "Card retained")
            } else {
                (BankErrorCode::PinIncorrect, "PIN incorrect")
            };
            Err(BankError::new(
                code,
                message,
                vec![
                    ("auth.attempts", json!(next)),
                    ("auth.attempts_remaining", json!(3u32.saturating_sub(next))),
                ],
            ))
        }
    }
}

pub fn reset_auth_attempts() {
    FAILED_ATTEMPTS.lock().expect("attempts mutex").clear();
}
