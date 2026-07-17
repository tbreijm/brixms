//! The HTTP-notify Driver — the template Ring 1 copies (Ring0 §1.11: "one
//! example Driver (HTTP notify) shipped as the template Ring 1 copies").
//!
//! It answers a notification protocol: each `Desired` request version carries
//! a canon-encoded `{ url, body }`; the Driver POSTs the body to the url over
//! its `Net` capability and reports the terminal [`Outcome`]:
//!
//! - 2xx response          -> [`Outcome::Succeeded`] (the status code, canon-encoded);
//! - any other status      -> [`Outcome::Failed`]    (the status code);
//! - transport / forbidden -> [`Outcome::Failed`]    (a canon-encoded reason);
//! - host cancellation     -> [`Outcome::Cancelled`] (Appendix H `Acknowledged`).
//!
//! Note what the Driver does *not* do: it does not retry (retry scheduling is
//! the engine's, over committed facts — Part VII §3), it does not read a
//! clock (backoff timers are clock facts the engine owns), and it reports a
//! non-2xx as a Driver-level `Failed` outcome rather than pretending success.
//! Those three restraints are the whole point of the template.

use brix_canon::{CanonReader, CanonWriter};

use crate::caps::{HttpRequest, Net, NetError};
use crate::{
    CanonRow, DeltaSource, DeltaSourceKind, Driver, Outcome, RelationRef, Request, RuleRef,
};

/// The decoded request payload for an HTTP notification.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Notification {
    /// Target URL to POST to.
    pub url: String,
    /// Body to send.
    pub body: Vec<u8>,
}

impl Notification {
    /// Canon-encode this notification as a request payload (what a `Desired`
    /// request version would carry). Provided so tests and the flagship can
    /// build payloads through the *same* encoder the Driver decodes with —
    /// there is exactly one serializer (Ring0 §0).
    pub fn to_payload(&self) -> CanonRow {
        let mut w = CanonWriter::new();
        w.write_str(&self.url);
        w.write_bytes(&self.body);
        CanonRow(w.finish())
    }

    /// Decode a request payload. Returns `None` on a malformed payload,
    /// which the Driver reports as a `Failed` outcome rather than panicking.
    pub fn from_payload(payload: &CanonRow) -> Option<Notification> {
        let mut r = CanonReader::new(&payload.0);
        let url = core::str::from_utf8(r.read_bytes().ok()?).ok()?.to_string();
        let body = r.read_bytes().ok()?.to_vec();
        if !r.is_empty() {
            return None;
        }
        Some(Notification { url, body })
    }
}

fn canon_u64(v: u64) -> CanonRow {
    let mut w = CanonWriter::new();
    w.write_uint(v);
    CanonRow(w.finish())
}

fn canon_reason(reason: &str) -> CanonRow {
    let mut w = CanonWriter::new();
    w.write_str(reason);
    CanonRow(w.finish())
}

/// The HTTP-notify Driver. Generic over its `Net` capability type `N` so it
/// is unit-testable with a fake and production-usable with the host-issued
/// handle, with no code change (Part VII §5: the wall between profiles is the
/// deployment command, never the program text). `N` is a compile-time
/// marker here — the handle itself is passed to `on_request`, not stored —
/// so the type stays a zero-sized field.
pub struct HttpNotifyDriver<N> {
    source: DeltaSource,
    rule: RuleRef,
    _net: core::marker::PhantomData<fn() -> N>,
}

impl<N: Net> HttpNotifyDriver<N> {
    /// Build a Driver bound to protocol relation `protocol` (e.g.
    /// `"notify.Send"`).
    pub fn new(protocol: RelationRef) -> Self {
        let rule = RuleRef::from(protocol.as_str());
        HttpNotifyDriver {
            source: DeltaSource {
                relation: protocol.clone(),
                kind: DeltaSourceKind::Protocol { protocol },
            },
            rule,
            _net: core::marker::PhantomData,
        }
    }
}

impl<N: Net> Driver for HttpNotifyDriver<N> {
    type Caps = N;

    fn source(&self) -> &DeltaSource {
        &self.source
    }

    fn support_rule(&self) -> RuleRef {
        self.rule.clone()
    }

    fn on_request(&mut self, net: &N, request: &Request) -> Outcome {
        let Some(notification) = Notification::from_payload(&request.payload) else {
            return Outcome::Failed(canon_reason("malformed-notification-payload"));
        };
        let http = HttpRequest {
            method: "POST".to_string(),
            url: notification.url,
            headers: vec![(
                "content-type".to_string(),
                "application/octet-stream".to_string(),
            )],
            body: notification.body,
        };
        match net.request(&http) {
            Ok(resp) if (200..300).contains(&resp.status) => {
                Outcome::Succeeded(canon_u64(resp.status as u64))
            }
            Ok(resp) => Outcome::Failed(canon_u64(resp.status as u64)),
            Err(NetError::Forbidden(why)) => {
                Outcome::Failed(canon_reason(&format!("forbidden:{why}")))
            }
            Err(NetError::Transport(why)) => {
                Outcome::Failed(canon_reason(&format!("transport:{why}")))
            }
            // Appendix H: host-cancelled in-flight attempt -> Acknowledged.
            Err(NetError::Cancelled) => Outcome::Cancelled(canon_reason("Acknowledged")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::HttpResponse;
    use crate::{run_batch, DeltaBatch, DeltaOp, SupportOp};
    use brix_rt::ids::DataRevision;
    use std::cell::RefCell;

    /// A fake `Net` that records requests and replays a scripted result.
    struct FakeNet {
        result: Result<HttpResponse, NetError>,
        seen: RefCell<Vec<HttpRequest>>,
    }

    impl FakeNet {
        fn ok(status: u16) -> Self {
            FakeNet {
                result: Ok(HttpResponse {
                    status,
                    headers: vec![],
                    body: vec![],
                }),
                seen: RefCell::new(vec![]),
            }
        }
        fn err(e: NetError) -> Self {
            FakeNet {
                result: Err(e),
                seen: RefCell::new(vec![]),
            }
        }
    }

    impl Net for FakeNet {
        fn request(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
            self.seen.borrow_mut().push(req.clone());
            self.result.clone()
        }
    }

    fn notif() -> Notification {
        Notification {
            url: "https://example.test/hook".to_string(),
            body: b"payload".to_vec(),
        }
    }

    #[test]
    fn payload_round_trips_through_one_serializer() {
        let n = notif();
        let decoded = Notification::from_payload(&n.to_payload()).expect("valid payload decodes");
        assert_eq!(decoded, n);
    }

    #[test]
    fn malformed_payload_fails_not_panics() {
        assert!(Notification::from_payload(&CanonRow(vec![0xff, 0xff])).is_none());
    }

    #[test]
    fn success_on_2xx() {
        let mut d = HttpNotifyDriver::new(RelationRef::from("notify.Send"));
        let net = FakeNet::ok(202);
        let req = Request {
            edge: brix_canon::EdgeId::from_canon(b"e"),
            payload: notif().to_payload(),
        };
        assert_eq!(d.on_request(&net, &req), Outcome::Succeeded(canon_u64(202)));
        assert_eq!(net.seen.borrow().len(), 1);
        assert_eq!(net.seen.borrow()[0].method, "POST");
    }

    #[test]
    fn non_2xx_is_failed_not_success() {
        let mut d = HttpNotifyDriver::new(RelationRef::from("notify.Send"));
        let net = FakeNet::ok(500);
        let req = Request {
            edge: brix_canon::EdgeId::from_canon(b"e"),
            payload: notif().to_payload(),
        };
        assert_eq!(d.on_request(&net, &req), Outcome::Failed(canon_u64(500)));
    }

    #[test]
    fn host_cancel_is_acknowledged_cancellation() {
        let mut d = HttpNotifyDriver::new(RelationRef::from("notify.Send"));
        let net = FakeNet::err(NetError::Cancelled);
        let req = Request {
            edge: brix_canon::EdgeId::from_canon(b"e"),
            payload: notif().to_payload(),
        };
        assert_eq!(
            d.on_request(&net, &req),
            Outcome::Cancelled(canon_reason("Acknowledged"))
        );
    }

    #[test]
    fn run_batch_emits_outcome_with_add_support_and_withdrawn_removes() {
        let mut d = HttpNotifyDriver::new(RelationRef::from("notify.Send"));
        let net = FakeNet::ok(200);
        let payload = notif().to_payload();
        let batch = DeltaBatch {
            at: DataRevision(1),
            ops: vec![DeltaOp::Insert(payload.clone()), DeltaOp::Retract(payload)],
        };
        let out = run_batch(&mut d, &net, batch);
        // One Insert -> one emission grounded by exactly one Add.
        assert_eq!(out.emissions.len(), 1);
        assert_eq!(out.emissions[0].supports.len(), 1);
        assert!(matches!(out.emissions[0].supports[0], SupportOp::Add(_)));
        // One Retract (Withdrawn) -> one bare Remove, no on_request call.
        assert_eq!(out.support_ops.len(), 1);
        assert!(matches!(out.support_ops[0], SupportOp::Remove(_)));
        // Only the Insert reached the network.
        assert_eq!(net.seen.borrow().len(), 1);
        assert!(out.errors.is_empty());
    }
}
