//! `OTEL_*` variables beat code. Each case re-runs this test binary with the variables
//! set, because the crate forbids the `unsafe` that `std::env::set_var` needs.
use autotel::{AutotelConfig, LogFile, OtlpProtocol};
use opentelemetry::Key;
use std::process::Command;

fn in_child(test: &str, env: &[(&str, &str)]) -> Option<bool> {
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        return None;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .env("AUTOTEL_ENV_CHILD", "1")
        .envs(env.iter().copied())
        .status()
        .unwrap();
    Some(status.success())
}

fn resource_value(config: &AutotelConfig, key: &'static str) -> String {
    config
        .resource()
        .get(&Key::from_static_str(key))
        .unwrap()
        .to_string()
}

#[test]
fn service_name_and_resource_attributes_beat_code() {
    let env = [
        ("OTEL_SERVICE_NAME", "from-env"),
        (
            "OTEL_RESOURCE_ATTRIBUTES",
            "service.version=9.9,deployment.environment.name=prod,team=a",
        ),
    ];
    if let Some(ok) = in_child("service_name_and_resource_attributes_beat_code", &env) {
        assert!(ok);
        return;
    }
    let config = AutotelConfig::new("from-code")
        .version("1.0")
        .environment("dev");
    assert_eq!(resource_value(&config, "service.name"), "from-env");
    assert_eq!(resource_value(&config, "service.version"), "9.9");
    assert_eq!(
        resource_value(&config, "deployment.environment.name"),
        "prod"
    );
    assert_eq!(resource_value(&config, "team"), "a");
}

#[test]
fn config_macro_reads_cargo_metadata() {
    let config = autotel::config!();
    assert_eq!(config.service_name, "autotel");
    assert_eq!(config.service_version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn protocol_variable_selects_protobuf() {
    let env = [
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "https://otlp.example.com/otlp",
        ),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
    ];
    if let Some(ok) = in_child("protocol_variable_selects_protobuf", &env) {
        assert!(ok);
        return;
    }
    let endpoints = AutotelConfig::new("x").otlp_endpoints().unwrap();
    assert_eq!(endpoints.protocol, OtlpProtocol::HttpProtobuf);
    assert_eq!(endpoints.traces, "https://otlp.example.com/otlp/v1/traces");
}

#[test]
fn unusable_protocol_fails_init_and_names_the_variable() {
    let env = [("OTEL_EXPORTER_OTLP_PROTOCOL", "carrier-pigeon")];
    if let Some(ok) = in_child("unusable_protocol_fails_init_and_names_the_variable", &env) {
        assert!(ok);
        return;
    }
    let error = autotel::init(AutotelConfig::new("x"))
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("OTEL_EXPORTER_OTLP_PROTOCOL=carrier-pigeon"),
        "{error}"
    );
}

#[test]
fn json_logs_carry_the_trace_id_of_the_active_span() {
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        let _telemetry =
            autotel::init(AutotelConfig::new("x").json_logs().capture_spans()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let id = runtime.block_on(autotel::trace("work", |ctx| async move {
            tracing::info!("inside");
            ctx.trace_id()
        }));
        println!("TRACE={id}");
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "json_logs_carry_the_trace_id_of_the_active_span",
            "--nocapture",
        ])
        .env("AUTOTEL_ENV_CHILD", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let id = stdout
        .split("TRACE=")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    let line = stdout.lines().find(|line| line.contains("inside")).unwrap();
    assert!(line.contains(&format!("\"trace_id\":\"{id}\"")), "{line}");
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("autotel-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn log_lines(dir: &std::path::Path) -> String {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|file| std::fs::read_to_string(file.unwrap().path()).unwrap())
        .collect()
}

/// In the child: log once inside a span, then drop the guard. Prints the trace id.
fn log_inside_a_span(config: AutotelConfig) {
    let telemetry = autotel::init(config).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let id = runtime.block_on(autotel::trace("work", |ctx| async move {
        tracing::info!("inside");
        autotel::counter("work.done", 1);
        ctx.trace_id()
    }));
    drop(telemetry);
    println!("TRACE={id}");
}

#[test]
fn log_file_gets_every_line_without_colors_once_the_guard_drops() {
    let dir = temp_dir("file");
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        let dir = std::env::var("AUTOTEL_TEST_DIR").unwrap();
        log_inside_a_span(
            AutotelConfig::new("x")
                .capture_spans()
                .log_file(LogFile::new(dir).prefix("orders")),
        );
        return;
    }
    let test = "log_file_gets_every_line_without_colors_once_the_guard_drops";
    let dir_text = dir.to_str().unwrap();
    assert_eq!(
        in_child(test, &[("AUTOTEL_TEST_DIR", dir_text)]),
        Some(true)
    );
    let logs = log_lines(&dir);
    assert!(logs.contains("inside"), "{logs:?}");
    assert!(!logs.contains('\u{1b}'), "colors in a file: {logs:?}");
    let name = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    assert!(name.to_str().unwrap().starts_with("orders."), "{name:?}");
    assert!(name.to_str().unwrap().ends_with(".log"), "{name:?}");
}

#[test]
fn log_file_variable_needs_no_code_and_json_files_carry_trace_ids() {
    let dir = temp_dir("env");
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        log_inside_a_span(
            AutotelConfig::new("x")
                .json_logs()
                .without_console_logs()
                .capture_spans(),
        );
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "log_file_variable_needs_no_code_and_json_files_carry_trace_ids",
            "--nocapture",
        ])
        .env("AUTOTEL_ENV_CHILD", "1")
        .env("AUTOTEL_LOG_FILE_DIR", &dir)
        .env("AUTOTEL_LOG_FILE_ROTATION", "never")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let id = stdout
        .split("TRACE=")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert!(
        !stdout.contains("inside"),
        "without_console_logs still printed: {stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("Error reading"), "startup noise: {stderr}");
    let logs = log_lines(&dir);
    assert!(logs.contains(&format!("\"trace_id\":\"{id}\"")), "{logs:?}");
}

#[test]
fn bad_log_file_settings_fail_init_and_name_the_cause() {
    let env = [
        ("AUTOTEL_LOG_FILE_DIR", "/tmp"),
        ("AUTOTEL_LOG_FILE_ROTATION", "weekly"),
    ];
    if let Some(ok) = in_child("bad_log_file_settings_fail_init_and_name_the_cause", &env) {
        assert!(ok);
        return;
    }
    let error = autotel::init(AutotelConfig::new("x"))
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("AUTOTEL_LOG_FILE_ROTATION=weekly"),
        "{error}"
    );
}

#[test]
fn unusable_log_directory_fails_init() {
    let file = temp_dir("not-a-dir");
    std::fs::write(&file, "").unwrap();
    let error = autotel::init(AutotelConfig::new("x").log_file(LogFile::new(&file).blocking()))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("can't log to"), "{error}");
}

/// Answers every request and reports the paths it saw on `paths`.
fn otlp_receiver() -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (paths, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if header.trim().is_empty() {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            paths.send(line).unwrap();
            let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
            reader.get_mut().write_all(reply).unwrap();
        }
    });
    (address, received)
}

/// The paths one export run reached, with `env` set on it.
fn exported_paths(test: &str, env: &[(&str, &str)]) -> String {
    let (address, paths) = otlp_receiver();
    let endpoint = format!("http://{address}");
    let mut env: Vec<(&str, String)> = env
        .iter()
        .map(|(key, value)| (*key, value.replace("{endpoint}", &endpoint)))
        .collect();
    env.push(("AUTOTEL_ENV_CHILD", "1".to_owned()));
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .envs(env.iter().map(|(key, value)| (*key, value.as_str())))
        .status()
        .unwrap();
    assert!(status.success());
    paths.try_iter().collect()
}

#[test]
fn exporter_none_silences_one_signal_and_leaves_the_others() {
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        log_inside_a_span(AutotelConfig::new("x").with_tracing_subscriber());
        return;
    }
    let test = "exporter_none_silences_one_signal_and_leaves_the_others";
    let base = ("OTEL_EXPORTER_OTLP_ENDPOINT", "{endpoint}");

    let all = exported_paths(test, &[base]);
    for path in ["/v1/traces", "/v1/metrics", "/v1/logs"] {
        assert!(all.contains(path), "control run lacks {path}: {all}");
    }

    let quiet = [
        base,
        ("OTEL_METRICS_EXPORTER", "none"),
        ("OTEL_LOGS_EXPORTER", "none"),
    ];
    let traces_only = exported_paths(test, &quiet);
    assert!(traces_only.contains("/v1/traces"), "{traces_only}");
    assert!(!traces_only.contains("/v1/metrics"), "{traces_only}");
    assert!(!traces_only.contains("/v1/logs"), "{traces_only}");
}

#[test]
fn one_per_signal_endpoint_exports_that_signal_alone() {
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        log_inside_a_span(AutotelConfig::new("x").with_tracing_subscriber());
        return;
    }
    let test = "one_per_signal_endpoint_exports_that_signal_alone";
    let traces = ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "{endpoint}/v1/traces");

    let paths = exported_paths(test, &[traces]);

    assert!(paths.contains("/v1/traces"), "{paths}");
    assert!(!paths.contains("/v1/metrics"), "{paths}");
}

#[test]
fn a_second_init_keeps_the_log_file_writing() {
    if std::env::var_os("AUTOTEL_ENV_CHILD").is_some() {
        let dir = std::env::var("AUTOTEL_TEST_DIR").unwrap();
        let first = autotel::init(AutotelConfig::new("x").log_file(LogFile::new(dir))).unwrap();
        let second = autotel::init(AutotelConfig::new("y").capture_spans()).unwrap();
        tracing::info!("after the second init");
        drop(second);
        drop(first);
        return;
    }
    let dir = temp_dir("second-init");
    let test = "a_second_init_keeps_the_log_file_writing";

    let ok = in_child(test, &[("AUTOTEL_TEST_DIR", dir.to_str().unwrap())]);

    assert_eq!(ok, Some(true));
    assert!(log_lines(&dir).contains("after the second init"));
}

#[test]
fn keeping_one_log_file_fails_init_because_rotation_would_delete_it() {
    let dir = temp_dir("one-file");
    let config = AutotelConfig::new("x").log_file(LogFile::new(&dir).max_files(1));

    let error = autotel::init(config).err().unwrap().to_string();

    assert!(error.contains("max_files"), "{error}");
}
