//! Part 7: Our solution: one translation at the boundary
//!
//!   cargo sol 07

// check: name = "atm.withdraw"
// check: withdrawal.refusal_code = "CARD_RETAINED"
// check: status = ERROR
use autotel::{AutotelConfig, StructuredError, request_logger, trace_result};
use bank::{
    BankError, BankErrorCode, Receipt, debit, find_user_by_card, get_account, reset_auth_attempts,
    reset_balances, users, verify_pin,
};
use serde_json::{Map, Value, json};

/// One table, one row per code, written once. `why` explains, `fix` is the
/// next action, `status` is what a caller branches on, `link` is where the
/// agent on the phone goes. None of them names a balance, a PIN or a card
/// number. The match has no `_` arm, so a new code will not compile until it
/// has a row.
fn refusal(code: BankErrorCode) -> StructuredError {
    let (message, why, fix, status, link) = match code {
        BankErrorCode::CardNotRecognised => (
            "This card cannot be used here",
            "The card number is not one this network recognises.",
            "Check the card is the right way round, or use a machine on your own network.",
            404,
            "https://bank.example/help/card-not-recognised",
        ),
        BankErrorCode::PinIncorrect => (
            "Incorrect PIN",
            "The PIN entered does not match this card.",
            "Try again. After three wrong attempts the machine keeps the card.",
            401,
            "https://bank.example/help/pin",
        ),
        BankErrorCode::CardRetained => (
            "Card retained",
            "Three incorrect PINs were entered on this card.",
            "Call the number on your statement to have a replacement sent.",
            423,
            "https://bank.example/help/card-retained",
        ),
        BankErrorCode::InsufficientFunds => (
            "Not enough in the account",
            "The amount requested is more than the available balance.",
            "Try a smaller amount, or check for pending transactions in the app.",
            402,
            "https://bank.example/help/balance",
        ),
        BankErrorCode::DailyLimitExceeded => (
            "Daily limit reached",
            "This amount would take today’s total past the £300 daily cash limit.",
            "The limit resets at midnight. Larger amounts can be arranged in branch.",
            429,
            "https://bank.example/help/daily-limit",
        ),
        BankErrorCode::LedgerUnavailable => (
            "The machine cannot reach your bank",
            "The ledger did not answer in time. No money has left the account.",
            "Wait a minute and try again. Nothing was debited.",
            503,
            "https://bank.example/help/try-again",
        ),
    };
    StructuredError::new(message, status, why, fix).with_link(link)
}

async fn withdraw(
    card_number: &str,
    pin: &str,
    amount_pence: i64,
) -> Result<Receipt, StructuredError> {
    trace_result("atm.withdraw", |_| async move {
        let log = request_logger();
        log.set(json!({ "withdrawal": { "amount_pence": amount_pence } }));

        let result: Result<Receipt, BankError> = async {
            let user =
                trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
            log.set(json!({ "user": { "id": user.id } }));

            trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;

            let account = autotel::trace("bank.get_account", |_| get_account(&user.id)).await;
            let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
            log.set(json!({ "withdrawal": { "result": "dispensed" } }));

            Ok(receipt)
        }
        .await;

        if let Err(error) = &result {
            log.set(json!({
                "withdrawal": { "result": "refused", "refusal_code": error.code.as_str() }
            }));
            // The domain's own detail (attempts remaining, limits) is safe to
            // keep in telemetry and is exactly what the on-call engineer wants.
            let detail: Map<String, Value> = error
                .detail
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect();
            log.set(Value::Object(detail));
        }
        log.emit_now();

        result.map_err(|error| refusal(error.code))
    })
    .await
}

/// The screen shows the typed error the caller got back: no parsing, the
/// fields are already there.
fn screen(error: &StructuredError) {
    let row = |text: &str| {
        let text: String = text.chars().take(56).collect();
        format!("│ {text:<56} │")
    };
    let rule = "─".repeat(58);

    println!("┌{rule}┐");
    println!("{}", row(&error.message));
    println!("├{rule}┤");
    for text in [&error.why, &error.fix] {
        if !text.is_empty() {
            println!("{}", row(text));
        }
    }
    println!("└{rule}┘");
    println!(
        "  status {}  ·  {}\n",
        error.status,
        error.link.as_deref().unwrap_or_default()
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    reset_balances();
    reset_auth_attempts();

    let card = users()[1].card_number.as_str();

    let scenarios = [
        ("an unknown card", "4000999999", "1234", 10_000),
        ("a wrong PIN", card, "9999", 10_000),
        ("a second wrong PIN", card, "9999", 10_000),
        (
            "a third wrong PIN, and the machine keeps the card",
            card,
            "9999",
            10_000,
        ),
        (
            "more than the balance",
            users()[2].card_number.as_str(),
            "1234",
            999_999,
        ),
        (
            "over the daily limit",
            users()[3].card_number.as_str(),
            "1234",
            40_000,
        ),
    ];

    for (label, card_number, pin, amount) in scenarios {
        println!("\n── {label} ──");

        if let Err(error) = withdraw(card_number, pin, amount).await {
            screen(&error);
        }
    }

    println!(
        "Every refusal above is one arm in refusal(), one Err, and one
event in devtools. Try:

  withdrawal.result = \"refused\"
  status = ERROR"
    );

    Ok(())
}
