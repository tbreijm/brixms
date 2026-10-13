//! The persisted network-state header `brix.world.network-state@1` (R0 Phase 2, M4).
//!
//! One content-addressed object per committed revision. It names, by `(root, len)`, every
//! persisted trie that makes up the maintained network (join indexes, Distinct and
//! GroupedCount support indexes, support-counted derived relations, the candidate frontier
//! and the settled-decision trie) together with the *validity key* under which they were
//! produced. A `brix.world.revision@3` record binds the header digest, so the digest chain
//! authenticates the whole persisted state; the header is a cache of the journal's
//! consequences, never an authority (ADR-0046 s5).
//!
//! Encoding is canonical and injective: a fixed field order, strictly ascending map keys,
//! no trailing bytes. The decoder accepts exactly the encoding the writer produces.
//!
//! Scope of the key: program digest, execution-profile digest, lowering version and state
//! codec version. The scheduling quantum and network limits are operational and excluded:
//! they never change roots (ADR-0046 s3.8).
//!
//! Distinct-backed derived relations are *not* listed in `derived`: their contents are the
//! Distinct operator's support index, already recorded under `operators`.

#![deny(unsafe_code)]

use std::collections::BTreeMap;

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::relation_dag::OperatorId;
use soc_core::store::{NodeStore, StorageError};

use super::error::WorldError;
use super::state_codec::{read_op, read_string, StateCodec, StateCodecError, SubRoot};
use super::state_store::{StateError, StateStore};

/// Schema tag of the header object.
pub const NETWORK_STATE_SCHEMA: &str = "brix.world.network-state@2";

/// Version of the program-to-network lowering and operator semantics. Bump whenever a change
/// to the lowering, the expression compiler or an operator could change persisted state for
/// the same program; worlds written under another version replay on open.
pub const NETWORK_LOWERING_VERSION: &str = "brix.world.net-lowering@2";

/// Everything that must be identical for persisted network state to be reusable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidityKey {
    /// The world manifest's program closure digest.
    pub program_digest: Digest,
    pub exec_profile_digest: Option<Digest>,
    /// Structural fingerprint of the network the state was computed by
    /// ([`super::network::WorldNetwork::fingerprint`]): operator DAG, expressions,
    /// projections, decide blocks and functions. Differs whenever the program differs, even
    /// if the manifest does not say so.
    pub network_fingerprint: Digest,
    /// Hand-bumped for semantic changes the fingerprint cannot see.
    pub lowering_version: String,
    pub codec_version: String,
    pub evaluator: String,
    pub numeric_semantics: String,
}

impl ValidityKey {
    /// The key this binary would write for a network with `network_fingerprint` in a world
    /// bound to `program_digest`.
    pub fn current(
        program_digest: Digest,
        exec_profile_digest: Option<Digest>,
        network_fingerprint: Digest,
    ) -> Self {
        let profile = super::session::ExecProfileV1::current_for_network();
        Self {
            program_digest,
            exec_profile_digest,
            network_fingerprint,
            lowering_version: NETWORK_LOWERING_VERSION.to_string(),
            codec_version: super::state_codec::STATE_CODEC_VERSION.to_string(),
            evaluator: profile.evaluator,
            numeric_semantics: profile.numeric_semantics,
        }
    }

    /// Content address of the key (for the state-cache sidecar).
    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        self.write(&mut w);
        Digest::of(Domain::State, &w.finish())
    }

    fn write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.program_digest.as_bytes());
        put_opt_digest(w, &self.exec_profile_digest);
        w.write_bytes(self.network_fingerprint.as_bytes());
        put_str(w, &self.lowering_version);
        put_str(w, &self.codec_version);
        put_str(w, &self.evaluator);
        put_str(w, &self.numeric_semantics);
    }
}

/// Plain string read with canon primitives only (the stable header prefix must not depend
/// on `state_codec`, whose version may be what changed).
fn prefix_str(r: &mut CanonReader<'_>) -> Result<String, StateCodecError> {
    let b = r.read_bytes()?;
    std::str::from_utf8(b)
        .map(str::to_owned)
        .map_err(|_| StateCodecError::Canon(brix_canon::CanonError::BadLength))
}

fn prefix_digest(r: &mut CanonReader<'_>) -> Result<Digest, StateCodecError> {
    let b = r.read_bytes()?;
    let arr: [u8; 32] = b
        .try_into()
        .map_err(|_| StateCodecError::Canon(brix_canon::CanonError::BadLength))?;
    Ok(Digest::from_bytes(arr))
}

fn prefix_opt_digest(r: &mut CanonReader<'_>) -> Result<Option<Digest>, StateCodecError> {
    match r.read_uint()? {
        0 => Ok(None),
        1 => Ok(Some(prefix_digest(r)?)),
        t => Err(StateCodecError::UnknownTag(t)),
    }
}

fn read_validity(r: &mut CanonReader<'_>) -> Result<ValidityKey, StateCodecError> {
    Ok(ValidityKey {
        program_digest: prefix_digest(r)?,
        exec_profile_digest: prefix_opt_digest(r)?,
        network_fingerprint: prefix_digest(r)?,
        lowering_version: prefix_str(r)?,
        codec_version: prefix_str(r)?,
        evaluator: prefix_str(r)?,
        numeric_semantics: prefix_str(r)?,
    })
}

/// Outcome of loading a header under an expected validity key.
#[derive(Debug)]
pub enum Loaded {
    /// Written under exactly the expected key.
    Header(Box<NetworkStateHeader>),
    /// Intact but produced under another schema, execution profile, program structure or
    /// codec/lowering version: not reusable here, the network must be rebuilt.
    Stale(&'static str),
}

/// Persisted roots of one stateful operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OperatorRoots {
    /// `EquiJoin`: the left and right outer indexes.
    Join { left: SubRoot, right: SubRoot },
    /// `Distinct`: the typed support index.
    Distinct(SubRoot),
    /// `GroupedCount`: the group support index.
    Group {
        supports: SubRoot,
        aggregates: SubRoot,
    },
    /// `AntiJoin`: indexed left and right support rows.
    Anti { left: SubRoot, right: SubRoot },
    /// `ParentCount`: indexed parent and child rows.
    ParentCount { left: SubRoot, right: SubRoot },
}

impl OperatorRoots {
    const KIND_JOIN: u64 = 1;
    const KIND_DISTINCT: u64 = 2;
    const KIND_GROUP: u64 = 3;
    const KIND_ANTI: u64 = 4;
    const KIND_PARENT_COUNT: u64 = 5;

    /// Human-readable operator kind (for error messages).
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Join { .. } => "join",
            Self::Distinct(_) => "distinct",
            Self::Group { .. } => "group",
            Self::Anti { .. } => "anti-join",
            Self::ParentCount { .. } => "parent-count",
        }
    }
}

/// The header object. `derived` lists only support-counted (non-Distinct) derived relations.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NetworkStateHeader {
    pub validity: ValidityKey,
    pub decision: SubRoot,
    pub frontier: SubRoot,
    pub derived: BTreeMap<String, SubRoot>,
    pub operators: BTreeMap<OperatorId, OperatorRoots>,
}

fn put_str(w: &mut CanonWriter, s: &str) {
    w.write_bytes(s.as_bytes());
}

fn put_opt_digest(w: &mut CanonWriter, d: &Option<Digest>) {
    match d {
        None => w.write_uint(0),
        Some(d) => {
            w.write_uint(1);
            w.write_bytes(d.as_bytes());
        }
    }
}

fn noncanon<T>(msg: &str) -> Result<T, StateCodecError> {
    Err(StateCodecError::NonCanonical(msg.to_string()))
}

impl NetworkStateHeader {
    /// Canonical bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        put_str(&mut w, NETWORK_STATE_SCHEMA);
        self.validity.write(&mut w);
        self.decision.state_write(&mut w);
        self.frontier.state_write(&mut w);
        w.write_uint(self.derived.len() as u64);
        for (name, root) in &self.derived {
            put_str(&mut w, name);
            root.state_write(&mut w);
        }
        w.write_uint(self.operators.len() as u64);
        for (op, roots) in &self.operators {
            w.write_uint(op.0 as u64);
            match roots {
                OperatorRoots::Join { left, right } => {
                    w.write_uint(OperatorRoots::KIND_JOIN);
                    left.state_write(&mut w);
                    right.state_write(&mut w);
                }
                OperatorRoots::Distinct(r) => {
                    w.write_uint(OperatorRoots::KIND_DISTINCT);
                    r.state_write(&mut w);
                }
                OperatorRoots::Group {
                    supports,
                    aggregates,
                } => {
                    w.write_uint(OperatorRoots::KIND_GROUP);
                    supports.state_write(&mut w);
                    aggregates.state_write(&mut w);
                }
                OperatorRoots::Anti { left, right } => {
                    w.write_uint(OperatorRoots::KIND_ANTI);
                    left.state_write(&mut w);
                    right.state_write(&mut w);
                }
                OperatorRoots::ParentCount { left, right } => {
                    w.write_uint(OperatorRoots::KIND_PARENT_COUNT);
                    left.state_write(&mut w);
                    right.state_write(&mut w);
                }
            }
        }
        w.finish()
    }

    /// The header's content address (the revision's `network_state_root`).
    pub fn digest(&self) -> Digest {
        Digest::of(Domain::State, &self.encode())
    }

    /// Decode a complete header; trailing bytes and non-canonical orderings are errors.
    pub fn decode(bytes: &[u8]) -> Result<Self, StateCodecError> {
        let mut r = CanonReader::new(bytes);
        if prefix_str(&mut r)? != NETWORK_STATE_SCHEMA {
            return noncanon("network-state schema tag");
        }
        let validity = read_validity(&mut r)?;
        let decision = SubRoot::state_read(&mut r)?;
        let frontier = SubRoot::state_read(&mut r)?;

        let n_derived = r.read_uint()?;
        let mut derived = BTreeMap::new();
        let mut prev: Option<String> = None;
        for _ in 0..n_derived {
            let name = read_string(&mut r)?;
            if prev.as_ref().is_some_and(|p| p >= &name) {
                return noncanon("derived relation names not strictly ascending");
            }
            let root = SubRoot::state_read(&mut r)?;
            prev = Some(name.clone());
            derived.insert(name, root);
        }

        let n_ops = r.read_uint()?;
        let mut operators = BTreeMap::new();
        let mut prev_op: Option<OperatorId> = None;
        for _ in 0..n_ops {
            let op = read_op(&mut r)?;
            if prev_op.is_some_and(|p| p >= op) {
                return noncanon("operator ids not strictly ascending");
            }
            let roots = match r.read_uint()? {
                OperatorRoots::KIND_JOIN => OperatorRoots::Join {
                    left: SubRoot::state_read(&mut r)?,
                    right: SubRoot::state_read(&mut r)?,
                },
                OperatorRoots::KIND_DISTINCT => {
                    OperatorRoots::Distinct(SubRoot::state_read(&mut r)?)
                }
                OperatorRoots::KIND_GROUP => OperatorRoots::Group {
                    supports: SubRoot::state_read(&mut r)?,
                    aggregates: SubRoot::state_read(&mut r)?,
                },
                OperatorRoots::KIND_ANTI => OperatorRoots::Anti {
                    left: SubRoot::state_read(&mut r)?,
                    right: SubRoot::state_read(&mut r)?,
                },
                OperatorRoots::KIND_PARENT_COUNT => OperatorRoots::ParentCount {
                    left: SubRoot::state_read(&mut r)?,
                    right: SubRoot::state_read(&mut r)?,
                },
                t => return Err(StateCodecError::UnknownTag(t)),
            };
            prev_op = Some(op);
            operators.insert(op, roots);
        }
        if !r.is_empty() {
            return Err(StateCodecError::TrailingBytes);
        }
        Ok(Self {
            validity,
            decision,
            frontier,
            derived,
            operators,
        })
    }

    /// Write the header into the node store (not yet flushed) and return its digest.
    pub fn write(&self, store: &StateStore) -> Digest {
        let bytes = self.encode();
        let digest = Digest::of(Domain::State, &bytes);
        let mut s = store.clone();
        s.put_node(digest, bytes);
        digest
    }

    /// The verified bytes of the header object at `digest`. Absence is `MissingState`,
    /// bytes that do not hash to their address (or a damaged pack) are `CorruptState`, and
    /// an OS I/O failure stays a plain state error.
    fn verified_bytes(store: &StateStore, digest: &Digest) -> Result<Vec<u8>, WorldError> {
        let Some(bytes) = store
            .get_node(digest)
            .map_err(|e| WorldError::from(StateError::Storage(e)))?
        else {
            return Err(StateError::Storage(StorageError::MissingNode(*digest)).into());
        };
        if Digest::of(Domain::State, &bytes) != *digest {
            return Err(StateError::Storage(StorageError::CorruptedNode(*digest)).into());
        }
        Ok(bytes)
    }

    /// Load the header at `digest` and check it against `expected`.
    ///
    /// Only a stable prefix (schema tag, validity key) is decoded before the key is
    /// compared, using canon primitives alone: a header written under another schema or
    /// codec version is [`Loaded::Stale`], never corruption. A header for a different
    /// program is a hard `ValidityMismatch`. Everything after the prefix is decoded only
    /// under an equal key, and any failure there is `CorruptState`.
    pub fn load(
        store: &StateStore,
        digest: &Digest,
        expected: &ValidityKey,
    ) -> Result<Loaded, WorldError> {
        let bytes = Self::verified_bytes(store, digest)?;
        let corrupt = |e: StateCodecError| WorldError::from(StateError::from(e));
        let mut r = CanonReader::new(&bytes);
        if prefix_str(&mut r).map_err(corrupt)? != NETWORK_STATE_SCHEMA {
            return Ok(Loaded::Stale("schema"));
        }
        let validity = read_validity(&mut r).map_err(corrupt)?;
        if validity.program_digest != expected.program_digest {
            return Err(WorldError::ValidityMismatch(
                "persisted network state belongs to a different program".into(),
            ));
        }
        if &validity != expected {
            return Ok(Loaded::Stale("validity-key"));
        }
        Self::decode(&bytes)
            .map(|h| Loaded::Header(Box::new(h)))
            .map_err(corrupt)
    }

    /// Load and fully decode the header at `digest`, whatever its validity key.
    pub fn read(store: &StateStore, digest: &Digest) -> Result<Self, WorldError> {
        let bytes = Self::verified_bytes(store, digest)?;
        Self::decode(&bytes)
            .map_err(StateError::from)
            .map_err(WorldError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NetworkStateHeader {
        let d = |n: u8| Digest::of(Domain::State, &[n]);
        let sr = |n: u8, len: u64| SubRoot { root: d(n), len };
        let mut derived = BTreeMap::new();
        derived.insert("a::x".to_string(), sr(1, 3));
        derived.insert("b::y".to_string(), sr(2, 0));
        let mut operators = BTreeMap::new();
        operators.insert(
            OperatorId(2),
            OperatorRoots::Join {
                left: sr(3, 4),
                right: sr(4, 5),
            },
        );
        operators.insert(OperatorId(5), OperatorRoots::Distinct(sr(5, 6)));
        operators.insert(
            OperatorId(6),
            OperatorRoots::Group {
                supports: sr(6, 7),
                aggregates: sr(7, 8),
            },
        );
        NetworkStateHeader {
            validity: ValidityKey::current(d(9), Some(d(10)), d(11)),
            decision: sr(7, 8),
            frontier: sr(8, 9),
            derived,
            operators,
        }
    }

    #[test]
    fn roundtrip_and_digest_is_content_address() {
        let h = sample();
        let bytes = h.encode();
        assert_eq!(NetworkStateHeader::decode(&bytes).unwrap(), h);
        assert_eq!(h.digest(), Digest::of(Domain::State, &bytes));
    }

    #[test]
    fn decoder_rejects_trailing_bytes_and_truncation() {
        let bytes = sample().encode();
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(NetworkStateHeader::decode(&extra).is_err());
        assert!(NetworkStateHeader::decode(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn distinct_headers_have_distinct_digests() {
        let a = sample();
        let mut b = sample();
        b.frontier.len += 1;
        assert_ne!(a.digest(), b.digest());
        let mut c = sample();
        c.validity.lowering_version = "other".into();
        assert_ne!(a.digest(), c.digest());
        let mut e = sample();
        e.validity.network_fingerprint = Digest::of(Domain::State, b"other");
        assert_ne!(a.digest(), e.digest());
        let mut d = sample();
        d.operators.insert(
            OperatorId(7),
            OperatorRoots::Group {
                supports: SubRoot::empty(),
                aggregates: SubRoot::empty(),
            },
        );
        assert_ne!(a.digest(), d.digest());
    }

    #[test]
    fn load_classifies_stale_foreign_and_corrupt() {
        let store = StateStore::memory(1 << 20);
        let h = sample();
        let d = h.write(&store);
        assert!(matches!(
            NetworkStateHeader::load(&store, &d, &h.validity),
            Ok(Loaded::Header(_))
        ));
        let mut other = h.validity.clone();
        other.network_fingerprint = Digest::of(Domain::State, b"x");
        assert!(matches!(
            NetworkStateHeader::load(&store, &d, &other),
            Ok(Loaded::Stale("validity-key"))
        ));
        other.codec_version = "newer".into();
        assert!(matches!(
            NetworkStateHeader::load(&store, &d, &other),
            Ok(Loaded::Stale("validity-key"))
        ));
        let mut foreign = h.validity.clone();
        foreign.program_digest = Digest::of(Domain::State, b"p");
        assert!(matches!(
            NetworkStateHeader::load(&store, &d, &foreign),
            Err(WorldError::ValidityMismatch(_))
        ));
        // A future schema tag is stale, whatever follows it.
        let mut w = CanonWriter::new();
        put_str(&mut w, "brix.world.network-state@3");
        w.write_uint(7);
        let bytes = w.finish();
        let fd = Digest::of(Domain::State, &bytes);
        store.clone().put_node(fd, bytes);
        assert!(matches!(
            NetworkStateHeader::load(&store, &fd, &h.validity),
            Ok(Loaded::Stale("schema"))
        ));
        // Same key, body damaged after the prefix: corrupt, not stale.
        let mut bytes = h.encode();
        bytes.truncate(bytes.len() - 3);
        let cd = Digest::of(Domain::State, &bytes);
        store.clone().put_node(cd, bytes);
        assert!(matches!(
            NetworkStateHeader::load(&store, &cd, &h.validity),
            Err(WorldError::CorruptState(_))
        ));
    }

    #[test]
    fn store_roundtrip_and_failures() {
        let store = StateStore::memory(1 << 20);
        let h = sample();
        let d = h.write(&store);
        assert_eq!(NetworkStateHeader::read(&store, &d).unwrap(), h);
        let missing = Digest::of(Domain::State, b"nope");
        assert!(matches!(
            NetworkStateHeader::read(&store, &missing),
            Err(WorldError::MissingState(_))
        ));
        // Bytes stored under the wrong address are corrupt, not missing.
        let mut s = store.clone();
        s.put_node(missing, h.encode());
        assert!(matches!(
            NetworkStateHeader::read(&store, &missing),
            Err(WorldError::CorruptState(_))
        ));
        // Garbage that hashes correctly but is not a header is a codec failure.
        let junk = b"not a header".to_vec();
        let jd = Digest::of(Domain::State, &junk);
        s.put_node(jd, junk);
        assert!(matches!(
            NetworkStateHeader::read(&store, &jd),
            Err(WorldError::CorruptState(StateError::Codec(_)))
        ));
    }
}
