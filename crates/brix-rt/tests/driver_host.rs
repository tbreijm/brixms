//! Conformance test for the wasmtime Driver host (issue #27, Ring0 §1.7/§1.11).
//!
//! Loads the SDK's `http_notify` example Driver as a real `wasm32-wasip2`
//! component and drives it through [`brix_rt::driver_host`], asserting the
//! full round trip: the guest receives the exact input delta, and the host
//! observes the guest's outbound capability call and its resulting outcome
//! delta — end to end through the wasmtime component boundary, not mocked
//! out at any point on the Rust side.
//!
//! # Where the component comes from
//!
//! `fixtures/http_notify_driver.component.wasm` is a **committed, prebuilt**
//! artifact: `wasm32-wasip2` (and a component-producing Rust toolchain) may
//! not be available in every environment this test runs in (CI included),
//! and the acceptance bar for issue #27 is that `cargo test --workspace`
//! exercises the real host round trip **by default**, unconditionally — not
//! a best-effort skip. Building the guest is otherwise ordinary:
//!
//! ```sh
//! rustup target add wasm32-wasip2
//! cargo build -p brix-driver-rs --target wasm32-wasip2 --release
//! cp target/wasm32-wasip2/release/brix_driver_rs.wasm \
//!    crates/brix-rt/tests/fixtures/http_notify_driver.component.wasm
//! ```
//!
//! No `wasm-tools`/`cargo-component` step is needed: rustc's `wasm32-wasip2`
//! backend emits the component binary format directly for the `cdylib`
//! wit-bindgen produces (verified by inspecting the output's binary version/
//! layer bytes — `0d 00 01 00`, the component-model preamble, not `01 00 00
//! 00`, the core-module one).
//!
//! Re-run the commands above and re-commit whenever `delta-abi.wit` or
//! `sdk/brix-driver-rs`'s guest bridge (`src/wasm_guest.rs`) changes.

use brix_canon::{CanonReader, CanonWriter};
use brix_rt::delta::{CanonRow, DeltaBatch, DeltaOp, DeltaSourceKind, SupportOp};
use brix_rt::driver_host::{CapabilityGrants, DriverHost, HttpResponse, NetError, RecordingNet};
use brix_rt::ids::DataRevision;

const COMPONENT_BYTES: &[u8] = include_bytes!("fixtures/http_notify_driver.component.wasm");

/// Canon-encode `{ url, body }` exactly as `sdk/brix-driver-rs`'s
/// `http_notify::Notification::to_payload` does (Ring0 §0: one serializer;
/// this test does not import the SDK crate, so it mirrors the two-field
/// encoding directly rather than adding a dependency edge for it).
fn notification_payload(url: &str, body: &[u8]) -> CanonRow {
    let mut w = CanonWriter::new();
    w.write_str(url);
    w.write_bytes(body);
    CanonRow(w.finish())
}

/// Decode one outcome row's tag byte (0 = Succeeded, 1 = Failed,
/// 2 = Cancelled) and its inner payload bytes, mirroring
/// `http_notify_driver`'s `outcome_row` encoding.
fn decode_outcome_tag(row: &CanonRow) -> (u64, Vec<u8>) {
    let mut r = CanonReader::new(&row.0);
    let tag = r.read_uint().expect("outcome row has a tag");
    let payload = r.read_bytes().expect("outcome row has a payload").to_vec();
    (tag, payload)
}

fn decode_canon_u64(bytes: &[u8]) -> u64 {
    let mut r = CanonReader::new(bytes);
    r.read_uint().expect("canon-encoded u64")
}

#[test]
fn http_notify_component_reports_source() {
    let host = DriverHost::new().expect("driver host construction");
    let component = host
        .load_component(COMPONENT_BYTES)
        .expect("component loads");
    let net = RecordingNet::scripted(vec![]);
    let grants = CapabilityGrants {
        net: Some(std::sync::Arc::new(net)),
        console: None,
    };
    let mut instance = host
        .instantiate(&component, grants)
        .expect("component instantiates");

    let source = instance.source().expect("source() call succeeds");
    assert_eq!(source.relation.as_str(), "notify.Send");
    match source.kind {
        DeltaSourceKind::Protocol { protocol } => {
            assert_eq!(protocol.as_str(), "notify.Send");
        }
        other => panic!("expected a Protocol delta source, got {other:?}"),
    }
}

/// The full happy-path round trip: host sends an `Insert` (a `Desired`
/// notification request) and a `Retract` (a withdrawn one) in one batch;
/// asserts the guest received exactly that input (via the *observable*
/// outbound HTTP call the `RecordingNet` capability captured — the guest
/// never gets to fake this, it has to actually decode the payload and call
/// the capability with it), and that the host received back a well-formed
/// `Succeeded` outcome for the insert and a bare support `Remove` for the
/// retract, with no call to the network for the retract.
#[test]
fn http_notify_component_round_trips_insert_and_retract() {
    let host = DriverHost::new().expect("driver host construction");
    let component = host
        .load_component(COMPONENT_BYTES)
        .expect("component loads");

    let net = RecordingNet::scripted(vec![Ok(HttpResponse {
        status: 202,
        headers: vec![],
        body: vec![],
    })]);
    let net = std::sync::Arc::new(net);
    let grants = CapabilityGrants {
        net: Some(net.clone()),
        console: None,
    };
    let mut instance = host
        .instantiate(&component, grants)
        .expect("component instantiates");

    let insert_payload = notification_payload("https://example.test/hook", b"payload-bytes");
    let retract_payload = notification_payload("https://example.test/other", b"unrelated");
    let batch = DeltaBatch {
        at: DataRevision(1),
        ops: vec![
            DeltaOp::Insert(insert_payload.clone()),
            DeltaOp::Retract(retract_payload),
        ],
    };

    let out = instance.apply(batch).expect("apply() call succeeds");

    // The host observed the guest's real outbound call — proof the guest
    // actually decoded the canon-encoded request payload it was handed
    // (not just echoed bytes back).
    let seen = net.seen();
    assert_eq!(seen.len(), 1, "only the Insert should reach the network");
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].url, "https://example.test/hook");
    assert_eq!(seen[0].body, b"payload-bytes");
    assert_eq!(
        seen[0]
            .headers
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| v.as_str()),
        Some("application/octet-stream")
    );

    // One emission (the Insert's outcome), grounded by exactly one Add.
    assert_eq!(out.emissions.len(), 1);
    let emission = &out.emissions[0];
    assert_eq!(emission.supports.len(), 1);
    assert!(matches!(emission.supports[0], SupportOp::Add(_)));
    let (tag, payload) = decode_outcome_tag(&emission.row);
    assert_eq!(tag, 0, "Succeeded tag");
    assert_eq!(decode_canon_u64(&payload), 202);

    // The Retract (Withdrawn): a bare support Remove, nothing else.
    assert_eq!(out.support_ops.len(), 1);
    assert!(matches!(out.support_ops[0], SupportOp::Remove(_)));
    assert!(out.errors.is_empty());
}

/// Lease/cancel plumbing (issue #27's other named scope item): when the
/// host cancels an in-flight capability call — modeling lease expiry or
/// shutdown, Appendix H — the guest must see `net-error.cancelled` and
/// answer with the honest `Cancelled` outcome, not a spurious `Failed`.
#[test]
fn host_cancellation_round_trips_to_cancelled_outcome() {
    let host = DriverHost::new().expect("driver host construction");
    let component = host
        .load_component(COMPONENT_BYTES)
        .expect("component loads");

    let net = RecordingNet::scripted(vec![Err(NetError::Cancelled)]);
    let grants = CapabilityGrants {
        net: Some(std::sync::Arc::new(net)),
        console: None,
    };
    let mut instance = host
        .instantiate(&component, grants)
        .expect("component instantiates");

    let payload = notification_payload("https://example.test/hook", b"body");
    let batch = DeltaBatch {
        at: DataRevision(1),
        ops: vec![DeltaOp::Insert(payload)],
    };
    let out = instance.apply(batch).expect("apply() call succeeds");

    assert_eq!(out.emissions.len(), 1);
    let (tag, _payload) = decode_outcome_tag(&out.emissions[0].row);
    assert_eq!(tag, 2, "Cancelled tag (Appendix H Acknowledged)");
}

/// A Driver instance the host never grants a `net` capability to: the guest
/// must degrade to an honest `Failed` outcome, never a panic or a trap, and
/// must never reach whatever `net` implementation exists (there is none to
/// reach here — no grant was made at all).
#[test]
fn ungranted_net_capability_yields_failed_not_a_trap() {
    let host = DriverHost::new().expect("driver host construction");
    let component = host
        .load_component(COMPONENT_BYTES)
        .expect("component loads");

    let grants = CapabilityGrants {
        net: None,
        console: None,
    };
    let mut instance = host
        .instantiate(&component, grants)
        .expect("component instantiates");

    let payload = notification_payload("https://example.test/hook", b"body");
    let batch = DeltaBatch {
        at: DataRevision(1),
        ops: vec![DeltaOp::Insert(payload)],
    };
    let out = instance.apply(batch).expect("apply() call succeeds");

    assert_eq!(out.emissions.len(), 1);
    let (tag, _payload) = decode_outcome_tag(&out.emissions[0].row);
    assert_eq!(tag, 1, "Failed tag — no net grant, not a trap");
}
