//! OpenTelemetry for Rust with one line of setup.
//!
//! [`init`] builds the exporters, providers and `tracing` bridges, and [`config!`] reads
//! the service name and version from `Cargo.toml`. The guard [`init`] returns flushes on
//! drop.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
//! # Ok(())
//! # }
//! ```
//!
//! Set `OTEL_EXPORTER_OTLP_ENDPOINT` to export. Wrap work in [`trace`], and add
//! `autotel::axum::layer()` or `autotel::tonic::layer()` to a server.

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
#[cfg(any(feature = "axum", feature = "tonic"))]
mod server_attrs;
mod slo;
#[cfg(feature = "sqlx")]
mod sqlx_support;
mod tail_sampling;
#[cfg(feature = "tonic")]
mod tonic_layer;
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

#[cfg(feature = "tonic")]
pub mod tonic {
    //! Tower layer that records gRPC SERVER spans for tonic services.
    pub use crate::tonic_layer::{AutotelLayer, AutotelMiddleware, TracedBody, layer};
}

pub use opentelemetry::{KeyValue, context::FutureExt, trace::SpanKind};
