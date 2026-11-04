use crate::{
    Attributes, GenAiOptions, GenAiUsage, SpanData, completed_spans, gen_ai, hash_hex,
    record_gen_ai_usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

pub fn deterministic_trace_id(seed: &str) -> String {
    hash_hex(seed.as_bytes(), 32)
}

pub fn flatten_metadata(metadata: &Attributes) -> Attributes {
    fn flatten(prefix: &str, value: &Value, output: &mut Attributes) {
        if let Value::Object(values) = value {
            for (key, nested) in values {
                flatten(&format!("{prefix}.{key}"), nested, output);
            }
        } else {
            output.insert(prefix.into(), value.clone());
        }
    }
    let mut output = BTreeMap::new();
    for (key, value) in metadata {
        flatten(&format!("metadata.{key}"), value, &mut output);
    }
    output
}

/// The thread whose collector is open. `init` clears the span buffer and dropping a
/// collector shuts the SDK down, so two open at once would break each other.
static COLLECTOR_TURN: Mutex<Option<ThreadId>> = Mutex::new(None);
static COLLECTOR_TURN_ENDED: Condvar = Condvar::new();

fn collector_turn() -> MutexGuard<'static, Option<ThreadId>> {
    COLLECTOR_TURN.lock().unwrap_or_else(|e| e.into_inner())
}

/// A held turn. It stores no guard, so [`TraceCollector`] stays `Send`.
struct CollectorTurn;

impl CollectorTurn {
    fn take() -> anyhow::Result<Self> {
        let current = thread::current().id();
        let mut holder = collector_turn();
        while let Some(thread) = *holder {
            anyhow::ensure!(
                thread != current,
                "a TraceCollector is already open on this thread"
            );
            holder = COLLECTOR_TURN_ENDED
                .wait(holder)
                .unwrap_or_else(|e| e.into_inner());
        }
        *holder = Some(current);
        Ok(Self)
    }
}

impl Drop for CollectorTurn {
    fn drop(&mut self) {
        *collector_turn() = None;
        COLLECTOR_TURN_ENDED.notify_one();
    }
}

/// Records spans for assertions in tests.
///
/// One collector is open at a time per process. [`TraceCollector::new`] waits for the
/// open one to drop, so tests that use a collector run one after another, and each
/// sees only its own spans. Opening a second on the same thread returns an error.
pub struct TraceCollector {
    _runtime: crate::AutotelRuntime,
    // Declared last: the turn ends after the runtime has shut down.
    _turn: CollectorTurn,
}
impl TraceCollector {
    pub fn new(service: &str) -> anyhow::Result<Self> {
        let turn = CollectorTurn::take()?;
        Ok(Self {
            _runtime: crate::init(crate::AutotelConfig::new(service).capture_spans())?,
            _turn: turn,
        })
    }
    pub fn spans(&self) -> Vec<SpanData> {
        completed_spans()
    }
    pub fn expect_span(&self, name: &str) -> SpanData {
        let matches: Vec<_> = self
            .spans()
            .into_iter()
            .filter(|span| span.name == name)
            .collect();
        assert_eq!(matches.len(), 1, "expected one {name}");
        matches[0].clone()
    }
    pub fn by_name(&self, name: &str) -> Vec<SpanData> {
        self.spans()
            .into_iter()
            .filter(|span| span.name == name)
            .collect()
    }
    pub fn by_attributes(&self, expected: &Attributes) -> Vec<SpanData> {
        self.spans()
            .into_iter()
            .filter(|span| {
                expected
                    .iter()
                    .all(|(key, value)| span.attributes.get(key) == Some(value))
            })
            .collect()
    }
    pub fn descendants(&self, parent_span_id: &str) -> Vec<SpanData> {
        self.spans()
            .into_iter()
            .filter(|span| span.parent_span_id.as_deref() == Some(parent_span_id))
            .collect()
    }
}

pub struct Devtools {
    port: u16,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Devtools {
    pub fn start(port: u16) -> std::io::Result<Self> {
        crate::core::enable_span_capture();
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let thread = thread::spawn(move || {
            while flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            port,
            running,
            thread: Some(thread),
        })
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn spans(&self) -> Vec<SpanData> {
        completed_spans()
    }
}
impl Drop for Devtools {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a panicked server thread has nothing left to clean up"
            )]
            let _ = thread.join();
        }
    }
}
fn serve(mut stream: TcpStream) {
    let mut request = [0_u8; 1024];
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the request is ignored; every path serves the trace list"
    )]
    let _ = stream.read(&mut request);
    let items = completed_spans()
        .iter()
        .map(|span| format!("<li>{}</li>", escape_html(&span.name)))
        .collect::<String>();
    let body =
        format!("<!doctype html><title>Autotel Devtools</title><h1>Traces</h1><ul>{items}</ul>");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a client that hung up needs no response"
    )]
    let _ = stream.write_all(response.as_bytes());
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Clone, Debug)]
pub struct GenAiResult {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub first_chunk: Duration,
}
#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
    prompt_eval_count: u64,
    eval_count: u64,
}
#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}

fn ollama_base_url() -> String {
    std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into())
}
pub async fn gen_ai_available() -> bool {
    reqwest::Client::new()
        .get(format!("{}/api/tags", ollama_base_url()))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}
pub async fn generate(model: &str, prompt: &str) -> anyhow::Result<GenAiResult> {
    let model = model.to_owned();
    let prompt = prompt.to_owned();
    let options = GenAiOptions::generate_content("ollama", model.clone());
    gen_ai(options, |context| async move {
        let started = Instant::now();
        let response: OllamaResponse = reqwest::Client::new()
            .post(format!("{}/api/generate", ollama_base_url()))
            .json(&OllamaRequest {
                model: &model,
                prompt: &prompt,
                stream: false,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let result = GenAiResult {
            text: response.response,
            input_tokens: response.prompt_eval_count,
            output_tokens: response.eval_count,
            first_chunk: started.elapsed(),
        };
        record_gen_ai_usage(
            &context,
            &GenAiUsage {
                input_tokens: result.input_tokens,
                output_tokens: result.output_tokens,
                response_model: Some(model.clone()),
                finish_reasons: Vec::new(),
            },
        );
        context.set_attribute(
            "gen_ai.response.time_to_first_chunk",
            result.first_chunk.as_secs_f64(),
        );
        Ok(result)
    })
    .await
}
