//! Part 6: the setup, in one place, run before the app does anything else.
//!
//! A process has one global `tracing` subscriber, built once. Every layer
//! that should see a log line has to be in it when it is built, and the log
//! bridge can only be built once autotel::init() has made somewhere to send
//! the lines. So the order is fixed: init, then the subscriber, then the app.

use autotel::{AutotelConfig, AutotelRuntime};
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Call once, on the first line of main, and keep the guard it returns.
pub fn setup() -> anyhow::Result<AutotelRuntime> {
    // 1. The pipeline: where spans, and now log records, go.
    let telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

    // 2. The one subscriber, with every layer in it.
    tracing_subscriber::registry()
        // Same JSON lines on stdout, with the active span's ids stamped in.
        .with(fmt::layer().event_format(autotel::JsonWithTraceIds))
        // Each line also goes to devtools as a log record, linked to its span.
        .with(autotel::log_bridge())
        .with(LevelFilter::INFO)
        .init();

    Ok(telemetry)
}
