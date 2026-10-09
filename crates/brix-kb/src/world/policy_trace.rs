//! Opt-in node-touch trace of the state store (R0 M7 seam).
//!
//! A [`PolicyTrace`] is an in-memory sink a [`StateStore`](super::state_store::StateStore)
//! can record into: one [`TraceRecord`] per trie node resolved (cache hit or store read) or
//! written, tagged with the query/apply scope it happened in. It is the observation side of
//! the operational-policy seam (a learned or heuristic `StatePolicy` is trained and
//! replayed against these traces); it is **not** an authority, is never persisted in the
//! node store, never reaches a revision or a root, and recording never changes a result.
//!
//! Default off. When off, the store pays one relaxed atomic load per node resolution and
//! nothing else (no allocation, no lock). Schema: [`POLICY_TRACE_SCHEMA`].

#![deny(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use brix_canon::Digest;

/// Schema tag of the JSONL form of a trace.
pub const POLICY_TRACE_SCHEMA: &str = "brix.world.policy-trace@1";

/// What kind of node touch a record describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceOp {
    /// A trie node was resolved by a store-aware operation: `hit` says whether the decoded
    /// node cache served it; on a miss `bytes` is the encoded size read from the store.
    NodeRead,
    /// A new node was written (`bytes` encoded bytes); `hit` is always false.
    NodeWrite,
}

impl TraceOp {
    /// The `op` field of the JSONL form.
    pub fn as_str(self) -> &'static str {
        match self {
            TraceOp::NodeRead => "node_read",
            TraceOp::NodeWrite => "node_write",
        }
    }
}

/// One recorded node touch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRecord {
    /// Position in the trace, from 0, dense across the whole life of the sink (records
    /// dropped at capacity still consume a sequence number).
    pub seq: u64,
    /// The innermost active scope label (`"apply"`, `"query"`, ...), or `""`.
    pub scope: &'static str,
    pub op: TraceOp,
    pub digest: Digest,
    pub hit: bool,
    pub bytes: u64,
}

impl TraceRecord {
    /// One line of the `brix.world.policy-trace@1` JSONL form.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": POLICY_TRACE_SCHEMA,
            "seq": self.seq,
            "scope": self.scope,
            "op": self.op.as_str(),
            "digest": self.digest.to_hex(),
            "hit": self.hit,
            "bytes": self.bytes,
        })
    }
}

#[derive(Default)]
struct Sink {
    cap: usize,
    records: Vec<TraceRecord>,
    dropped: u64,
}

/// Shared handle to an in-memory trace sink.
#[derive(Clone)]
pub struct PolicyTrace {
    sink: Arc<Mutex<Sink>>,
}

impl std::fmt::Debug for PolicyTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PolicyTrace")
    }
}

impl PolicyTrace {
    /// A sink keeping at most `cap` records; further touches are counted in
    /// [`Self::dropped`] but not stored.
    pub fn new(cap: usize) -> Self {
        Self {
            sink: Arc::new(Mutex::new(Sink {
                cap,
                ..Sink::default()
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Sink> {
        self.sink.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A copy of the records so far.
    pub fn records(&self) -> Vec<TraceRecord> {
        self.lock().records.clone()
    }

    /// Remove and return the records so far (the sequence continues).
    pub fn take(&self) -> Vec<TraceRecord> {
        std::mem::take(&mut self.lock().records)
    }

    /// Touches not stored because the sink was full.
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    pub fn len(&self) -> usize {
        self.lock().records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().records.is_empty()
    }

    /// The records as `brix.world.policy-trace@1` JSON lines.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for r in self.lock().records.iter() {
            out.push_str(&r.to_json().to_string());
            out.push('\n');
        }
        out
    }
}

/// The recorder a store owns: a switch, a sequence counter, the scope stack and the sink.
#[derive(Default)]
pub(crate) struct Recorder {
    on: AtomicBool,
    seq: AtomicU64,
    scopes: Mutex<Vec<&'static str>>,
    sink: Mutex<Option<PolicyTrace>>,
}

impl Recorder {
    pub(crate) fn enable(&self, trace: PolicyTrace) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(trace);
        self.on.store(true, Ordering::Release);
    }

    pub(crate) fn disable(&self) {
        self.on.store(false, Ordering::Release);
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[inline(always)]
    pub(crate) fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Record one touch. Only call after [`Self::is_on`] returned true.
    #[cold]
    pub(crate) fn record(&self, op: TraceOp, digest: &Digest, hit: bool, bytes: usize) {
        let Some(trace) = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
            return;
        };
        let scope = self
            .scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .copied()
            .unwrap_or("");
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut sink = trace.lock();
        if sink.records.len() >= sink.cap {
            sink.dropped += 1;
            return;
        }
        sink.records.push(TraceRecord {
            seq,
            scope,
            op,
            digest: *digest,
            hit,
            bytes: bytes as u64,
        });
    }

    pub(crate) fn push_scope(&self, label: &'static str) {
        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(label);
    }

    pub(crate) fn pop_scope(&self) {
        self.scopes.lock().unwrap_or_else(|e| e.into_inner()).pop();
    }
}

/// Keeps a scope label active until dropped. Inert (and free) when tracing is off.
#[must_use = "the scope ends when the guard is dropped"]
pub struct TraceScope {
    recorder: Option<Arc<Recorder>>,
}

impl TraceScope {
    pub(crate) fn new(recorder: &Arc<Recorder>, label: &'static str) -> Self {
        if recorder.is_on() {
            recorder.push_scope(label);
            Self {
                recorder: Some(recorder.clone()),
            }
        } else {
            Self { recorder: None }
        }
    }
}

impl Drop for TraceScope {
    fn drop(&mut self) {
        if let Some(r) = &self.recorder {
            r.pop_scope();
        }
    }
}
