//! Ergonomic, production-minded OpenTelemetry instrumentation for Rust.

mod analysis;
#[cfg(feature = "axum")]
mod axum_layer;
mod core;
mod events;
mod genai;
mod instrumentation;
mod instruments;
mod logs;
mod propagation;
mod sampling;
mod slo;
#[cfg(feature = "sqlx")]
mod sqlx_support;
mod tail_sampling;
mod utilities;
mod validation;

pub use analysis::*;
pub use core::*;
pub use events::*;
pub use genai::*;
pub use instrumentation::*;
pub use instruments::register_runtime_metrics;
pub use logs::*;
pub use propagation::*;
pub use sampling::*;
pub use slo::*;
#[cfg(feature = "sqlx")]
pub use sqlx_support::{acquire, register_pool_metrics, with_transaction};
pub use tail_sampling::*;
pub use utilities::*;
pub use validation::*;

#[cfg(feature = "axum")]
pub mod axum {
    //! Axum middleware: extract W3C context and start a SERVER HTTP span.
    pub use crate::axum_layer::{AutotelLayer, AutotelMiddleware, layer, middleware};
}

pub use opentelemetry::{KeyValue, context::FutureExt, trace::SpanKind};
