//! The wasmtime Driver host (Ring0 §1.7, §1.11 — issue #27).
//!
//! Hosts `sdk/driver-wit/delta-abi.wit`'s `driver` world: wasmtime
//! component-model instantiation, the imported `capabilities` interface
//! (`net`, `console`), and the delta round-trip (engine [`crate::delta`]
//! types <-> WIT types <-> engine types) that a guest Driver's `apply`
//! answers.
//!
//! # A documented gap-fill in the WIT
//!
//! `delta-abi.wit`'s `capabilities` interface originally declared the `net`
//! and `console` *resource* types (with their methods) but no way for a
//! guest to ever obtain an instance of either — the component model requires
//! a handle to arrive as a function parameter or result, and the world had
//! neither. The WIT file's own comments call this out as future work ("the
//! rest are named ... to be filled in as the wasmtime host lands"). Landing
//! this host requires closing that gap, so `get-net`/`get-console` free
//! functions were added to the `capabilities` interface: the host issues (or
//! withholds, as `none`) a capability once per instantiation, matching Part
//! VII §4 ("unforgeable, host-issued or attenuated"). Nothing about *what a
//! capability means* was invented — the resource shapes, methods, and error
//! vocabulary are exactly what the WIT already specified; this is the
//! mechanical wiring the WIT's own comments predicted would be needed.
//!
//! # Lease/cancel plumbing
//!
//! Ring0 §1.7 lists "wasmtime Driver host with capability imports" and
//! §1.11 lists "lease/cancel plumbing" as this lane's scope. The protocol
//! lifecycle state machine itself (Appendix H: `Desired -> Leased ->
//! Attempted -> ...`) is explicitly **not** part of this crate's current
//! slice (see the crate-level docs) — nothing drives that state machine yet.
//! What *is* this module's job, and what it implements: when the engine
//! cancels an in-flight capability call bound to a lease (lease expiry or
//! shutdown), the host must surface that to the guest as `net-error.cancelled`
//! rather than a transport failure, so the Driver produces an honest
//! `Outcome::Cancelled` (Appendix H's `Acknowledged`) instead of a spurious
//! `Failed`. [`NetCapability::request`] returning
//! [`bindings::brixms::delta::capabilities::NetError::Cancelled`] is that
//! plumbing; [`RecordingNet`] (the reference in-memory implementation this
//! module ships) can be scripted to return it, which the conformance test
//! exercises.

use std::sync::{Arc, Mutex};

use wasmtime::component::{Component, Linker, Resource, ResourceTable};
use wasmtime::{Engine, Result, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

/// The wasmtime component-model bindings generated from
/// `sdk/driver-wit/delta-abi.wit`'s `driver` world (path relative to this
/// crate's `Cargo.toml`, i.e. `../../sdk/driver-wit`). Namespaced in its own
/// module so the generated `brixms`/`Driver`/etc. names never collide with
/// this module's own host-side types.
mod bindings {
    wasmtime::component::bindgen!({
        world: "driver",
        path: "../../sdk/driver-wit",
        async: false,
        // Back the `net`/`console` WIT resources directly with this
        // module's own host types instead of bindgen's opaque placeholder
        // enums, so `ResourceTable` stores (and returns) real data.
        with: {
            "brixms:delta/capabilities/net": super::NetResource,
            "brixms:delta/capabilities/console": super::ConsoleResource,
        },
    });
}

use bindings::brixms::delta::capabilities::{
    Console as WitConsoleGuestBinding, HostConsole, HostNet, HttpRequest as WitHttpRequest,
    HttpResponse as WitHttpResponse, Net as WitNetGuestBinding, NetError as WitNetError,
};
use bindings::brixms::delta::delta as wit_delta;
use bindings::brixms::delta::types as wit_types;
use bindings::Driver;

use crate::delta::CanonRow;
use crate::delta::{
    DeltaBatch as RtDeltaBatch, DeltaOp as RtDeltaOp, DeltaOutput as RtDeltaOutput,
    DeltaSource as RtDeltaSource, DeltaSourceKind as RtDeltaSourceKind, Emission as RtEmission,
    RuleErrorEmission as RtRuleErrorEmission, SupportOp as RtSupportOp,
    SupportRecord as RtSupportRecord,
};
use crate::ids::{MatchDigest, RelationRef, RuleRef, SiteId};
use brix_canon::{Digest, EdgeId};

// ---------------------------------------------------------------------------
// Host-side capability traits. A production host implements these against a
// real outbound-HTTP client / logger; [`RecordingNet`]/[`RecordingConsole`]
// below are the reference in-memory implementations used by tests and dev
// profiles (Part VII §5: "ambient dev capabilities").
// ---------------------------------------------------------------------------

/// One outbound HTTP request as the host observes it, mirroring the WIT
/// `capabilities.http-request` record one-for-one.
pub type HttpRequest = WitHttpRequest;
/// One HTTP response as the host returns it, mirroring
/// `capabilities.http-response` one-for-one.
pub type HttpResponse = WitHttpResponse;
/// Why a `net` call failed, mirroring `capabilities.net-error` one-for-one.
pub type NetError = WitNetError;

/// The host-side implementation of one `net` capability grant. Real hosts
/// implement this against an actual HTTP client, validating each request
/// against the issued capability's scope (`Net<HostPattern>`, Part VII §4)
/// before it ever reaches the network.
pub trait NetCapability: Send + Sync {
    /// Perform (or refuse, or report cancelled) one outbound request.
    fn request(&self, req: HttpRequest) -> std::result::Result<HttpResponse, NetError>;
}

/// The host-side implementation of one `console` capability grant.
pub trait ConsoleCapability: Send + Sync {
    /// Emit one log line.
    fn log(&self, message: String);
}

/// The reference in-memory [`NetCapability`]: scripted responses in, every
/// request recorded out. This is a *real* capability implementation in the
/// sense the WIT/Part VII §4 care about — it faithfully executes the
/// `net.request` contract (including reporting `cancelled` when scripted
/// to) — it is just backed by a script instead of a socket, which is exactly
/// what `brix sim`'s "every boundary is bound to an adapter" profile (Part
/// VII §5) calls for, and what the conformance test needs to observe the
/// guest's outbound call without hitting the network.
#[derive(Default)]
pub struct RecordingNet {
    seen: Mutex<Vec<HttpRequest>>,
    script: Mutex<Vec<std::result::Result<HttpResponse, NetError>>>,
}

impl RecordingNet {
    /// A `RecordingNet` that answers every request with `response`, in order
    /// until the script runs out (the last entry then repeats).
    pub fn scripted(responses: Vec<std::result::Result<HttpResponse, NetError>>) -> Self {
        RecordingNet {
            seen: Mutex::new(Vec::new()),
            script: Mutex::new(responses),
        }
    }

    /// The requests this capability observed, in arrival order.
    pub fn seen(&self) -> Vec<HttpRequest> {
        self.seen
            .lock()
            .expect("RecordingNet::seen mutex poisoned")
            .clone()
    }
}

impl NetCapability for RecordingNet {
    fn request(&self, req: HttpRequest) -> std::result::Result<HttpResponse, NetError> {
        self.seen
            .lock()
            .expect("RecordingNet::request seen mutex poisoned")
            .push(req);
        let mut script = self
            .script
            .lock()
            .expect("RecordingNet::request script mutex poisoned");
        if script.is_empty() {
            return Err(NetError::Transport(
                "RecordingNet: script exhausted".to_string(),
            ));
        }
        if script.len() == 1 {
            script[0].clone()
        } else {
            script.remove(0)
        }
    }
}

/// The reference in-memory [`ConsoleCapability`]: records log lines instead
/// of writing them anywhere, so a test can assert on them.
#[derive(Default)]
pub struct RecordingConsole {
    lines: Mutex<Vec<String>>,
}

impl RecordingConsole {
    /// The log lines observed so far, in arrival order.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("RecordingConsole::lines mutex poisoned")
            .clone()
    }
}

impl ConsoleCapability for RecordingConsole {
    fn log(&self, message: String) {
        self.lines
            .lock()
            .expect("RecordingConsole::log mutex poisoned")
            .push(message);
    }
}

// ---------------------------------------------------------------------------
// Host state.
// ---------------------------------------------------------------------------

/// What a Driver instance was granted, per Part VII §4 (capabilities are
/// host-issued and scoped; `None` is a normal, expected "not granted", not
/// an error).
#[derive(Default, Clone)]
pub struct CapabilityGrants {
    pub net: Option<Arc<dyn NetCapability>>,
    pub console: Option<Arc<dyn ConsoleCapability>>,
}

/// Per-instantiation host state: WASI plumbing every `wasm32-wasip2`
/// component needs regardless of the delta ABI (see DEPS.md's
/// `wasmtime-wasi` entry), the resource table backing `net`/`console`
/// handles, and this Driver's capability grants.
pub struct DriverHostState {
    wasi_ctx: WasiCtx,
    table: ResourceTable,
    grants: CapabilityGrants,
}

impl DriverHostState {
    fn new(grants: CapabilityGrants) -> Self {
        DriverHostState {
            wasi_ctx: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            grants,
        }
    }
}

impl WasiView for DriverHostState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi_ctx
    }
}

/// The host-side resource backing one guest `net` handle: just the capability
/// grant it was issued, wrapped so the resource table can own it.
// `pub` (not `pub(crate)`/private): `mod bindings`'s `with:` map re-exports
// these as `Net`/`Console` on its own public surface, which requires them to
// be at least as visible as that re-export.
pub struct NetResource(Arc<dyn NetCapability>);
/// The host-side resource backing one guest `console` handle.
pub struct ConsoleResource(Arc<dyn ConsoleCapability>);

impl HostNet for DriverHostState {
    // No `wasmtime::Result` wrapper (unlike `drop` below, which always gets
    // one): wit-bindgen only adds the trap-wrapper for opted-in trappable
    // imports (not configured here), so a plain import's trait signature is
    // exactly its WIT return type, `result<http-response, net-error>` ->
    // `Result<HttpResponse, NetError>`.
    fn request(
        &mut self,
        self_: Resource<WitNetGuestBinding>,
        req: WitHttpRequest,
    ) -> std::result::Result<WitHttpResponse, WitNetError> {
        // A missing table entry here would be a host bug (the guest can only
        // hold handles this host issued), so this trusts the table.
        let net = self
            .table
            .get(&self_)
            .expect("guest holds a net handle this host did not issue");
        net.0.request(req)
    }

    fn drop(&mut self, rep: Resource<WitNetGuestBinding>) -> Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl HostConsole for DriverHostState {
    fn log(&mut self, self_: Resource<WitConsoleGuestBinding>, message: String) {
        let console = self
            .table
            .get(&self_)
            .expect("guest holds a console handle this host did not issue");
        console.0.log(message);
    }

    fn drop(&mut self, rep: Resource<WitConsoleGuestBinding>) -> Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl bindings::brixms::delta::capabilities::Host for DriverHostState {
    // `option<net>`/`option<console>` return types (no explicit `result<>`)
    // are not in wit-bindgen's "trappable" shape, so — unlike `request`/`log`
    // above — these two are plain, unwrapped values: pushing a freshly
    // granted capability into this instantiation's resource table cannot
    // fail in practice (it fails only on `u32` handle-counter exhaustion).
    fn get_net(&mut self) -> Option<Resource<WitNetGuestBinding>> {
        self.grants.net.clone().map(|net| {
            self.table
                .push(NetResource(net))
                .expect("resource table push cannot fail for a freshly issued net grant")
        })
    }

    fn get_console(&mut self) -> Option<Resource<WitConsoleGuestBinding>> {
        self.grants.console.clone().map(|console| {
            self.table
                .push(ConsoleResource(console))
                .expect("resource table push cannot fail for a freshly issued console grant")
        })
    }
}

// The `types`/`delta` WIT interfaces the `driver` world `use`s carry only
// type definitions, no free functions — their generated `Host` traits are
// empty marker traits, but `Driver::add_to_linker`'s bound still names them.
impl bindings::brixms::delta::types::Host for DriverHostState {}
impl bindings::brixms::delta::delta::Host for DriverHostState {}

// ---------------------------------------------------------------------------
// Delta round-trip: engine <-> WIT.
//
// Every conversion here is the mechanical mirror the WIT file's header
// promises ("every shape here mirrors a Rust type one-for-one"). Nothing
// below is a semantic decision — it is byte/field-for-field with the shapes
// already fixed by `crate::delta` and `delta-abi.wit`.
// ---------------------------------------------------------------------------

/// Host only ever receives digests over this boundary (never sends one back
/// through `apply`'s input — `delta-batch` carries opaque `canon-row` bytes,
/// not edge/match identities), so only the WIT -> engine direction is
/// needed. `EdgeId`/`MatchDigest` wrap an already-domain-tagged
/// [`brix_canon::Digest`]; the boundary carries the raw 32 bytes and the
/// host re-tags them into the right typed wrapper (see the WIT file's
/// `types` interface header).
fn digest32(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

fn edge_id_from_wit(bytes: &wit_types::EdgeId) -> EdgeId {
    EdgeId(Digest::from_bytes(digest32(bytes)))
}

fn match_digest_from_wit(bytes: &wit_types::MatchDigest) -> MatchDigest {
    MatchDigest(Digest::from_bytes(digest32(bytes)))
}

fn support_record_from_wit(s: &wit_delta::SupportRecord) -> RtSupportRecord {
    RtSupportRecord {
        edge: edge_id_from_wit(&s.edge),
        rule: RuleRef::from(s.rule.as_str()),
        match_digest: match_digest_from_wit(&s.match_digest),
    }
}

fn support_op_from_wit(op: &wit_delta::SupportOp) -> RtSupportOp {
    match op {
        wit_delta::SupportOp::Add(s) => RtSupportOp::Add(support_record_from_wit(s)),
        wit_delta::SupportOp::Remove(s) => RtSupportOp::Remove(support_record_from_wit(s)),
    }
}

fn delta_source_from_wit(source: &wit_delta::DeltaSource) -> RtDeltaSource {
    let kind = match &source.kind {
        wit_delta::DeltaSourceKind::Rule(r) => RtDeltaSourceKind::Rule {
            rule: RuleRef::from(r.rule.as_str()),
            site: r.site.map(SiteId),
        },
        wit_delta::DeltaSourceKind::Protocol(p) => RtDeltaSourceKind::Protocol {
            protocol: RelationRef::from(p.as_str()),
        },
    };
    RtDeltaSource {
        relation: RelationRef::from(source.relation.as_str()),
        kind,
    }
}

fn delta_batch_to_wit(batch: RtDeltaBatch<CanonRow>) -> wit_delta::DeltaBatch {
    wit_delta::DeltaBatch {
        at: batch.at.0,
        ops: batch
            .ops
            .into_iter()
            .map(|op| match op {
                RtDeltaOp::Insert(row) => wit_delta::DeltaOp::Insert(row.0),
                RtDeltaOp::Retract(row) => wit_delta::DeltaOp::Retract(row.0),
            })
            .collect(),
    }
}

fn delta_output_from_wit(out: wit_delta::DeltaOutput) -> RtDeltaOutput<CanonRow> {
    RtDeltaOutput {
        emissions: out
            .emissions
            .iter()
            .map(|e| RtEmission {
                edge: edge_id_from_wit(&e.edge),
                row: CanonRow(e.row.clone()),
                supports: e.supports.iter().map(support_op_from_wit).collect(),
            })
            .collect(),
        support_ops: out.support_ops.iter().map(support_op_from_wit).collect(),
        errors: out
            .errors
            .iter()
            .map(|e| RtRuleErrorEmission {
                site: SiteId(e.site),
                partial_match: match_digest_from_wit(&e.partial_match),
                error: CanonRow(e.error.clone()),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The host: load a component, instantiate it against a set of capability
// grants, and drive its `source`/`apply` exports.
// ---------------------------------------------------------------------------

/// One instantiated guest Driver, ready to answer `source`/`apply` calls.
pub struct DriverInstance {
    store: Store<DriverHostState>,
    bindings: Driver,
}

impl DriverInstance {
    /// The delta source this Driver answers to (read once, per the WIT
    /// world's contract that the host reads it once at load).
    pub fn source(&mut self) -> Result<RtDeltaSource> {
        // `source`/`apply` are exported directly by the `driver` world (not
        // via a named interface), so they are top-level methods on the
        // generated world struct — no interface accessor to go through.
        let wit_source = self.bindings.call_source(&mut self.store)?;
        Ok(delta_source_from_wit(&wit_source))
    }

    /// Drive one batch through the guest's `apply` export, round-tripping
    /// engine delta types through the WIT ABI and back.
    pub fn apply(&mut self, batch: RtDeltaBatch<CanonRow>) -> Result<RtDeltaOutput<CanonRow>> {
        let wit_batch = delta_batch_to_wit(batch);
        let wit_out = self.bindings.call_apply(&mut self.store, &wit_batch)?;
        Ok(delta_output_from_wit(wit_out))
    }
}

/// The wasmtime Driver host: loads `driver`-world components and instantiates
/// them against a set of capability grants.
pub struct DriverHost {
    engine: Engine,
    linker: Linker<DriverHostState>,
}

impl DriverHost {
    /// Build a host with a fresh wasmtime [`Engine`] and a [`Linker`] wired
    /// for the `driver` world: the standard WASI 0.2 worlds every
    /// `wasm32-wasip2` component needs (DEPS.md), plus this crate's
    /// `capabilities` implementation.
    pub fn new() -> Result<Self> {
        let engine = Engine::default();
        let mut linker: Linker<DriverHostState> = Linker::new(&engine);
        wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        Driver::add_to_linker::<DriverHostState, DriverHostState>(&mut linker, |state| state)?;
        Ok(DriverHost { engine, linker })
    }

    /// Load a `driver`-world component from its compiled bytes (a
    /// `wasm32-wasip2` component built from `sdk/brix-driver-rs`; see
    /// `crates/brix-rt/tests/driver_host.rs` for how the conformance test
    /// obtains one).
    pub fn load_component(&self, bytes: &[u8]) -> Result<Component> {
        Component::from_binary(&self.engine, bytes)
    }

    /// Instantiate a loaded component against one set of capability grants.
    pub fn instantiate(
        &self,
        component: &Component,
        grants: CapabilityGrants,
    ) -> Result<DriverInstance> {
        let mut store = Store::new(&self.engine, DriverHostState::new(grants));
        let bindings = Driver::instantiate(&mut store, component, &self.linker)?;
        Ok(DriverInstance { store, bindings })
    }
}
