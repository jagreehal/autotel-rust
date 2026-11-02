use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

pub type Attributes = BTreeMap<String, Value>;

#[derive(Clone, Debug)]
pub struct EventPayload {
    pub kind: String,
    pub name: String,
    pub attributes: Attributes,
    pub timestamp: SystemTime,
}

pub trait EventSubscriber: Send + Sync {
    fn name(&self) -> &str;
    fn send(&self, payload: &EventPayload);
}

pub type Subscriber = Arc<dyn EventSubscriber>;

/// Appends each event to a file as one JSON object per line.
pub struct FileSubscriber {
    file: Mutex<File>,
}

impl FileSubscriber {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            file: Mutex::new(OpenOptions::new().create(true).append(true).open(path)?),
        })
    }
}

impl EventSubscriber for FileSubscriber {
    fn name(&self) -> &str {
        "file"
    }

    fn send(&self, payload: &EventPayload) {
        let millis = payload
            .timestamp
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        let line = json!({
            "type": payload.kind,
            "name": payload.name,
            "attributes": payload.attributes,
            "timestamp": millis,
        });
        // A failed write drops the event so the request still succeeds.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "event delivery is best-effort"
        )]
        let _ = writeln!(self.file.lock().expect("file mutex"), "{line}");
    }
}

/// An error with something for everyone: `message` for the customer, `why` and
/// `fix` for whoever helps them, `status` for the code that branches on it.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct StructuredError {
    pub message: String,
    pub status: u16,
    pub why: String,
    pub fix: String,
    pub link: Option<String>,
}

impl StructuredError {
    pub fn new(
        message: impl Into<String>,
        status: u16,
        why: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            status,
            why: why.into(),
            fix: fix.into(),
            link: None,
        }
    }

    #[must_use]
    pub fn with_link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }
}
