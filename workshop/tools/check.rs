//! Runs every exercise and solution in the workshop and asserts that each one
//! still emits the spans its file says it emits.
//!
//! The workshop has no failing test suite by design: you learn by looking at a
//! trace, not by turning a suite green. That leaves the solutions with nothing
//! holding them up, so a change in autotel could break every part and the only
//! signal would be a learner's confusion three months from now. This tool is
//! that signal, and it stays invisible to the learner.
//!
//! The expectations live next to the code they describe, one per line:
//!
//!   // check: name = "atm.withdraw"
//!
//! Anything after `check:` is a devtools query, so the assertion is written in
//! the same language the workshop teaches you to type into the search box.
//! Two more forms cover the parts that emit more than spans:
//!
//!   // check-metric: atm.withdraw.duration   a metric with that name arrived
//!   // check-log: pin ok                     a log with that text, linked to a trace
//!
//!   cargo workshop-check        every part
//!   cargo workshop-check 07     only parts whose directory starts with 07
//!
//! It starts its own devtools on a free port, so one you left running is safe.
//! AUTOTEL_DEVTOOLS=/path/to/autotel-devtools/dist/cli.js runs a local build
//! instead of the published one, to check a devtools change before release.

use serde_json::{Value, json};
use std::{
    env, fs,
    net::TcpListener,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DEVTOOLS: &str = "autotel-devtools@31.4.3";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Every `// <name>: value` line in a file, in order.
fn directives(source: &str, name: &str) -> Vec<String> {
    let marker = format!("{name}:");
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("//")?.trim_start();
            Some(rest.strip_prefix(&marker)?.trim().to_owned())
        })
        .collect()
}

struct Devtools {
    base: String,
    child: Child,
}

impl Drop for Devtools {
    fn drop(&mut self) {
        // npx runs devtools as a grandchild, so stop the whole process group.
        let _ = Command::new("kill")
            .args(["-TERM", &format!("-{}", self.child.id())])
            .status();
        let _ = self.child.wait();
    }
}

async fn start_devtools(client: &reqwest::Client) -> anyhow::Result<Devtools> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let ports = [
        "--port".to_owned(),
        port.to_string(),
        "--grpc-port".to_owned(),
        (port + 1).to_string(),
    ];
    let mut command = match env::var("AUTOTEL_DEVTOOLS") {
        Ok(cli) => {
            let mut command = Command::new("node");
            command.arg(cli);
            command
        }
        Err(_) => {
            let mut command = Command::new("npx");
            command.args(["-y", DEVTOOLS]);
            command
        }
    };
    let child = command
        .args(ports)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let devtools = Devtools {
        base: format!("http://127.0.0.1:{port}"),
        child,
    };
    // The first npx run downloads devtools, so allow it a while.
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let up = client
            .get(format!("{}/healthz", devtools.base))
            .timeout(Duration::from_millis(500))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success());
        if up {
            return Ok(devtools);
        }
    }
    anyhow::bail!("autotel-devtools never came up on {}", devtools.base)
}

async fn query(client: &reqwest::Client, url: String, body: Value) -> anyhow::Result<Value> {
    Ok(client.post(url).json(&body).send().await?.json().await?)
}

async fn trace_matched(
    client: &reqwest::Client,
    base: &str,
    query_text: &str,
    window: (u64, u64),
) -> anyhow::Result<bool> {
    let body = query(
        client,
        format!("{base}/api/query/traces"),
        json!({ "query": query_text, "window": { "start": window.0, "end": window.1 }, "limit": 500 }),
    )
    .await?;
    if let Some(errors) = body.get("errors") {
        anyhow::bail!("bad query {query_text}: {errors}");
    }
    Ok(body["traces"]
        .as_array()
        .is_some_and(|traces| !traces.is_empty()))
}

async fn metric_arrived(
    client: &reqwest::Client,
    base: &str,
    name: &str,
    window: (u64, u64),
) -> anyhow::Result<bool> {
    let body = query(
        client,
        format!("{base}/api/query/metrics"),
        json!({ "name": name }),
    )
    .await?;
    Ok(body["series"].as_array().is_some_and(|series| {
        series.iter().any(|series| {
            series["points"].as_array().is_some_and(|points| {
                points
                    .iter()
                    .any(|point| point["timestamp"].as_f64().unwrap_or(0.0) >= window.0 as f64)
            })
        })
    }))
}

async fn linked_log_arrived(
    client: &reqwest::Client,
    base: &str,
    text: &str,
    window: (u64, u64),
) -> anyhow::Result<bool> {
    let body = query(
        client,
        format!("{base}/api/query/logs"),
        json!({ "query": "", "limit": 500 }),
    )
    .await?;
    Ok(body["logs"].as_array().is_some_and(|logs| {
        logs.iter().any(|log| {
            log["timestamp"].as_f64().unwrap_or(0.0) >= window.0 as f64
                && log["traceId"].as_str().is_some_and(|id| !id.is_empty())
                && log["body"].to_string().contains(text)
        })
    }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let workshop = Path::new(env!("CARGO_MANIFEST_DIR"));
    let filter = env::args().nth(1).unwrap_or_default();
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    let built = Command::new(&cargo)
        .args(["build", "-q", "-p", "workshop", "--bins"])
        .status()?;
    anyhow::ensure!(built.success(), "the workshop does not build");
    let bin_dir = env::current_exe()?
        .parent()
        .expect("binaries live in a directory")
        .to_path_buf();

    let client = reqwest::Client::new();
    let devtools = start_devtools(&client).await?;
    let base = devtools.base.clone();

    let mut parts: Vec<String> = fs::read_dir(workshop)?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.len() > 3 && name[..2].bytes().all(|b| b.is_ascii_digit()) && &name[2..3] == "-"
        })
        .filter(|name| name.starts_with(&filter))
        .collect();
    parts.sort();

    let mut failures = 0;
    for part in &parts {
        for (kind, alias) in [("exercise", "ex"), ("solution", "sol")] {
            let file = workshop.join(part).join(format!("{kind}.rs"));
            let Ok(source) = fs::read_to_string(&file) else {
                continue;
            };
            let label = format!("{part}/{kind}");
            let start = now_ms() - 1000;
            let output = Command::new(bin_dir.join(format!("{alias}{}", &part[..2])))
                .env("OTEL_EXPORTER_OTLP_ENDPOINT", &base)
                .stdin(Stdio::null())
                .output()?;
            let end = now_ms() + 1000;

            if !output.status.success() {
                failures += 1;
                eprintln!(
                    "✗ {label} exited {:?}\n{}{}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                continue;
            }

            // Give the exporter's last flush time to land in the store.
            tokio::time::sleep(Duration::from_millis(300)).await;

            let mut ok = true;
            for expected in directives(&source, "check") {
                if !trace_matched(&client, &base, &expected, (start, end)).await? {
                    ok = false;
                    failures += 1;
                    eprintln!("✗ {label}: no trace matched `{expected}`");
                }
            }
            for name in directives(&source, "check-metric") {
                if !metric_arrived(&client, &base, &name, (start, end)).await? {
                    ok = false;
                    failures += 1;
                    eprintln!("✗ {label}: no metric named `{name}` arrived");
                }
            }
            for text in directives(&source, "check-log") {
                if !linked_log_arrived(&client, &base, &text, (start, end)).await? {
                    ok = false;
                    failures += 1;
                    eprintln!("✗ {label}: no log containing `{text}` arrived with a trace id");
                }
            }
            if ok {
                println!("✓ {label}");
            }
        }
    }

    drop(devtools);

    if failures > 0 {
        eprintln!("\n{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("\nAll parts run and emit what they claim to emit.");
    Ok(())
}
