//! Starts and stops the two service processes.
//!
//! Nothing here teaches observability; it is the plumbing that makes a
//! three-process example runnable with one command. Parts 11 and 12 and both
//! services pull it in with `#[path]`, and each uses only some of it.
#![allow(dead_code)]

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bank::{Account, BankError, Receipt};
use futures::FutureExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use std::{
    env,
    net::TcpListener,
    panic::AssertUnwindSafe,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use tokio::sync::Notify;

static CHILDREN: Mutex<Vec<Child>> = Mutex::new(Vec::new());

/// Refuse to start on a port something else already holds. Without this, a
/// service left running by an earlier run answers the health check, the new one
/// dies silently, and you spend twenty minutes debugging the code you just
/// changed while talking to the version you replaced.
fn require_free_port(port: u16) -> anyhow::Result<()> {
    TcpListener::bind(("127.0.0.1", port))
        .map(drop)
        .map_err(|_| {
            anyhow::anyhow!(
                "port {port} is already in use. A service from an earlier run is probably \
             still up: kill it with `lsof -ti tcp:{port} | xargs kill`."
            )
        })
}

async fn wait_for_port(port: u16) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        let up = client
            .get(format!("http://127.0.0.1:{port}/ping"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success());
        if up {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("service on :{port} never came up")
}

/// Build the two services in `directory` (`"services"` or
/// `"services-propagated"`) so they match the source, then start them.
pub async fn start_services(
    directory: &str,
    auth_port: u16,
    accounts_port: u16,
) -> anyhow::Result<()> {
    require_free_port(auth_port)?;
    require_free_port(accounts_port)?;

    let suffix = directory
        .strip_prefix("services")
        .ok_or_else(|| anyhow::anyhow!("no services in {directory}"))?;
    let auth = format!("p11-auth{suffix}");
    let accounts = format!("p11-accounts{suffix}");

    let built = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build", "-q", "-p", "workshop", "--bin", &auth, "--bin", &accounts,
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()?;
    anyhow::ensure!(built.success(), "the services in {directory} do not build");

    // The services inherit this process's environment, so with
    // OTEL_EXPORTER_OTLP_ENDPOINT set they export to the same devtools.
    let exe = env::current_exe()?;
    for (bin, port) in [(auth, auth_port), (accounts, accounts_port)] {
        let child = Command::new(exe.with_file_name(bin))
            .arg(port.to_string())
            .stdin(Stdio::null())
            .spawn()?;
        CHILDREN.lock().expect("children mutex").push(child);
    }

    wait_for_port(auth_port).await?;
    wait_for_port(accounts_port).await
}

/// Ask each service to flush its spans and exit, rather than killing it.
pub async fn stop_services(ports: &[u16]) {
    let client = reqwest::Client::new();
    for port in ports {
        let _ = client
            .get(format!("http://127.0.0.1:{port}/shutdown"))
            .send()
            .await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    for mut child in CHILDREN.lock().expect("children mutex").drain(..) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// What each route replies with. The services in this folder write exactly these.
#[derive(Serialize, Deserialize)]
pub struct CardReply {
    pub id: String,
    pub name: String,
}

pub type AccountReply = Account;

pub type DebitReply = Receipt;

/// The refusal both services send on a non-2xx.
#[derive(Default, Deserialize)]
struct RefusalReply {
    code: Option<String>,
    message: Option<String>,
}

/// Turn the structured refusal the service sent into an error, when the
/// response is not 2xx.
pub async fn ok_or_throw(response: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    // Each service writes every non-2xx as { code, message }; the default
    // covers a body that is not JSON at all.
    let body: RefusalReply = response.json().await.unwrap_or_default();

    anyhow::bail!(
        "{}: {}",
        body.code.unwrap_or_else(|| status.as_u16().to_string()),
        body.message.as_deref().unwrap_or("unknown")
    )
}

/// `ok_or_throw`, then the body, as the shape the fetched route writes.
pub async fn json_or_throw<T: DeserializeOwned>(response: reqwest::Response) -> anyhow::Result<T> {
    Ok(ok_or_throw(response).await?.json().await?)
}

/// The services' catch block: a bank refusal becomes a 400, anything else
/// (a panic, such as `get_account` finding no account) a 500, both as
/// `{ code, message }`.
pub async fn reply<T: IntoResponse>(work: impl Future<Output = Result<T, BankError>>) -> Response {
    match AssertUnwindSafe(work).catch_unwind().await {
        Ok(Ok(body)) => body.into_response(),
        Ok(Err(error)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "code": error.code.as_str(), "message": error.message })),
        )
            .into_response(),
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                .unwrap_or_default();
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "code": "UNEXPECTED", "message": message })),
            )
                .into_response()
        }
    }
}

static STOP: Notify = Notify::const_new();

/// `GET /shutdown`: reply, then let the server finish. `main` returns, the
/// autotel guard drops and flushes, and the process exits.
pub async fn shutdown() -> &'static str {
    STOP.notify_one();
    "bye"
}

/// Resolves once `/shutdown` has been called. Pass it to `with_graceful_shutdown`.
pub async fn shutdown_requested() {
    STOP.notified().await;
}
