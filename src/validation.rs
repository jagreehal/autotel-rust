use crate::{Attributes, StructuredError, TraceContext};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug)]
pub struct ValidationIssue {
    pub path: String,
    pub code: String,
    pub expected: String,
}
#[derive(Clone, Debug)]
pub struct ValidationMismatch {
    pub validator: String,
    pub boundary: String,
    pub issues: Vec<ValidationIssue>,
}

pub fn require<T>(
    validator: &str,
    boundary: &str,
    value: T,
    predicate: impl FnOnce(&T) -> bool,
    issue: ValidationIssue,
    on_mismatch: impl FnOnce(&ValidationMismatch),
) -> Result<T, StructuredError> {
    if predicate(&value) {
        return Ok(value);
    }
    let mismatch = ValidationMismatch {
        validator: validator.into(),
        boundary: boundary.into(),
        issues: vec![issue.clone()],
    };
    on_mismatch(&mismatch);
    Err(StructuredError::new(
        format!("Validation failed for {validator}"),
        400,
        "Production input contradicted the declared contract",
        format!("Correct the field at {}", issue.path),
    ))
}

/// One wide event per request: every `set` lands on the active span as
/// flattened attributes (`{"user": {"id": "u_1"}}` becomes `user.id`), and
/// `emit_now` closes the record.
pub struct RequestLogger {
    context: TraceContext,
    fields: Mutex<Value>,
    emitted: AtomicBool,
}

/// The request logger for the active span. Panics outside `trace`.
pub fn request_logger() -> RequestLogger {
    RequestLogger::new(crate::ctx())
}

impl RequestLogger {
    pub fn new(context: TraceContext) -> Self {
        Self {
            context,
            fields: Mutex::new(Value::Object(Map::new())),
            emitted: AtomicBool::new(false),
        }
    }

    /// Merge nested fields into the event. Ignored once the event is emitted.
    pub fn set(&self, fields: Value) -> &Self {
        if !self.emitted.load(Ordering::Relaxed) {
            merge(&mut self.fields.lock().expect("logger mutex"), &fields);
            self.context.set_attributes(flatten(&fields));
        }
        self
    }

    /// Write the event once, as a `log.emit.manual` span event with every field.
    pub fn emit_now(&self) -> Attributes {
        let flattened = flatten(&self.snapshot());
        if !self.emitted.swap(true, Ordering::Relaxed) {
            self.context.set_attributes(flattened.clone());
            self.context.add_event("log.emit.manual", flattened.clone());
        }
        flattened
    }

    pub fn snapshot(&self) -> Value {
        self.fields.lock().expect("logger mutex").clone()
    }
}

fn merge(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source.clone(),
    }
}

fn flatten(value: &Value) -> Attributes {
    fn walk(prefix: &str, value: &Value, output: &mut Attributes) {
        match value {
            Value::Object(values) => {
                for (key, nested) in values {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(&path, nested, output);
                }
            }
            leaf => {
                output.insert(prefix.to_owned(), leaf.clone());
            }
        }
    }
    let mut output = BTreeMap::new();
    walk("", value, &mut output);
    output
}

pub fn user_attributes(id: &str, name: &str, roles: &[&str]) -> Attributes {
    Attributes::from([
        ("user.id".into(), Value::String(id.into())),
        ("user.name".into(), Value::String(name.into())),
        (
            "user.roles".into(),
            Value::Array(
                roles
                    .iter()
                    .map(|role| Value::String((*role).into()))
                    .collect(),
            ),
        ),
    ])
}

pub fn safe_set(context: &TraceContext, values: Attributes, redact_pii: bool) {
    for (key, value) in values {
        if redact_pii
            && ["email", "card"]
                .iter()
                .any(|part| key.to_ascii_lowercase().contains(part))
        {
            context.set_attribute(key, "[REDACTED]");
        } else {
            context.set_attribute(key, value);
        }
    }
}
