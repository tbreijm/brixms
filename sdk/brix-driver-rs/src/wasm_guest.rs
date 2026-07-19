//! The `wasm32-wasip2` component bridge (issue #27).
//!
//! `wit_bindgen::generate!` below produces the guest-side low-level
//! component ABI glue for the `driver` world in `delta-abi.wit`: a `Guest`
//! trait for the world's `source`/`apply` exports, and guest-callable
//! wrappers (`bindings::brixms::delta::capabilities::{get_net, get_console,
//! Net, Console}`) for its imported `capabilities` interface — mirroring
//! `wasmtime::component::bindgen!` in `crates/brix-rt/src/driver_host.rs`
//! one for one (same WIT, same world, opposite side of the boundary).
//!
//! This module wires that generated glue to the crate's existing,
//! wasm-independent [`crate::http_notify::HttpNotifyDriver`] +
//! [`crate::run_batch`]: `on_request` bodies do not change at all (per the
//! crate-level docs' promise) — only this thin adapter is new.
//!
//! # `unsafe_code`
//!
//! The workspace denies `unsafe_code` everywhere except an explicitly
//! allowlisted module. This is the first: the component ABI's low-level
//! export trampolines (`wit_bindgen::generate!`'s `Guest`-trait plumbing and
//! `export!`'s `extern "C"` entry points) are `unsafe` by the component
//! model's own definition — that boundary is what a wasmtime host calls
//! into — and are generated code this crate does not hand-write or review
//! line by line, unlike the arena-style manual-unsafe case the workspace
//! lint doc anticipates.

#![allow(unsafe_code)]

mod bindings {
    wit_bindgen::generate!({
        world: "driver",
        path: "../driver-wit",
    });
}

use brix_canon::EdgeId;
use brix_rt::ids::DataRevision;

use crate::caps::{HttpRequest, HttpResponse, Net, NetError};
use crate::http_notify::HttpNotifyDriver;
use crate::{
    run_batch, CanonRow, DeltaBatch, DeltaOp, DeltaOutput, DeltaSource, DeltaSourceKind, Driver,
    Emission, RelationRef, RuleErrorEmission, SupportOp, SupportRecord,
};

/// The protocol relation this component's Driver answers to — the
/// HTTP-notify template's own convention (`sdk/brix-driver-rs/src/
/// http_notify.rs`'s tests use the same name).
const PROTOCOL: &str = "notify.Send";

// ---------------------------------------------------------------------------
// `caps::Net` over the generated imported `net` capability handle.
// ---------------------------------------------------------------------------

/// What this Driver instance was granted for `net`, fetched fresh at the
/// start of each `apply` call (Part VII §4: host-issued, and `none` is a
/// normal "not granted", not an error — see the WIT file's `get-net` docs).
enum NetGrant {
    Granted(bindings::brixms::delta::capabilities::Net),
    Denied,
}

fn wit_net_error_to_caps(e: bindings::brixms::delta::capabilities::NetError) -> NetError {
    match e {
        bindings::brixms::delta::capabilities::NetError::Forbidden(s) => NetError::Forbidden(s),
        bindings::brixms::delta::capabilities::NetError::Transport(s) => NetError::Transport(s),
        bindings::brixms::delta::capabilities::NetError::Cancelled => NetError::Cancelled,
    }
}

impl Net for NetGrant {
    fn request(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        let handle = match self {
            NetGrant::Granted(h) => h,
            // No grant: mirror the WIT's own `forbidden` case one level up,
            // rather than inventing a distinct "ungranted" error shape.
            NetGrant::Denied => {
                return Err(NetError::Forbidden(
                    "host did not grant a net capability to this Driver".to_string(),
                ))
            }
        };
        let wit_req = bindings::brixms::delta::capabilities::HttpRequest {
            method: req.method.clone(),
            url: req.url.clone(),
            headers: req.headers.clone(),
            body: req.body.clone(),
        };
        handle
            .request(&wit_req)
            .map(|resp| HttpResponse {
                status: resp.status,
                headers: resp.headers,
                body: resp.body,
            })
            .map_err(wit_net_error_to_caps)
    }
}

// ---------------------------------------------------------------------------
// Delta round-trip: WIT <-> engine, guest side. The mirror image of
// `crates/brix-rt/src/driver_host.rs`'s conversions — see that module's
// header for why every shape here is one-for-one, not a semantic choice.
// ---------------------------------------------------------------------------

fn edge_id_to_wit(edge: EdgeId) -> bindings::brixms::delta::types::EdgeId {
    edge.digest().as_bytes().to_vec()
}

fn support_record_to_wit(s: SupportRecord) -> bindings::brixms::delta::delta::SupportRecord {
    bindings::brixms::delta::delta::SupportRecord {
        edge: edge_id_to_wit(s.edge),
        rule: s.rule.as_str().to_string(),
        match_digest: s.match_digest.0.as_bytes().to_vec(),
    }
}

fn support_op_to_wit(op: SupportOp) -> bindings::brixms::delta::delta::SupportOp {
    match op {
        SupportOp::Add(s) => {
            bindings::brixms::delta::delta::SupportOp::Add(support_record_to_wit(s))
        }
        SupportOp::Remove(s) => {
            bindings::brixms::delta::delta::SupportOp::Remove(support_record_to_wit(s))
        }
    }
}

fn source_to_wit(source: &DeltaSource) -> bindings::brixms::delta::delta::DeltaSource {
    let kind = match &source.kind {
        DeltaSourceKind::Rule { rule, site } => {
            bindings::brixms::delta::delta::DeltaSourceKind::Rule(
                bindings::brixms::delta::delta::SourceRule {
                    rule: rule.as_str().to_string(),
                    site: site.map(|s| s.0),
                },
            )
        }
        DeltaSourceKind::Protocol { protocol } => {
            bindings::brixms::delta::delta::DeltaSourceKind::Protocol(protocol.as_str().to_string())
        }
    };
    bindings::brixms::delta::delta::DeltaSource {
        relation: source.relation.as_str().to_string(),
        kind,
    }
}

fn batch_from_wit(batch: bindings::brixms::delta::delta::DeltaBatch) -> DeltaBatch<CanonRow> {
    DeltaBatch {
        at: DataRevision(batch.at),
        ops: batch
            .ops
            .into_iter()
            .map(|op| match op {
                bindings::brixms::delta::delta::DeltaOp::Insert(row) => {
                    DeltaOp::Insert(CanonRow(row))
                }
                bindings::brixms::delta::delta::DeltaOp::Retract(row) => {
                    DeltaOp::Retract(CanonRow(row))
                }
            })
            .collect(),
    }
}

fn output_to_wit(out: DeltaOutput<CanonRow>) -> bindings::brixms::delta::delta::DeltaOutput {
    bindings::brixms::delta::delta::DeltaOutput {
        emissions: out
            .emissions
            .into_iter()
            .map(
                |e: Emission<CanonRow>| bindings::brixms::delta::delta::Emission {
                    edge: edge_id_to_wit(e.edge),
                    row: e.row.0,
                    supports: e.supports.into_iter().map(support_op_to_wit).collect(),
                },
            )
            .collect(),
        support_ops: out.support_ops.into_iter().map(support_op_to_wit).collect(),
        errors: out
            .errors
            .into_iter()
            .map(
                |e: RuleErrorEmission| bindings::brixms::delta::delta::RuleError {
                    site: e.site.0,
                    partial_match: e.partial_match.0.as_bytes().to_vec(),
                    error: e.error.0,
                },
            )
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The `Guest` export: the world's `source`/`apply`, backed by
// `HttpNotifyDriver`/`run_batch` exactly as `on_request` already implements
// them (no wasm-specific logic in the Driver itself).
// ---------------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn source() -> bindings::brixms::delta::delta::DeltaSource {
        let driver = HttpNotifyDriver::<NetGrant>::new(RelationRef::from(PROTOCOL));
        source_to_wit(Driver::source(&driver))
    }

    fn apply(
        batch: bindings::brixms::delta::delta::DeltaBatch,
    ) -> bindings::brixms::delta::delta::DeltaOutput {
        let net = match bindings::brixms::delta::capabilities::get_net() {
            Some(handle) => NetGrant::Granted(handle),
            None => NetGrant::Denied,
        };
        let mut driver = HttpNotifyDriver::<NetGrant>::new(RelationRef::from(PROTOCOL));
        let out = run_batch(&mut driver, &net, batch_from_wit(batch));
        output_to_wit(out)
    }
}

bindings::export!(Component with_types_in bindings);
