//! The settlement checkpoint `settlements.ckpt`, schema `brix.world.settlement-checkpoint@1`:
//! every `(decide, entity)` settlement of one revision as one flat, sorted, self-verifying
//! file, so `all_settlements` is a single sequential read instead of a walk of the candidate
//! frontier trie (about 1.1 million serial node reads for 800k decisions).
//!
//! It is a *cache* in the sense of ADR-0046 s5: derived, never authoritative, never part of
//! any digest. A missing, torn, corrupt, foreign, stale or unreachable checkpoint is a miss
//! and the reader falls back to the frontier path with byte-identical output. A checkpoint
//! at an older revision is brought to HEAD by applying the (digest-authenticated)
//! `{seq}.decisions` delta bodies of the revisions after it, so a commit never rewrites it.
//!
//! Layout (all integers big-endian, strings canonical-encoded):
//! `magic "BRIXSCK1"`, `u64 revision_seq`, `32 revision_digest`, `32 validity_key_digest`,
//! `1 + 32 decision_root` (flag, root), `u64 entity count`, the body (per decide: name,
//! entity count, then per entity: name, canonical decision tuple, in `BTreeMap` order), and
//! a trailing `32`-byte digest of everything before it. The decision root is the root of the
//! trie over exactly these settlements; `verify` recomputes it.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};

use super::decision_codec::{
    compute_decision_root, decision_tuple, decode_settled_decision, SettledDecision,
};
use super::error::WorldError;
use super::paths::WorldPaths;

pub const CHECKPOINT_SCHEMA: &str = "brix.world.settlement-checkpoint@1";
const MAGIC: &[u8; 8] = b"BRIXSCK1";

pub type Settlements = BTreeMap<String, BTreeMap<String, SettledDecision>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementCheckpoint {
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub validity_key_digest: Digest,
    pub decision_root: Option<Digest>,
    pub settlements: Settlements,
}

fn corrupt(why: &str) -> WorldError {
    WorldError::NetworkError(format!("corrupt settlement checkpoint: {why}"))
}

impl SettlementCheckpoint {
    pub fn count(&self) -> usize {
        self.settlements.values().map(|m| m.len()).sum()
    }

    pub fn encode(&self) -> Vec<u8> {
        Self::encode_parts(
            self.revision_seq,
            self.revision_digest,
            self.validity_key_digest,
            self.decision_root,
            &self.settlements,
        )
    }

    /// [`Self::encode`] over borrowed settlements (no clone of a large map to write it).
    pub fn encode_parts(
        revision_seq: u64,
        revision_digest: Digest,
        validity_key_digest: Digest,
        decision_root: Option<Digest>,
        settlements: &Settlements,
    ) -> Vec<u8> {
        let count: usize = settlements.values().map(|m| m.len()).sum();
        let mut out = Vec::with_capacity(128 + count * 96);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&revision_seq.to_be_bytes());
        out.extend_from_slice(revision_digest.as_bytes());
        out.extend_from_slice(validity_key_digest.as_bytes());
        match decision_root {
            Some(r) => {
                out.push(1);
                out.extend_from_slice(r.as_bytes());
            }
            None => {
                out.push(0);
                out.extend_from_slice(&[0u8; 32]);
            }
        }
        out.extend_from_slice(&(count as u64).to_be_bytes());
        let mut w = CanonWriter::new();
        w.write_uint(settlements.len() as u64);
        for (decide, entities) in settlements {
            w.write_str(decide);
            w.write_uint(entities.len() as u64);
            for (entity, d) in entities {
                w.write_str(entity);
                w.write_bytes(decision_tuple(d).as_bytes());
            }
        }
        out.extend_from_slice(&w.finish());
        let digest = Digest::of(Domain::Value, &out);
        out.extend_from_slice(digest.as_bytes());
        out
    }

    /// Strict decode: the trailer digest, the framing, every tuple's canonical form and the
    /// declared count must all hold.
    pub fn decode(bytes: &[u8]) -> Result<Self, WorldError> {
        const HEAD: usize = 8 + 8 + 32 + 32 + 1 + 32 + 8;
        if bytes.len() < HEAD + 32 || &bytes[..8] != MAGIC {
            return Err(corrupt("bad magic or length"));
        }
        let (payload, trailer) = bytes.split_at(bytes.len() - 32);
        if Digest::of(Domain::Value, payload).as_bytes() != trailer {
            return Err(corrupt("digest mismatch"));
        }
        let digest_at = |o: usize| {
            let mut d = [0u8; 32];
            d.copy_from_slice(&payload[o..o + 32]);
            Digest::from_bytes(d)
        };
        let revision_seq = u64::from_be_bytes(payload[8..16].try_into().expect("8 bytes"));
        let revision_digest = digest_at(16);
        let validity_key_digest = digest_at(48);
        let decision_root = match payload[80] {
            0 => None,
            1 => Some(digest_at(81)),
            _ => return Err(corrupt("bad root flag")),
        };
        let count = u64::from_be_bytes(payload[113..121].try_into().expect("8 bytes"));
        let mut r = CanonReader::new(&payload[HEAD..]);
        let cerr = |_| corrupt("framing");
        let n_decides = r.read_uint().map_err(cerr)?;
        let mut settlements: Settlements = BTreeMap::new();
        let mut seen = 0u64;
        for _ in 0..n_decides {
            let decide = std::str::from_utf8(r.read_bytes().map_err(cerr)?)
                .map_err(|_| corrupt("utf8"))?
                .to_owned();
            let n = r.read_uint().map_err(cerr)?;
            if n == 0
                || settlements
                    .last_key_value()
                    .is_some_and(|(p, _)| *p >= decide)
            {
                return Err(corrupt("decide order"));
            }
            let mut entities = BTreeMap::new();
            for _ in 0..n {
                let entity = std::str::from_utf8(r.read_bytes().map_err(cerr)?)
                    .map_err(|_| corrupt("utf8"))?
                    .to_owned();
                if entities
                    .last_key_value()
                    .is_some_and(|(p, _): (&String, &SettledDecision)| *p >= entity)
                {
                    return Err(corrupt("entity order"));
                }
                let d = decode_settled_decision(&entity, r.read_bytes().map_err(cerr)?)?;
                entities.insert(entity, d);
                seen += 1;
            }
            settlements.insert(decide, entities);
        }
        if !r.is_empty() || seen != count {
            return Err(corrupt("count or trailing bytes"));
        }
        Ok(Self {
            revision_seq,
            revision_digest,
            validity_key_digest,
            decision_root,
            settlements,
        })
    }

    /// Read the checkpoint file; any failure is `None` (a miss).
    pub fn read(paths: &WorldPaths) -> Option<Self> {
        Self::decode(&fs::read(paths.settlement_checkpoint_file()).ok()?).ok()
    }

    /// Write atomically: a process-unique temp file, then rename. Not fsynced: a torn file
    /// fails the trailer digest and is a miss. Concurrent writers each rename a complete
    /// file; for one revision their contents are identical.
    pub fn write_bytes(
        paths: &WorldPaths,
        bytes: &[u8],
        crash_before_rename: bool,
    ) -> std::io::Result<()> {
        let tmp = paths
            .settlement_checkpoint_file()
            .with_extension(format!("tmp.{}", std::process::id()));
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        drop(f);
        if crash_before_rename {
            return Err(std::io::Error::other("injected crash before rename"));
        }
        fs::rename(&tmp, paths.settlement_checkpoint_file())
    }

    /// Recompute the decision root from the settlements (an O(n) trie build) and compare it
    /// with the one the checkpoint claims.
    pub fn verify_root(&self) -> Result<(), WorldError> {
        let computed = if self.settlements.is_empty() {
            None
        } else {
            Some(compute_decision_root(&self.settlements))
        };
        match (computed, self.decision_root) {
            (Some(c), Some(d)) if c == d => Ok(()),
            (None, _) => Ok(()),
            _ => Err(corrupt("decision root does not match the settlements")),
        }
    }
}

/// Apply one revision's decision delta to `settlements`.
pub fn apply_delta(
    settlements: &mut Settlements,
    delta: BTreeMap<(String, String), Option<SettledDecision>>,
) {
    for ((decide, entity), value) in delta {
        match value {
            Some(d) => {
                settlements.entry(decide).or_default().insert(entity, d);
            }
            None => {
                if let Some(m) = settlements.get_mut(&decide) {
                    m.remove(&entity);
                    if m.is_empty() {
                        settlements.remove(&decide);
                    }
                }
            }
        }
    }
}
