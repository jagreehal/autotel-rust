//! autotel version of `logs-basic`: `tracing` events become OpenTelemetry logs.
fn main() -> anyhow::Result<()> {
    // Resource, provider, bridge, filters and console output in one line.
    // The guard flushes and shuts down when `main` returns.
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;

    tracing::error!(name: "my-event-name", target: "my-system", event_id = 20, user_name = "otel", user_email = "otel@opentelemetry.io", message = "This is an example message");
    Ok(())
}
