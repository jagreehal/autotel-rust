//! `with_tracing_subscriber` + `set_log_filter`: change log levels without a redeploy.
//! Own process: the global `tracing` subscriber can be installed only once.

use autotel::{AutotelConfig, set_log_filter};

#[test]
fn set_log_filter_reloads_the_installed_subscriber() {
    assert!(
        set_log_filter("debug").is_err(),
        "no autotel subscriber installed yet"
    );
    let _runtime =
        autotel::init(AutotelConfig::new("log-filter").with_tracing_subscriber()).unwrap();
    set_log_filter("warn").unwrap();
    assert!(!tracing::enabled!(tracing::Level::INFO));
    set_log_filter("info,orders=debug").unwrap();
    assert!(tracing::enabled!(tracing::Level::INFO));
    assert!(tracing::enabled!(target: "orders", tracing::Level::DEBUG));
    assert!(!tracing::enabled!(target: "payments", tracing::Level::DEBUG));
    assert!(set_log_filter("info,[bad").is_err());
}
