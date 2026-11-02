use crate::{
    Attributes, TraceContext, current_context, inject_trace_context, instruments, trace, trace_kind,
};
use opentelemetry::trace::SpanKind;
use serde_json::Value;
use std::{future::Future, time::Instant};

async fn semantic<T, F, Fut>(name: String, kind: SpanKind, attributes: Attributes, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    trace_kind(name, kind, |context| async move {
        context.set_attributes(attributes);
        work(context).await
    })
    .await
}

pub async fn produce<T, F, Fut>(system: &str, destination: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    semantic(
        format!("publish {destination}"),
        SpanKind::Producer,
        Attributes::from([
            ("messaging.system".into(), Value::String(system.into())),
            (
                "messaging.destination.name".into(),
                Value::String(destination.into()),
            ),
            (
                "messaging.operation.type".into(),
                Value::String("publish".into()),
            ),
        ]),
        work,
    )
    .await
}

pub async fn consume<T, F, Fut>(system: &str, destination: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    semantic(
        format!("process {destination}"),
        SpanKind::Consumer,
        Attributes::from([
            ("messaging.system".into(), Value::String(system.into())),
            (
                "messaging.destination.name".into(),
                Value::String(destination.into()),
            ),
            (
                "messaging.operation.type".into(),
                Value::String("process".into()),
            ),
        ]),
        work,
    )
    .await
}

pub async fn workflow<T, F, Fut>(name: &str, id: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    semantic(
        format!("workflow.{name}"),
        SpanKind::Internal,
        Attributes::from([
            ("workflow.name".into(), Value::String(name.into())),
            ("workflow.id".into(), Value::String(id.into())),
        ]),
        work,
    )
    .await
}

pub async fn step<T, F, Fut>(name: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    trace(format!("step.{name}"), work).await
}

/// HTTP span. Uses `url.full` when `route_or_url` looks absolute, else `http.route`.
/// Prefer [`http_server`] / [`http_client`] when you know the direction.
pub async fn http<T, F, Fut>(method: &str, route_or_url: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    let kind = if route_or_url.starts_with("http") {
        SpanKind::Client
    } else {
        SpanKind::Server
    };
    http_span(method, Some(route_or_url), kind, work).await
}

/// Incoming HTTP request span (`SpanKind::Server`). Records `http.server.request.duration`
/// from the `http.response.status_code` you set on the span.
pub async fn http_server<T, F, Fut>(method: &str, route: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    http_span(method, Some(route), SpanKind::Server, work).await
}

/// Outgoing HTTP request span (`SpanKind::Client`). Records `http.client.request.duration`
/// from the `http.response.status_code` you set on the span. [`fetch`] sets it for you.
pub async fn http_client<T, F, Fut>(method: &str, url: &str, work: F) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    http_span(method, Some(url), SpanKind::Client, work).await
}

/// `target` is the route (server) or full URL (client). A server span with no matched
/// route is named after the method alone, per HTTP semantic conventions.
pub(crate) async fn http_span<T, F, Fut>(
    method: &str,
    target: Option<&str>,
    kind: SpanKind,
    work: F,
) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    let server = kind == SpanKind::Server;
    let original_method = method;
    let method = normalize_method(method);
    let mut attributes =
        Attributes::from([("http.request.method".into(), Value::String(method.into()))]);
    if method != original_method {
        attributes.insert(
            "http.request.method_original".into(),
            Value::String(original_method.into()),
        );
    }
    if server {
        if let Some(route) = target {
            attributes.insert("http.route".into(), Value::String(route.into()));
        }
    } else if let Some(url) = target {
        attributes.insert("url.full".into(), Value::String(redact_url(url)));
        if let Ok(url) = reqwest::Url::parse(url) {
            if let Some(host) = url.host_str() {
                attributes.insert("server.address".into(), Value::String(host.into()));
            }
            if let Some(port) = url.port_or_known_default() {
                attributes.insert("server.port".into(), Value::from(port));
            }
        }
    }
    // Server spans: `{method} {route}` (routes are templates, so low-cardinality).
    // Client spans: `{method}` only. A full URL carries ids and query strings.
    let name = match target {
        Some(route) if server => format!("{method} {route}"),
        _ => method.to_owned(),
    };
    semantic(name, kind, attributes, |context| async move {
        let started = Instant::now();
        let output = work(context.clone()).await;
        let span = context.snapshot();
        if server {
            instruments::record_http_server(method, target, &span, started.elapsed());
        } else {
            instruments::record_http_client(
                method,
                target.unwrap_or_default(),
                &span,
                started.elapsed(),
            );
        }
        output
    })
    .await
}

/// HTTP semconv: known methods pass through, anything else (a client can send any
/// token) becomes `_OTHER`, so method can't mint unbounded span names or series.
fn normalize_method(method: &str) -> &str {
    const KNOWN: [&str; 9] = [
        "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
    ];
    KNOWN
        .iter()
        .find(|known| **known == method)
        .copied()
        .unwrap_or("_OTHER")
}

/// `url.full` without credentials or query values: `user:pass@` is dropped and
/// every query value becomes `REDACTED` (signed URLs carry secrets there).
/// Unparseable input is not recorded verbatim.
pub(crate) fn redact_url(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return "REDACTED".into();
    };
    // Both only fail for URLs that can't carry credentials (`mailto:`), so there is nothing to strip.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "fails only when there are no credentials"
    )]
    let _ = parsed.set_username("");
    #[expect(
        clippy::let_underscore_must_use,
        reason = "fails only when there are no credentials"
    )]
    let _ = parsed.set_password(None);
    parsed.set_fragment(None);
    if parsed.query().is_some() {
        let keys: Vec<String> = parsed
            .query_pairs()
            .map(|(key, _)| key.into_owned())
            .collect();
        parsed
            .query_pairs_mut()
            .clear()
            .extend_pairs(keys.iter().map(|key| (key.as_str(), "REDACTED")));
    }
    parsed.into()
}

/// Attach the active W3C `traceparent` / `baggage` headers to an outgoing reqwest call.
pub fn inject_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut builder = builder;
    for (name, value) in inject_trace_context().iter() {
        if let Ok(value) = value.to_str() {
            builder = builder.header(name.as_str(), value);
        }
    }
    builder
}

/// Send an HTTP request inside a CLIENT span, with W3C context injected.
///
/// `build` customises the request (headers, body); `fetch` sends it on a shared,
/// pooled client and records the status code, `error.type` on 4xx/5xx or transport
/// failure, and `http.client.request.duration`.
///
/// ```no_run
/// # async fn demo() -> reqwest::Result<()> {
/// let order = serde_json::json!({ "sku": "widget", "qty": 2 });
/// let response = autotel::fetch(reqwest::Method::POST, "http://orders/checkout", |request| {
///     request.json(&order)
/// })
/// .await?;
/// # let _ = response;
/// # Ok(())
/// # }
/// ```
pub async fn fetch<F>(
    method: reqwest::Method,
    url: &str,
    build: F,
) -> reqwest::Result<reqwest::Response>
where
    F: FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
{
    let method_name = method.to_string();
    // Inject inside the async body so the CLIENT span is already current
    // (`trace` attaches context when the future is polled, not when the
    // closure runs its synchronous prelude).
    http_client(&method_name, url, |context| async move {
        let request = build(inject_request(shared_client().request(method, url)));
        let result = request.send().await;
        match &result {
            Ok(response) => {
                let status = response.status();
                context.set_attribute("http.response.status_code", status.as_u16());
                if status.is_client_error() || status.is_server_error() {
                    context.record_error(&format!("HTTP {}", status.as_u16()));
                }
            }
            Err(error) => {
                let kind = if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connect"
                } else {
                    "_OTHER"
                };
                context.set_attribute("error.type", kind);
                // reqwest errors embed the full URL; record the redacted one instead.
                let mut message = error.to_string();
                if let Some(failed) = error.url() {
                    message = message.replace(failed.as_str(), &redact_url(failed.as_str()));
                }
                context.record_error(&message);
            }
        }
        result
    })
    .await
}

/// One pooled client for every `fetch`, so calls reuse connections and TLS sessions.
///
/// Bounded by default so a hung upstream can't pin a request (and its span) forever:
/// 5s to connect, 30s overall. Override per call with `request.timeout(..)` in `build`.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("autotel: building the shared reqwest client")
    })
}

/// Database CLIENT span. Records `db.client.operation.duration` labelled with system,
/// namespace, operation and collection, plus `error.type` when the span failed.
pub async fn database<T, F, Fut>(
    system: &str,
    operation: &str,
    database: &str,
    collection: &str,
    work: F,
) -> T
where
    F: FnOnce(TraceContext) -> Fut,
    Fut: Future<Output = T>,
{
    semantic(
        format!("{operation} {collection}"),
        SpanKind::Client,
        Attributes::from([
            ("db.system.name".into(), Value::String(system.into())),
            ("db.operation.name".into(), Value::String(operation.into())),
            ("db.namespace".into(), Value::String(database.into())),
            (
                "db.collection.name".into(),
                Value::String(collection.into()),
            ),
        ]),
        |context| async move {
            let started = Instant::now();
            let output = work(context.clone()).await;
            instruments::record_db_operation(
                &[
                    ("db.system.name", system),
                    ("db.namespace", database),
                    ("db.operation.name", operation),
                    ("db.collection.name", collection),
                ],
                &context.snapshot(),
                started.elapsed(),
            );
            output
        },
    )
    .await
}

/// Replace literal values in SQL with `?` so `db.query.text` can be recorded without
/// leaking data or exploding cardinality. `system` is the `db.system.name` value; it
/// decides the dialect's quoting rules:
///
/// - strings (`'…'` with `''` escapes) and numbers become `?`; comments are dropped;
/// - MySQL/MariaDB: backslash escapes, `"…"` strings, `` `ident` ``, `#` comments;
/// - PostgreSQL: `E'…'` backslash-escaped strings and `$tag$…$tag$` dollar quoting;
/// - `"…"` is kept as an identifier only for dialects where that is what it means
///   (PostgreSQL, SQLite, Oracle, SQL Server, Db2); otherwise it is treated as a string.
///
/// Fails closed: an unterminated string, identifier or dollar quote turns the rest of
/// the query into `?`, so a tokenizer mismatch can't expose what follows.
pub fn sanitize_query(sql: &str, system: &str) -> String {
    let mysql = matches!(system, "mysql" | "mariadb");
    let postgres = matches!(system, "postgresql" | "postgres");
    let quoted_identifiers = matches!(
        system,
        "postgresql"
            | "postgres"
            | "sqlite"
            | "oracle"
            | "mssql"
            | "microsoft.sql_server"
            | "db2"
            | "ibm.db2"
    );
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == ':';
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let in_word = out.chars().last().is_some_and(is_word);
        let skip_to = match c {
            '\'' => {
                // `E'…'` (Postgres) opts into backslash escapes; drop the prefix too.
                let mut tail = out.chars().rev();
                let escape_prefix = postgres
                    && matches!(tail.next(), Some('E' | 'e'))
                    && !tail.next().is_some_and(is_word);
                if escape_prefix {
                    out.pop();
                }
                let end = quoted_end(&chars, i, '\'', mysql || escape_prefix);
                out.push('?');
                end
            }
            '"' if !quoted_identifiers => {
                let end = quoted_end(&chars, i, '"', mysql);
                out.push('?');
                end
            }
            '"' | '`' if c == '"' || mysql => match quoted_end(&chars, i, c, false) {
                Some(end) => {
                    out.extend(&chars[i..=end]);
                    Some(end)
                }
                None => {
                    out.push('?');
                    None
                }
            },
            '$' if !in_word && !mysql && !chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                match dollar_quote_end(&chars, i) {
                    DollarQuote::Literal(end) => {
                        out.push('?');
                        Some(end)
                    }
                    DollarQuote::Unterminated => {
                        out.push('?');
                        None
                    }
                    DollarQuote::NotAQuote => {
                        out.push(c);
                        Some(i)
                    }
                }
            }
            '-' if chars.get(i + 1) == Some(&'-') => line_end(&chars, i),
            '#' if mysql => line_end(&chars, i),
            '/' if chars.get(i + 1) == Some(&'*') => {
                out.push(' ');
                (i + 2..chars.len().saturating_sub(1))
                    .find(|&k| chars[k] == '*' && chars[k + 1] == '/')
                    .map(|k| k + 1)
            }
            c if c.is_ascii_digit() && !in_word => {
                let mut end = i;
                while chars
                    .get(end + 1)
                    .is_some_and(|next| next.is_ascii_alphanumeric() || *next == '.')
                {
                    end += 1;
                }
                out.push('?');
                Some(end)
            }
            c => {
                out.push(c);
                Some(i)
            }
        };
        match skip_to {
            Some(last) => i = last + 1,
            // Unterminated: everything after is (possibly) literal. Stop here.
            None => break,
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Index of the closing `quote` for the literal opening at `start`, honouring doubled
/// quotes and, when `escapes`, backslash escapes. `None` if it never closes.
fn quoted_end(chars: &[char], start: usize, quote: char, escapes: bool) -> Option<usize> {
    let mut i = start + 1;
    loop {
        let c = *chars.get(i)?;
        if escapes && c == '\\' {
            i += 2;
        } else if c == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2;
            } else {
                return Some(i);
            }
        } else {
            i += 1;
        }
    }
}

/// Index of the line's last character, so a `--`/`#` comment is skipped (newline kept).
fn line_end(chars: &[char], start: usize) -> Option<usize> {
    Some(
        (start..chars.len())
            .find(|&k| chars[k] == '\n')
            .map_or(chars.len() - 1, |newline| newline - 1),
    )
}

enum DollarQuote {
    Literal(usize),
    Unterminated,
    NotAQuote,
}

/// Postgres dollar quoting: `$$…$$` or `$tag$…$tag$`, tag `[A-Za-z_][A-Za-z0-9_]*`.
fn dollar_quote_end(chars: &[char], start: usize) -> DollarQuote {
    let mut tag_end = start + 1;
    while chars
        .get(tag_end)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        tag_end += 1;
    }
    let tag_starts_ok = chars
        .get(start + 1)
        .is_some_and(|c| *c == '$' || c.is_ascii_alphabetic() || *c == '_');
    if !tag_starts_ok || chars.get(tag_end) != Some(&'$') {
        return DollarQuote::NotAQuote;
    }
    let delimiter = &chars[start..=tag_end];
    let body = tag_end + 1;
    (body..=chars.len().saturating_sub(delimiter.len()))
        .find(|&k| &chars[k..k + delimiter.len()] == delimiter)
        .map_or(DollarQuote::Unterminated, |k| {
            DollarQuote::Literal(k + delimiter.len() - 1)
        })
}

pub fn warn(message: &str) {
    if let Some(context) = current_context() {
        context.add_event(
            "log",
            Attributes::from([
                ("log.message".into(), Value::String(message.into())),
                ("log.method".into(), Value::String("warn".into())),
            ]),
        );
    }
}
