//! Part 12: Our solution: put the fact in baggage, once
//!
//!   cargo sol 12

// check: name = "atm.withdraw"
// check: service = accounts-service and atm.branch = "canada-water"
// check: autotel.debug = "part-10"
#[path = "../11-one-trace-three-services/harness.rs"]
mod harness;

use autotel::{
    AUTOTEL_DEBUG_BAGGAGE_KEY, AutotelConfig, inject_trace_context, request_logger, trace_result,
};
use axum::http::HeaderMap;
use bank::{reset_balances, users};
use harness::{
    AccountReply, CardReply, DebitReply, json_or_throw, ok_or_throw, start_services, stop_services,
};
use serde_json::json;

const AUTH: &str = "http://127.0.0.1:7401";

const ACCOUNTS: &str = "http://127.0.0.1:7402";

struct Machine {
    id: &'static str,
    branch: &'static str,
}

const MACHINES: [Machine; 2] = [
    Machine {
        id: "atm_0431",
        branch: "bridge-street",
    },
    Machine {
        id: "atm_1330",
        branch: "canada-water",
    },
];

async fn withdraw(
    machine: &Machine,
    card_number: &str,
    amount_pence: i64,
    debug_label: Option<&str>,
) -> anyhow::Result<(DebitReply, HeaderMap)> {
    trace_result("atm.withdraw", |ctx| async move {
        let log = request_logger();

        // Baggage, not an attribute. An attribute stays on this span; baggage
        // rides the `baggage` header to every service this request reaches, and
        // `.baggage("")` in each init() copies it onto their spans too.
        ctx.set_baggage("atm.id", machine.id);
        ctx.set_baggage("atm.branch", machine.branch);

        // Full fidelity for one request, decided per request rather than per deploy.
        if let Some(label) = debug_label {
            ctx.set_baggage(AUTOTEL_DEBUG_BAGGAGE_KEY, label);
        }

        log.set(json!({ "withdrawal": { "amount_pence": amount_pence } }));

        // inject_trace_context writes `traceparent` and `baggage` together.
        let headers = inject_trace_context();
        let http = reqwest::Client::new();

        let user: CardReply = json_or_throw(
            http.post(format!("{AUTH}/card"))
                .headers(headers.clone())
                .json(&json!({ "card": card_number }))
                .send()
                .await?,
        )
        .await?;

        ok_or_throw(
            http.post(format!("{AUTH}/verify"))
                .json(&json!({ "card": card_number, "pin": "1234" }))
                .headers(headers.clone())
                .send()
                .await?,
        )
        .await?;

        let account: AccountReply = json_or_throw(
            http.get(format!("{ACCOUNTS}/account?userId={}", user.id))
                .headers(headers.clone())
                .send()
                .await?,
        )
        .await?;

        let receipt: DebitReply = json_or_throw(
            http.post(format!(
                "{ACCOUNTS}/debit?userId={}&amountPence={amount_pence}",
                user.id
            ))
            .headers(headers.clone())
            .send()
            .await?,
        )
        .await?;

        log.set(json!({
            "account": { "id": account.id },
            "withdrawal": { "result": "dispensed" },
        }));
        log.emit_now();

        Ok((receipt, headers))
    })
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(
        AutotelConfig::new("atm")
            .devtools()
            // Copy baggage onto spans with no prefix, the same as the two services do.
            .baggage(""),
    )?;

    reset_balances();
    start_services("services-propagated", 7401, 7402).await?;

    let mut last_headers = HeaderMap::new();

    for (index, user) in users().iter().take(6).enumerate() {
        let (_receipt, headers) = withdraw(
            &MACHINES[index % 2],
            &user.card_number,
            5_000,
            // One withdrawal is marked for full fidelity. In production this
            // comes from a gateway, a feature flag, or a curl with an extra header.
            (index == 3).then_some("part-10"),
        )
        .await?;

        last_headers = headers;
    }

    stop_services(&[7401, 7402]).await;
    autotel::shutdown();

    let header = |name: &str| {
        last_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };

    println!(
        "
The headers on the last hop:

  traceparent: {}
  baggage:     {}

Now the query the exercise could not answer:

  service = accounts-service and atm.branch = \"canada-water\"

And the withdrawal that asked to be kept whatever the sampler decides:

  autotel.debug = \"part-10\"",
        header("traceparent"),
        header("baggage"),
    );

    Ok(())
}
