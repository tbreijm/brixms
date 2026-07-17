//! Transactions (Part VII §2) and the per-kind ground key-conflict rules
//! that fire at commit time (Part III §8's first three sub-cases).
//!
//! A transaction is a batch of ground operations read against **one settled
//! snapshot** and committed atomically as a later revision, or failed (Part
//! III §4, Part VII §2). Snapshot isolation is structural here: an
//! [`crate::store::Store`] hands a transaction an immutable clone of the
//! committed ground state; the transaction mutates its own working copy and
//! either the whole copy is swapped in (commit) or discarded (abort). No API
//! observes intra-settlement or intra-transaction state (Appendix I.11).
//!
//! Implemented (Part III §8):
//! - **Ground `rel`** — assert with a key matching a live tuple of different
//!   content is a conflict unless the prior claim is retracted/superseded in
//!   the same transaction.
//! - **`state rel`** — `set` atomically supersedes the version the
//!   transaction read; two conflicting `set`s on one key within a
//!   transaction conflict.
//! - **`event rel`** — reasserting an `EventId` with identical content is
//!   idempotent; with different content, fails.
//! - **`entity`** — `ensure` returns-or-creates; disagreeing non-key fields
//!   within one transaction fail it (see `spec/errata/0001`).
//!
//! Stubbed / out of scope (documented, not silent): serializable
//! read/write/predicate conflict *detection across concurrent transactions*
//! (Appendix I.6, I.11) — the oracle commits transactions one at a time
//! against the latest revision, so cross-transaction write-write is a
//! degenerate no-op here; genuine concurrency lives in `brix-rt`. `retract`
//! consumes a `ClaimRef` but affine-use enforcement (double-retract
//! rejection) is the type system's job (brix-ir), not settlement's.

use brix_canon::ClaimId;

use crate::program::{RelKind, RelName, RelationDef};
use crate::row::{row_key, EdgeRecord, Row};
use crate::store::GroundState;

/// One ground operation inside a transaction. Mirrors the `TxExpr` forms of
/// Appendix D relevant to the kernel proof (`ensure`/`fresh`/`assert`/`set`/
/// `retract`/`supersede`).
#[derive(Clone, Debug)]
pub enum Op {
    /// `ensure Entity { ... }` — return-or-create a keyed entity (Part VII
    /// §2). Idempotent on identical content; conflicting non-key fields for
    /// one key within a transaction fail it (errata 0001).
    Ensure { rel: RelName, row: Row },
    /// `assert R(...)` — assert a ground relation tuple, returning a
    /// `ClaimRef`. The returned claim id is deterministic from the intent
    /// (`ClaimId::from_canon` over the tx intent + op ordinal + row).
    Assert { rel: RelName, row: Row },
    /// `set StateR(...)` — assert-and-supersede a state relation (Part VII
    /// §2). Supersedes whatever version currently lives under the key.
    Set { rel: RelName, row: Row },
    /// `assert EventR(...)` — an event assertion (immutable identity).
    Event { rel: RelName, row: Row },
    /// `retract claim` — withdraw one source's claim on a ground edge.
    Retract { rel: RelName, claim: ClaimId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxnError {
    UnknownRelation(RelName),
    /// Ground `rel`: key matches a live tuple with different content and the
    /// prior claim was not retracted/superseded in this transaction.
    GroundKeyConflict {
        rel: RelName,
        key: Vec<u8>,
    },
    /// `event rel`: same `EventId`, different content.
    EventContentMismatch {
        rel: RelName,
    },
    /// `entity`: two disagreeing non-key field sets for one key in one txn.
    EntityFieldConflict {
        rel: RelName,
    },
    /// Op kind used on a relation of the wrong kind (e.g. `set` on a
    /// non-state relation).
    WrongRelKind {
        rel: RelName,
        op: &'static str,
    },
}

impl std::fmt::Display for TxnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxnError::UnknownRelation(r) => write!(f, "unknown relation `{r}`"),
            TxnError::GroundKeyConflict { rel, .. } => {
                write!(f, "ground key conflict on `{rel}` (Part III §8)")
            }
            TxnError::EventContentMismatch { rel } => {
                write!(
                    f,
                    "event `{rel}` reasserted with different content (Part III §8)"
                )
            }
            TxnError::EntityFieldConflict { rel } => {
                write!(
                    f,
                    "entity `{rel}` ensured with conflicting non-key fields (errata 0001)"
                )
            }
            TxnError::WrongRelKind { rel, op } => {
                write!(f, "operation `{op}` not valid on relation `{rel}`")
            }
        }
    }
}
impl std::error::Error for TxnError {}

/// A transaction: an intent identity (retry-stable — Part VII §2) plus an
/// ordered list of ops. The intent bytes seed every `ClaimId` so retries of
/// the same intent produce the same claim references (Appendix I.2).
#[derive(Clone, Debug)]
pub struct Transaction {
    pub intent: Vec<u8>,
    pub ops: Vec<Op>,
}

impl Transaction {
    pub fn new(intent: impl Into<Vec<u8>>) -> Self {
        Transaction {
            intent: intent.into(),
            ops: Vec::new(),
        }
    }

    pub fn ensure(mut self, rel: impl Into<RelName>, row: Row) -> Self {
        self.ops.push(Op::Ensure {
            rel: rel.into(),
            row,
        });
        self
    }
    pub fn assert(mut self, rel: impl Into<RelName>, row: Row) -> Self {
        self.ops.push(Op::Assert {
            rel: rel.into(),
            row,
        });
        self
    }
    pub fn set(mut self, rel: impl Into<RelName>, row: Row) -> Self {
        self.ops.push(Op::Set {
            rel: rel.into(),
            row,
        });
        self
    }
    pub fn event(mut self, rel: impl Into<RelName>, row: Row) -> Self {
        self.ops.push(Op::Event {
            rel: rel.into(),
            row,
        });
        self
    }
    pub fn retract(mut self, rel: impl Into<RelName>, claim: ClaimId) -> Self {
        self.ops.push(Op::Retract {
            rel: rel.into(),
            claim,
        });
        self
    }

    /// Deterministic `ClaimId` for the `ordinal`-th op of this transaction
    /// (Part III §3: `ClaimId = Hash(transaction intent, operation ordinal,
    /// source scope)`; the oracle treats one transaction as one source).
    pub fn claim_id(&self, ordinal: usize) -> ClaimId {
        let mut w = brix_canon::CanonWriter::new();
        w.write_bytes(&self.intent);
        w.write_uint(ordinal as u64);
        ClaimId::from_canon(&w.finish())
    }
}

/// Apply `txn` to a clone of `base`, returning the new ground state or an
/// error (the whole transaction is atomic — Part VII §2). `base` is never
/// mutated: snapshot isolation is the caller cloning before calling and
/// swapping on success (see `crate::store::Store::commit`).
pub fn apply(
    base: &GroundState,
    relations: &std::collections::BTreeMap<RelName, RelationDef>,
    txn: &Transaction,
) -> Result<GroundState, TxnError> {
    let mut working = base.clone();

    for (ordinal, op) in txn.ops.iter().enumerate() {
        match op {
            Op::Ensure { rel, row } => {
                let def = relations
                    .get(rel)
                    .ok_or_else(|| TxnError::UnknownRelation(rel.clone()))?;
                if def.kind != RelKind::Entity {
                    return Err(TxnError::WrongRelKind {
                        rel: rel.clone(),
                        op: "ensure",
                    });
                }
                apply_keyed_upsert(
                    &mut working,
                    def,
                    row,
                    txn.claim_id(ordinal),
                    // ensure: reject a live row under the same key with
                    // different content (errata 0001).
                    TxnError::EntityFieldConflict { rel: rel.clone() },
                )?;
            }
            Op::Assert { rel, row } => {
                let def = relations
                    .get(rel)
                    .ok_or_else(|| TxnError::UnknownRelation(rel.clone()))?;
                if def.kind != RelKind::Ground {
                    return Err(TxnError::WrongRelKind {
                        rel: rel.clone(),
                        op: "assert",
                    });
                }
                apply_keyed_upsert(
                    &mut working,
                    def,
                    row,
                    txn.claim_id(ordinal),
                    TxnError::GroundKeyConflict {
                        rel: rel.clone(),
                        key: def.key_bytes(row),
                    },
                )?;
            }
            Op::Event { rel, row } => {
                let def = relations
                    .get(rel)
                    .ok_or_else(|| TxnError::UnknownRelation(rel.clone()))?;
                if def.kind != RelKind::Event {
                    return Err(TxnError::WrongRelKind {
                        rel: rel.clone(),
                        op: "event",
                    });
                }
                apply_keyed_upsert(
                    &mut working,
                    def,
                    row,
                    txn.claim_id(ordinal),
                    TxnError::EventContentMismatch { rel: rel.clone() },
                )?;
            }
            Op::Set { rel, row } => {
                let def = relations
                    .get(rel)
                    .ok_or_else(|| TxnError::UnknownRelation(rel.clone()))?;
                if def.kind != RelKind::State {
                    return Err(TxnError::WrongRelKind {
                        rel: rel.clone(),
                        op: "set",
                    });
                }
                // `set` supersedes: drop every live row under the same key
                // (Part III §8 / Part VII §2), then insert the new version.
                let key_bytes = def.key_bytes(row);
                let extent = working.live.entry(rel.clone()).or_default();
                extent.retain(|_, rec| def.key_bytes(&rec.row) != key_bytes);
                insert_row(extent, row.clone(), txn.claim_id(ordinal));
                record_history(&mut working, rel, row.clone(), txn.claim_id(ordinal), def);
            }
            Op::Retract { rel, claim } => {
                let def = relations
                    .get(rel)
                    .ok_or_else(|| TxnError::UnknownRelation(rel.clone()))?;
                let _ = def;
                if let Some(extent) = working.live.get_mut(rel) {
                    let mut empties = Vec::new();
                    for (key, rec) in extent.iter_mut() {
                        rec.claims.remove(claim);
                        if !rec.is_live() {
                            empties.push(key.clone());
                        }
                    }
                    for key in empties {
                        extent.remove(&key);
                    }
                }
            }
        }
    }

    Ok(working)
}

/// Insert-or-merge-claims a keyed row into a live extent. When a live row
/// already exists under the same key, applies the relation's per-kind
/// conflict rule (Part III §8): identical content is idempotent (merge the
/// claim); differing content raises `on_conflict`.
fn apply_keyed_upsert(
    working: &mut GroundState,
    def: &RelationDef,
    row: &Row,
    claim: ClaimId,
    on_conflict: TxnError,
) -> Result<(), TxnError> {
    let key_bytes = def.key_bytes(row);
    let new_key = row_key(row);
    let extent = working.live.entry(def.name.clone()).or_default();

    // A live extent holds at most one row per key (every keyed insert goes
    // through this function or `set`, which supersede-drop same-key rows), so
    // "the existing same-key row, if any" is a single lookup.
    let existing_same_key = extent
        .iter()
        .find(|(_, rec)| def.key_bytes(&rec.row) == key_bytes)
        .map(|(k, _)| k.clone());

    match existing_same_key {
        Some(k) if k == new_key => {
            // Identical content under the same key: idempotent, just add
            // this source's claim (Part III §8 event/ground idempotency).
            if let Some(rec) = extent.get_mut(&k) {
                rec.claims.insert(claim);
            }
        }
        Some(_) => return Err(on_conflict), // same key, different content
        None => insert_row(extent, row.clone(), claim),
    }
    record_history(working, &def.name, row.clone(), claim, def);
    Ok(())
}

fn insert_row(extent: &mut crate::row::Extent, row: Row, claim: ClaimId) {
    let key = row_key(&row);
    let rec = extent.entry(key).or_insert_with(|| EdgeRecord {
        row,
        ..Default::default()
    });
    rec.claims.insert(claim);
}

/// Append this row to the append-only per-relation history log (Part III §2:
/// ground remains history even after retraction/supersession; §10:
/// transaction time is sealed on every claim). Used by `history R(...)`
/// reads (Part IV §3). History is keyed by row content so the same content
/// asserted twice occupies one history slot with merged claims — the log
/// records *what was ever true*, which is exactly what `history` reads want.
fn record_history(
    working: &mut GroundState,
    rel: &RelName,
    row: Row,
    claim: ClaimId,
    _def: &RelationDef,
) {
    let extent = working.history.entry(rel.clone()).or_default();
    insert_row(extent, row, claim);
}
