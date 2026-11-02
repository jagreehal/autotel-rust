//! The domain fails with plain errors carrying a machine-readable `code`.
//!
//! Part 7 is about what happens at the boundary: the same codes, turned into
//! something a customer, a call-centre agent and an on-call engineer can each
//! act on without reading this file.

use serde_json::Value;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BankErrorCode {
    CardNotRecognised,
    PinIncorrect,
    CardRetained,
    InsufficientFunds,
    DailyLimitExceeded,
    LedgerUnavailable,
}

impl BankErrorCode {
    /// The code as it appears in telemetry: `"DAILY_LIMIT_EXCEEDED"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CardNotRecognised => "CARD_NOT_RECOGNISED",
            Self::PinIncorrect => "PIN_INCORRECT",
            Self::CardRetained => "CARD_RETAINED",
            Self::InsufficientFunds => "INSUFFICIENT_FUNDS",
            Self::DailyLimitExceeded => "DAILY_LIMIT_EXCEEDED",
            Self::LedgerUnavailable => "LEDGER_UNAVAILABLE",
        }
    }
}

impl fmt::Display for BankErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct BankError {
    pub code: BankErrorCode,
    pub message: String,
    /// Facts about the refusal that are safe to record: attempts left, limits.
    pub detail: Vec<(&'static str, Value)>,
}

impl BankError {
    pub(crate) fn new(
        code: BankErrorCode,
        message: &str,
        detail: Vec<(&'static str, Value)>,
    ) -> Self {
        Self {
            code,
            message: message.to_owned(),
            detail,
        }
    }
}

impl fmt::Display for BankError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BankError {}
