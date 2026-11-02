//! Process-local tail sampling: keeps or drops the spans this process recorded
//! for a trace, decided once that part of the trace has finished here.
//!
//! Every span of a trace is held until the trace's local root (no parent, or a
//! remote one) has ended and every span started under it has ended too. The
//! trace is kept when any span carries `autotel.sampling.tail.keep = true`: a
//! `trace()` verdict, `force_keep()`, or an `autotel.debug` request. Otherwise
//! the sampler decides from the whole trace: any span with ERROR status, the
//! root's duration, the trace id for the baseline. Children follow their
//! trace, so a dropped trace leaves no orphan spans.
//!
//! Each service decides on its own spans. Two services can reach different
//! verdicts for one distributed trace and leave it incomplete. For one decision
//! across services, route every span of a trace to one OpenTelemetry Collector
//! running the `tail_sampling` processor.
//!
//! Fail-open: a trace whose root has not ended within 30s, or the oldest trace
//! once 20,000 spans are held, is exported as it stands. Shutdown decides
//! everything.

use crate::{AUTOTEL_SAMPLING_TAIL_EVALUATED, AUTOTEL_SAMPLING_TAIL_KEEP, TailSampler};
use opentelemetry::{
    Context, Value,
    trace::{Span as _, SpanId, Status, TraceContextExt as _, TraceId},
};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    trace::{Span, SpanData, SpanProcessor},
};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_BUFFERED_SPANS: usize = 20_000;
const TRACE_TIMEOUT: Duration = Duration::from_secs(30);
/// Verdicts remembered for spans that end after their trace was decided.
const MAX_DECIDED_TRACES: usize = 10_000;

struct Pending {
    spans: Vec<SpanData>,
    /// Spans started and not yet ended.
    open: usize,
    root: Option<usize>,
    deadline: Instant,
}

#[derive(Default)]
struct State {
    pending: HashMap<TraceId, Pending>,
    /// Pending traces in creation order, so deadlines ascend. Decided ids linger until swept.
    order: VecDeque<TraceId>,
    decided: HashMap<TraceId, bool>,
    decided_order: VecDeque<TraceId>,
    buffered: usize,
}

pub struct TailSamplingProcessor {
    inner: Box<dyn SpanProcessor>,
    sampler: Arc<dyn TailSampler>,
    state: Mutex<State>,
}

impl fmt::Debug for TailSamplingProcessor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TailSamplingProcessor")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl TailSamplingProcessor {
    /// Hold spans for `inner` (the exporting processor) until their trace is decided.
    pub fn new(inner: impl SpanProcessor + 'static, sampler: Arc<dyn TailSampler>) -> Self {
        Self {
            inner: Box::new(inner),
            sampler,
            state: Mutex::default(),
        }
    }

    fn release(&self, spans: Vec<SpanData>) {
        for span in spans {
            self.inner.on_end(span);
        }
    }

    /// Decide a trace and return its spans if kept. `fail_open` keeps it unjudged.
    fn decide(&self, state: &mut State, trace_id: TraceId, fail_open: bool) -> Vec<SpanData> {
        let Some(trace) = state.pending.remove(&trace_id) else {
            return Vec::new();
        };
        state.buffered -= trace.spans.len();
        let keep = fail_open
            || trace
                .root
                .is_some_and(|root| self.keep_trace(&trace.spans[root], &trace.spans));
        state.decided.insert(trace_id, keep);
        state.decided_order.push_back(trace_id);
        if state.decided_order.len() > MAX_DECIDED_TRACES
            && let Some(oldest) = state.decided_order.pop_front()
        {
            state.decided.remove(&oldest);
        }
        if keep { trace.spans } else { Vec::new() }
    }

    fn keep_trace(&self, root: &SpanData, spans: &[SpanData]) -> bool {
        let verdicts: Vec<bool> = spans
            .iter()
            .filter(|span| attribute(span, AUTOTEL_SAMPLING_TAIL_EVALUATED) == Some(true))
            .map(|span| attribute(span, AUTOTEL_SAMPLING_TAIL_KEEP) == Some(true))
            .collect();
        if verdicts.contains(&true) {
            return true;
        }
        if spans
            .iter()
            .any(|span| self.sampler.keep_span(&span.attributes))
        {
            return true;
        }
        let failed = spans
            .iter()
            .any(|span| matches!(span.status, Status::Error { .. }));
        let root_dropped = attribute(root, AUTOTEL_SAMPLING_TAIL_EVALUATED) == Some(true)
            && attribute(root, AUTOTEL_SAMPLING_TAIL_KEEP) == Some(false);
        // The root's wrapper already weighed a clean trace; only a failure it
        // could not see (a child error it swallowed) reopens the question.
        if root_dropped && !failed {
            return false;
        }
        self.sampler.should_keep_trace(
            &root.span_context.trace_id().to_string(),
            !failed,
            root.end_time
                .duration_since(root.start_time)
                .unwrap_or_default(),
        )
    }

    /// Decide every trace whose deadline has passed: on what it has if its root
    /// ended, otherwise export it as it stands.
    fn sweep(&self, state: &mut State) -> Vec<SpanData> {
        let now = Instant::now();
        let mut released = Vec::new();
        while let Some(&trace_id) = state.order.front() {
            match state.pending.get(&trace_id) {
                Some(trace) if trace.deadline > now => break,
                Some(trace) => {
                    let fail_open = trace.root.is_none();
                    released.extend(self.decide(state, trace_id, fail_open));
                }
                None => {}
            }
            state.order.pop_front();
        }
        released
    }
}

fn attribute(span: &SpanData, key: &str) -> Option<bool> {
    span.attributes
        .iter()
        .find(|pair| pair.key.as_str() == key)
        .and_then(|pair| match pair.value {
            Value::Bool(value) => Some(value),
            _ => None,
        })
}

fn pending(state: &mut State, trace_id: TraceId) -> &mut Pending {
    state.pending.entry(trace_id).or_insert_with(|| {
        state.order.push_back(trace_id);
        Pending {
            spans: Vec::new(),
            open: 0,
            root: None,
            deadline: Instant::now() + TRACE_TIMEOUT,
        }
    })
}

impl SpanProcessor for TailSamplingProcessor {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        self.inner.on_start(span, cx);
        let trace_id = span.span_context().trace_id();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A new local root (a later request in the same distributed trace) starts a
        // fresh decision instead of inheriting the earlier verdict.
        let parent = cx.span().span_context().clone();
        if !parent.is_valid() || parent.is_remote() {
            state.decided.remove(&trace_id);
        }
        if !state.decided.contains_key(&trace_id) {
            pending(&mut state, trace_id).open += 1;
        }
    }

    fn on_end(&self, span: SpanData) {
        let trace_id = span.span_context.trace_id();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&keep) = state.decided.get(&trace_id) {
            drop(state);
            if keep {
                self.inner.on_end(span);
            }
            return;
        }
        let local_root = span.parent_span_id == SpanId::INVALID || span.parent_span_is_remote;
        let trace = pending(&mut state, trace_id);
        trace.spans.push(span);
        // A span started before this processor existed was never counted.
        trace.open = trace.open.saturating_sub(1);
        if local_root {
            trace.root = Some(trace.spans.len() - 1);
        }
        let complete = trace.root.is_some() && trace.open == 0;
        state.buffered += 1;
        let mut released = if complete {
            self.decide(&mut state, trace_id, false)
        } else {
            Vec::new()
        };
        released.extend(self.sweep(&mut state));
        while state.buffered > MAX_BUFFERED_SPANS {
            let Some(oldest) = state.order.pop_front() else {
                break;
            };
            released.extend(self.decide(&mut state, oldest, true));
        }
        drop(state);
        self.release(released);
    }

    /// Flushes decided spans only. A trace still waiting on running children
    /// stays pending: deciding it now would miss a late error or `force_keep()`.
    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        // Nothing more will end. Decide what has a root on what it has, and
        // export the rest (no root ever ended) rather than lose it.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut released = Vec::new();
        while let Some(trace_id) = state.order.pop_front() {
            let fail_open = state
                .pending
                .get(&trace_id)
                .is_some_and(|trace| trace.root.is_none());
            released.extend(self.decide(&mut state, trace_id, fail_open));
        }
        state.decided.clear();
        state.decided_order.clear();
        drop(state);
        self.release(released);
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}
