//! brix-driver-rs — Rust guest SDK for WASM Drivers.
//!
//! Generated against `sdk/driver-wit/delta-abi.wit` (the `driver` world) from
//! the delta-ABI definition. A Driver is, structurally, a delta function over
//! a protocol's request relation: it consumes a [`DeltaBatch`] of `Desired`
//! request versions and produces outcome facts — the `Succeeded`/`Failed`/
//! `Cancelled` sealed edges of Appendix H — plus the support bookkeeping that
//! grounds them (Ring0 §1.7, Part VII §3).
//!
//! This crate gives Drivers the ergonomic layer over that raw shape: instead
//! of matching [`DeltaOp`] and hand-building a [`DeltaOutput`], a Driver
//! implements [`Driver::on_request`] once per request and returns a typed
//! [`Outcome`]; the SDK's [`run_batch`] adapter turns a whole batch of those
//! into the ABI's `emissions + support ops out`.
//!
//! # Status: guest SDK sketch (Ring0 §1.11, Day-6 deliverable, previewed here)
//!
//! The delta-ABI *types* are re-exported straight from `brix-rt` so the guest
//! and host cannot drift. What is a **sketch** in this pass: the
//! `wit-bindgen`-generated bindings for the `driver` world's imports/exports
//! are not wired (that needs the wasmtime host + a bindings build step — see
//! the crate-level note in `brix-rt/src/lib.rs` on the deferred wasmtime
//! host). The capability surface a Driver calls is modeled here by the
//! [`caps`] trait objects so `on_request` bodies are already written against
//! the real shape; swapping the trait for the generated `capabilities`
//! resource is mechanical when the host lands. The HTTP-notify example
//! ([`http_notify`]) is the template Ring 1 copies.

pub use brix_rt::delta::{
    CanonRow, DeltaBatch, DeltaOp, DeltaOutput, DeltaSource, DeltaSourceKind, Emission,
    RuleErrorEmission, SupportOp, SupportRecord,
};
pub use brix_rt::ids::{MatchDigest, RelationRef, RuleRef, SiteId};

use brix_canon::EdgeId;

pub mod caps;
pub mod http_notify;

/// The typed result of handling one protocol request (Appendix H terminal
/// outcomes). A Driver returns one of these from [`Driver::on_request`]; the
/// SDK maps it to the sealed outcome edge and its support op.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// `P.Succeeded(version, outcome)` — the external effect completed. The
    /// bytes are the canon-encoded outcome payload.
    Succeeded(CanonRow),
    /// `P.Failed(version, outcome)` — a terminal failure (retry budget
    /// exhausted, or a non-retryable error). Retry *scheduling* is the
    /// engine's job over committed facts (Part VII §3); a Driver reports the
    /// honest terminal outcome, it does not loop internally.
    Failed(CanonRow),
    /// `P.Cancelled(version, outcome)` — one of Appendix H's cancellation
    /// outcomes (`BeforeStart | Acknowledged | TooLate(result) |
    /// Unsupported`). The bytes carry which.
    Cancelled(CanonRow),
}

/// One request handed to a Driver: the request version's canon-encoded
/// payload plus the identity the outcome must bind to. Appendix H is
/// emphatic that "attempts and terminal outcomes bind to a RequestVersion,
/// never to the bare key," so the Driver never sees or invents the key — it
/// only ever answers the exact version the engine leased to it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Request {
    /// The request edge this outcome will attach to.
    pub edge: EdgeId,
    /// The canon-encoded request payload (`RequestVersion`'s content).
    pub payload: CanonRow,
}

/// The guest-side Driver trait. A Driver implements this; the SDK's
/// [`run_batch`] drives it over a whole [`DeltaBatch`].
pub trait Driver {
    /// The capability set this Driver needs (e.g. [`caps::Net`]). Bundled as
    /// one associated type so a Driver's `on_request` signature names exactly
    /// the boundary it touches and nothing else (Part VII §4: capabilities
    /// say who and over which scope).
    type Caps;

    /// The delta source this Driver answers to — a protocol relation.
    fn source(&self) -> &DeltaSource;

    /// The rule/source identity outcomes are supported by, for the
    /// `Support(edge, rule, match, atRevision)` bookkeeping. For a Driver
    /// this is the protocol's own name acting as the deriving source.
    fn support_rule(&self) -> RuleRef;

    /// Handle one leased request version, using `caps` for any boundary
    /// effect, and return its terminal [`Outcome`]. Pure with respect to the
    /// model (no ambient state, no clock — `sim.Now` is a relation, Part III
    /// §10); the only impurity is through the capability handle.
    fn on_request(&mut self, caps: &Self::Caps, request: &Request) -> Outcome;
}

/// Encode one [`Outcome`] as the canon bytes of its outcome edge. The tag
/// byte keeps `Succeeded`/`Failed`/`Cancelled` from ever colliding on
/// identical payload bytes (same discipline as `RoleValue` in `brix-rt`).
fn outcome_row(outcome: &Outcome) -> CanonRow {
    use brix_canon::CanonWriter;
    let mut w = CanonWriter::new();
    match outcome {
        Outcome::Succeeded(p) => {
            w.write_uint(0);
            w.write_bytes(&p.0);
        }
        Outcome::Failed(p) => {
            w.write_uint(1);
            w.write_bytes(&p.0);
        }
        Outcome::Cancelled(p) => {
            w.write_uint(2);
            w.write_bytes(&p.0);
        }
    }
    CanonRow(w.finish())
}

/// Drive `driver` over one whole [`DeltaBatch`], producing the delta-ABI
/// [`DeltaOutput`] — this is the adapter that makes a [`Driver`] an
/// implementation of the ABI's `apply`.
///
/// - each `Insert` (a newly `Desired` request version) is handled and its
///   [`Outcome`] emitted as an outcome edge with one `Add` support op;
/// - each `Retract` (support for a version lost before terminal — Appendix
///   H's `Withdrawn`) removes the support that grounded that request; the
///   Driver's `on_request` is not called, because there is nothing to attempt.
pub fn run_batch<D: Driver>(
    driver: &mut D,
    caps: &D::Caps,
    batch: DeltaBatch<CanonRow>,
) -> DeltaOutput<CanonRow> {
    let rule = driver.support_rule();
    let mut out = DeltaOutput::empty();
    for op in batch.ops {
        match op {
            DeltaOp::Insert(payload) => {
                let request = Request {
                    edge: EdgeId::from_canon(&payload.0),
                    payload,
                };
                let outcome = driver.on_request(caps, &request);
                let row = outcome_row(&outcome);
                let edge = EdgeId::from_canon(&row.0);
                let match_digest = MatchDigest::of(&rule, &request.payload.0);
                out.emissions.push(Emission {
                    edge,
                    row,
                    supports: vec![SupportOp::Add(SupportRecord {
                        edge,
                        rule: rule.clone(),
                        match_digest,
                    })],
                });
            }
            DeltaOp::Retract(payload) => {
                // Withdrawn: the desired version lost support before a
                // terminal outcome. Remove the support that grounded the
                // request; completed history is never unwound (Part VII §3).
                let request_edge = EdgeId::from_canon(&payload.0);
                let match_digest = MatchDigest::of(&rule, &payload.0);
                out.support_ops.push(SupportOp::Remove(SupportRecord {
                    edge: request_edge,
                    rule: rule.clone(),
                    match_digest,
                }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_variants_never_collide() {
        let s = outcome_row(&Outcome::Succeeded(CanonRow(b"x".to_vec())));
        let f = outcome_row(&Outcome::Failed(CanonRow(b"x".to_vec())));
        let c = outcome_row(&Outcome::Cancelled(CanonRow(b"x".to_vec())));
        assert_ne!(s, f);
        assert_ne!(f, c);
        assert_ne!(s, c);
    }
}
