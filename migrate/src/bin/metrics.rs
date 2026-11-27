//! autotel version of `metrics-basic`.
use autotel::KeyValue;

fn main() -> anyhow::Result<()> {
    // Sets the global meter provider; the guard flushes metrics on drop,
    // so there is no 60-second wait and no explicit shutdown.
    let _telemetry = autotel::init(autotel::config!())?;
    let attributes = [
        KeyValue::new("mykey1", "myvalue1"),
        KeyValue::new("mykey2", "myvalue2"),
    ];

    // One line per measurement.
    autotel::counter_with("my_counter", 10, &attributes);
    autotel::up_down_counter_with("my_updown_counter", -10, &attributes);
    autotel::histogram_with("my_histogram", 10.5, "ms", &attributes);
    autotel::gauge_with("my_gauge", 1.0, "myunit", &attributes);

    // Observable instruments: the callback runs at each export.
    autotel::observe_counter("my_observable_counter", "myunit", &attributes, || 100);
    autotel::observe_up_down_counter(
        "my_observable_updown_counter",
        "myunit",
        &attributes,
        || 100,
    );
    autotel::observe_gauge("my_observable_gauge", "myunit", &attributes, || 1.0);
    Ok(())
}
