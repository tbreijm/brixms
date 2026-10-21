//! ADR-0048's bounded application workflow: `qty >= 1` on a keyed order.
//!
//! Requests read an immutable world revision, never caller-supplied quantities.
//! A portable artifact retains the revision, the Merkle lookup path, and a
//! checkable journal. Verification needs only that artifact and an independently
//! selected target; neither a live session nor a cached authorization is used.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::evidence::{
    check_persisted_dispatch, DispatchCertificate, Disposition, History, Reason, Receipt, Snapshot,
    DISPATCH_POLICY,
};
use serde_json::{json, Value as Json};
use soc_core::store::{MemoryNodeStore, NodeStore, StorageError, TrieMap};

use super::session::{atomic_write, WorldLockGuard};
use super::{TupleRecord, WorldError, WorldKey, WorldRevision, WorldSession, WorldTuple};

pub const DISPATCH_EVIDENCE_SCHEMA: &str = "brix.world.dispatch-evidence@1";
const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROOF_NODES: usize = 128;

/// The consumer's independently selected claim and context. `world` is the
/// manifest digest; `revision_digest` binds the exact retained state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchTarget {
    pub world: Digest,
    pub revision: u64,
    pub revision_digest: Digest,
    pub relation: String,
    pub order: String,
    pub policy: String,
}

impl DispatchTarget {
    fn to_json(&self) -> Json {
        json!({
            "world": self.world.to_hex(), "revision": self.revision,
            "revision_digest": self.revision_digest.to_hex(),
            "relation": self.relation, "order": self.order, "policy": self.policy,
        })
    }

    fn id(&self) -> Digest {
        let mut writer = CanonWriter::new();
        writer.write_tag(DISPATCH_EVIDENCE_SCHEMA);
        writer.write_bytes(self.world.as_bytes());
        writer.write_uint(self.revision);
        writer.write_bytes(self.revision_digest.as_bytes());
        writer.write_str(&self.relation);
        writer.write_str(&self.order);
        writer.write_str(&self.policy);
        writer.digest(Domain::Claim)
    }
}

/// Portable, untrusted evidence. Receiving or decoding this object grants no
/// authority: call [`verify_dispatch_evidence`] with the consumer's target.
#[derive(Clone, Debug)]
pub struct DispatchEvidence {
    pub target: DispatchTarget,
    pub revision: WorldRevision,
    pub certificate: DispatchCertificate,
    pub journal: String,
    pub nodes: BTreeMap<Digest, Vec<u8>>,
}

#[derive(Debug)]
pub enum PolicyError {
    World(Box<WorldError>),
    Refused(Disposition),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::World(error) => write!(f, "{error}"),
            Self::Refused(outcome) => write!(f, "policy request refused: {outcome:?}"),
        }
    }
}

impl std::error::Error for PolicyError {}

impl From<WorldError> for PolicyError {
    fn from(error: WorldError) -> Self {
        Self::World(Box::new(error))
    }
}

impl From<std::io::Error> for PolicyError {
    fn from(error: std::io::Error) -> Self {
        WorldError::Io(error).into()
    }
}

fn malformed(message: impl Into<String>) -> PolicyError {
    PolicyError::Refused(Disposition::Unknown {
        reason: Reason::Malformed {
            what: message.into(),
        },
    })
}

fn accepted(outcome: Disposition) -> Result<Receipt, PolicyError> {
    match outcome {
        Disposition::Satisfied(receipt) => Ok(*receipt),
        other => Err(PolicyError::Refused(other)),
    }
}

fn field<'a>(value: &'a Json, name: &str) -> Result<&'a str, PolicyError> {
    value[name]
        .as_str()
        .ok_or_else(|| malformed(format!("missing {name}")))
}

fn digest(value: &str) -> Result<Digest, PolicyError> {
    let bytes = WorldKey::from_hex(value)?.0;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| malformed("digest length"))?;
    Ok(Digest::from_bytes(bytes))
}

impl DispatchEvidence {
    pub fn encode(&self) -> Result<Vec<u8>, PolicyError> {
        let nodes: Vec<Json> = self
            .nodes
            .iter()
            .map(|(digest, bytes)| json!({"digest": digest.to_hex(), "bytes": bytes}))
            .collect();
        let bytes = serde_json::to_vec(&json!({
            "schema": DISPATCH_EVIDENCE_SCHEMA,
            "target": self.target.to_json(),
            "revision": self.revision.to_json(),
            "quantity": self.certificate.quantity,
            "certificate": {
                "order": self.certificate.order, "policy": self.certificate.policy,
                "revision": self.certificate.revision, "world": self.certificate.world,
            },
            "journal": self.journal, "nodes": nodes,
        }))
        .map_err(|error| malformed(error.to_string()))?;
        if bytes.len() > MAX_ARTIFACT_BYTES || self.nodes.len() > MAX_PROOF_NODES {
            return Err(malformed("dispatch evidence exceeds size limit"));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PolicyError> {
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(malformed("dispatch evidence exceeds size limit"));
        }
        let value: Json = serde_json::from_slice(bytes).map_err(|e| malformed(e.to_string()))?;
        if field(&value, "schema")? != DISPATCH_EVIDENCE_SCHEMA {
            return Err(malformed("unsupported dispatch evidence schema"));
        }
        let target = &value["target"];
        let target = DispatchTarget {
            world: digest(field(target, "world")?)?,
            revision: target["revision"]
                .as_u64()
                .ok_or_else(|| malformed("revision"))?,
            revision_digest: digest(field(target, "revision_digest")?)?,
            relation: field(target, "relation")?.into(),
            order: field(target, "order")?.into(),
            policy: field(target, "policy")?.into(),
        };
        let cert = &value["certificate"];
        let certificate = DispatchCertificate {
            order: field(cert, "order")?.into(),
            policy: field(cert, "policy")?.into(),
            revision: cert["revision"]
                .as_u64()
                .ok_or_else(|| malformed("certificate revision"))?,
            world: field(cert, "world")?.into(),
            quantity: value["quantity"]
                .as_i64()
                .ok_or_else(|| malformed("quantity"))?,
        };
        let entries = value["nodes"]
            .as_array()
            .ok_or_else(|| malformed("proof nodes"))?;
        if entries.len() > MAX_PROOF_NODES {
            return Err(malformed("too many proof nodes"));
        }
        let mut nodes = BTreeMap::new();
        for entry in entries {
            let bytes: Vec<u8> = serde_json::from_value(entry["bytes"].clone())
                .map_err(|error| malformed(error.to_string()))?;
            if nodes
                .insert(digest(field(entry, "digest")?)?, bytes)
                .is_some()
            {
                return Err(malformed("duplicate proof node"));
            }
        }
        Ok(Self {
            target,
            certificate,
            nodes,
            revision: WorldRevision::from_json(&value["revision"])?,
            journal: field(&value, "journal")?.into(),
        })
    }
}

/// Recheck without a world session, compiler, solver, or decision cache. The
/// caller must obtain `expected` independently, not copy it from the artifact.
pub fn verify_dispatch_evidence(
    evidence: &DispatchEvidence,
    expected: &DispatchTarget,
) -> Result<Receipt, PolicyError> {
    if &evidence.target != expected || expected.policy != DISPATCH_POLICY {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("{expected:?}"),
                found: format!("{:?}", evidence.target),
            },
        }));
    }
    if evidence.revision.seq != expected.revision
        || evidence.revision.revision_digest != expected.revision_digest
        || evidence.revision.compute_digest_for_record() != expected.revision_digest
        || evidence.certificate.world != expected.revision_digest.to_hex()
    {
        return Err(malformed("revision identity mismatch"));
    }
    if evidence.nodes.len() > MAX_PROOF_NODES
        || evidence.nodes.values().map(Vec::len).sum::<usize>() > MAX_ARTIFACT_BYTES
    {
        return Err(malformed("proof size limit"));
    }
    let mut store = MemoryNodeStore::new();
    for (digest, bytes) in &evidence.nodes {
        require_materialized_node(digest, bytes).map_err(WorldError::from)?;
        store.put_node(*digest, bytes.clone());
    }
    let tuple = read_order(
        &evidence.revision,
        &expected.relation,
        &expected.order,
        &store,
    )?;
    let quantity = order_quantity(&tuple, &expected.order)?;
    if quantity != evidence.certificate.quantity {
        return Err(malformed(
            "certificate quantity differs from revision's order",
        ));
    }
    // The journal must describe exactly the snapshot whose quantity the Merkle
    // path established. It cannot substitute a caller-invented snapshot.
    let mut trusted = History::default();
    trusted
        .record_snapshot(Snapshot {
            revision: expected.revision,
            world: expected.revision_digest.to_hex(),
            quantities: [(expected.order.clone(), quantity)].into(),
        })
        .map_err(PolicyError::Refused)?;
    accepted(trusted.use_dispatch(
        &evidence.certificate,
        &expected.order,
        &expected.policy,
        expected.revision,
    ))?;
    accepted(check_persisted_dispatch(
        &evidence.journal,
        &evidence.certificate,
        &expected.order,
        &expected.policy,
        expected.revision,
    ))
}

fn read_order<S: NodeStore>(
    revision: &WorldRevision,
    relation: &str,
    order: &str,
    store: &S,
) -> Result<WorldTuple, PolicyError> {
    let root = revision
        .relation_roots
        .get(relation)
        .ok_or_else(|| malformed("missing order relation root"))?;
    // Cardinality is not an authority input for a membership lookup.
    let trie = TrieMap::<WorldKey, WorldTuple>::from_root_digest(*root, 1);
    trie.get_with_store(&WorldKey::from_str(order), store)
        .map_err(WorldError::from)?
        .ok_or(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::MissingEvidence,
        }))
}

fn order_quantity(tuple: &WorldTuple, order: &str) -> Result<i64, PolicyError> {
    let record = TupleRecord::from_tuple(tuple)?;
    if record.get_str("id") != Some(order) {
        return Err(malformed("order key differs from id field"));
    }
    record
        .get_str("qty")
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| malformed("qty must be an Int"))
}

// Bypass decoded caches so every node used by the lookup is retained.
struct RecordingStore<'a, S> {
    inner: &'a S,
    nodes: RefCell<BTreeMap<Digest, Vec<u8>>>,
}

// A serialized lazy stub has the digest of the node it references, so digest
// equality alone does not distinguish it from the materialized node. Trie
// lookups expect a store to resolve these references, never return another
// stub. Enforce that contract for both portable and on-disk evidence reads.
fn require_materialized_node(digest: &Digest, bytes: &[u8]) -> Result<(), StorageError> {
    let tag = CanonReader::new(bytes)
        .read_uint()
        .map_err(StorageError::DecodeError)?;
    if !(1..=3).contains(&tag) {
        return Err(StorageError::CorruptedNode(*digest));
    }
    Ok(())
}

impl<S: NodeStore> NodeStore for RecordingStore<'_, S> {
    fn get_node(&self, digest: &Digest) -> Result<Option<Vec<u8>>, StorageError> {
        let bytes = self.inner.get_node(digest)?;
        if let Some(bytes) = &bytes {
            require_materialized_node(digest, bytes)?;
            self.nodes.borrow_mut().insert(*digest, bytes.clone());
        }
        Ok(bytes)
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        self.nodes.get_mut().insert(digest, bytes);
    }
}

impl WorldSession {
    /// Select the exact claim from a retained world revision. No quantity or
    /// digest comes from the requesting application.
    pub fn dispatch_target(
        &self,
        revision: u64,
        relation: &str,
        order: &str,
        policy: &str,
    ) -> Result<DispatchTarget, PolicyError> {
        let snapshot = self.pin_revision(revision)?;
        Ok(DispatchTarget {
            world: self.manifest.digest(),
            revision,
            revision_digest: snapshot.revision.revision_digest,
            relation: relation.into(),
            order: order.into(),
            policy: policy.into(),
        })
    }

    pub fn dispatch_evidence_path(&self, target: &DispatchTarget) -> PathBuf {
        self.paths.revisions_dir().join(format!(
            "{}.policy-{}.evidence",
            target.revision,
            target.id().to_hex()
        ))
    }

    /// Request, independently check, and durably retain a policy certificate.
    /// The policy is exactly `qty>=1` on the relation's `id`/`qty` record.
    /// Publication is atomic under the world writer lock; it does not change
    /// the revision identity. Repeated requests recheck the retained artifact.
    pub fn request_dispatch(
        &mut self,
        revision: u64,
        relation: &str,
        order: &str,
        policy: &str,
    ) -> Result<DispatchEvidence, PolicyError> {
        let _lock = WorldLockGuard::acquire(&self.root)?;
        self.sync_gc_epoch()?;
        let target = self.dispatch_target(revision, relation, order, policy)?;
        if policy != DISPATCH_POLICY {
            return Err(malformed("unsupported dispatch policy"));
        }
        let path = self.dispatch_evidence_path(&target);
        if path.exists() {
            return self.load_dispatch_evidence(&target);
        }
        let snapshot = self.pin_revision(revision)?;
        let store = RecordingStore {
            inner: &snapshot.node_store,
            nodes: RefCell::default(),
        };
        let tuple = read_order(&snapshot.revision, relation, order, &store)?;
        let quantity = order_quantity(&tuple, order)?;
        let mut history = History::default();
        history
            .record_snapshot(Snapshot {
                revision,
                world: target.revision_digest.to_hex(),
                quantities: [(order.to_string(), quantity)].into(),
            })
            .map_err(PolicyError::Refused)?;
        accepted(history.establish_can_dispatch(revision, order))?;
        let evidence = DispatchEvidence {
            target,
            revision: snapshot.revision,
            certificate: history
                .export_dispatch_at(order, revision)
                .ok_or_else(|| malformed("missing established certificate"))?,
            journal: history.encode_journal().map_err(PolicyError::Refused)?,
            nodes: store.nodes.into_inner(),
        };
        verify_dispatch_evidence(&evidence, &evidence.target)?;
        atomic_write(&path, &evidence.encode()?)?;
        Ok(evidence)
    }

    /// Load and recheck retained evidence. This remains usable without its
    /// original trie files: the artifact carries the verified lookup path.
    pub fn load_dispatch_evidence(
        &self,
        expected: &DispatchTarget,
    ) -> Result<DispatchEvidence, PolicyError> {
        if expected.world != self.manifest.digest() {
            return Err(malformed("world identity mismatch"));
        }
        let mut bytes = Vec::new();
        fs::File::open(self.dispatch_evidence_path(expected))?
            .take((MAX_ARTIFACT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let evidence = DispatchEvidence::decode(&bytes)?;
        verify_dispatch_evidence(&evidence, expected)?;
        Ok(evidence)
    }

    /// Use only for the consumer-selected order, policy, relation and revision
    /// in this world. A later revision requires a new request even if qty is unchanged.
    pub fn use_dispatch_evidence(
        &self,
        evidence: &DispatchEvidence,
        revision: u64,
        relation: &str,
        order: &str,
        policy: &str,
    ) -> Result<Receipt, PolicyError> {
        let expected = self.dispatch_target(revision, relation, order, policy)?;
        verify_dispatch_evidence(evidence, &expected)
    }
}
