//! Program IR owned by the native generated-engine runtime.
//!
//! `brix-oracle` remains the independent reference evaluator.  This module
//! is intentionally a separate runtime representation: generated workspaces
//! register a checked program here instead of calling back into the oracle.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, ClaimId, Digest, Domain, EdgeId, NodeId};

/// A runtime value occurring in a relation row or rule environment.
///
/// `PartialEq`/`Eq`/`PartialOrd`/`Ord`/`Hash` are hand-implemented, not
/// derived (see [`Value::key`]): `Enum`'s `name` field must **not**
/// participate in identity. Appendix G is explicit that enums encode by
/// declaration-order ordinal, never the variant's name — two `Value::Enum`s
/// with the same `(ty, ordinal)` are the same value regardless of what
/// display name each was constructed with. A derived comparison would make
/// otherwise-identical enum values silently fail to unify whenever their
/// `name` strings happened to differ, exactly like the bug this crate's
/// sibling `brix_oracle::value::Value` had before issue #24 fixed it —
/// caught here proactively, before any adapter drove real enum literals
/// through this engine.
#[derive(Clone, Debug)]
pub enum Value {
    Nat(u64),
    Int(i64),
    Bool(bool),
    Str(String),
    Node(NodeId),
    Edge(EdgeId),
    Claim(ClaimId),
    Enum {
        ty: String,
        ordinal: u32,
        name: String,
    },
    Unit,
}

/// The identity-relevant projection of a [`Value`] — everything
/// `PartialEq`/`Ord`/`Hash` actually key on. Exists solely to give `Enum`
/// an identity of `(ty, ordinal)`, excluding `name`, without hand-writing
/// five structurally-identical trait impls (`derive` handles this one).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ValueKey<'a> {
    Nat(u64),
    Int(i64),
    Bool(bool),
    Str(&'a str),
    Node(NodeId),
    Edge(EdgeId),
    Claim(ClaimId),
    Enum { ty: &'a str, ordinal: u32 },
    Unit,
}

impl Value {
    fn key(&self) -> ValueKey<'_> {
        match self {
            Value::Nat(n) => ValueKey::Nat(*n),
            Value::Int(n) => ValueKey::Int(*n),
            Value::Bool(b) => ValueKey::Bool(*b),
            Value::Str(s) => ValueKey::Str(s.as_str()),
            Value::Node(id) => ValueKey::Node(*id),
            Value::Edge(id) => ValueKey::Edge(*id),
            Value::Claim(id) => ValueKey::Claim(*id),
            Value::Enum { ty, ordinal, .. } => ValueKey::Enum {
                ty,
                ordinal: *ordinal,
            },
            Value::Unit => ValueKey::Unit,
        }
    }

    pub fn as_i128(&self) -> Option<i128> {
        match self {
            Self::Nat(value) => Some(*value as i128),
            Self::Int(value) => Some(*value as i128),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Value {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key().hash(state)
    }
}

impl Canonical for Value {
    fn canon_write(&self, writer: &mut CanonWriter) {
        match self {
            Self::Nat(value) => {
                writer.write_uint(0);
                writer.write_uint(*value);
            }
            Self::Int(value) => {
                writer.write_uint(1);
                writer.write_int(*value);
            }
            Self::Bool(value) => {
                writer.write_uint(2);
                writer.write_uint(*value as u64);
            }
            Self::Str(value) => {
                writer.write_uint(3);
                writer.write_str(value);
            }
            Self::Node(value) => {
                writer.write_uint(4);
                writer.write_bytes(value.digest().as_bytes());
            }
            Self::Edge(value) => {
                writer.write_uint(5);
                writer.write_bytes(value.digest().as_bytes());
            }
            Self::Claim(value) => {
                writer.write_uint(6);
                writer.write_bytes(value.digest().as_bytes());
            }
            Self::Enum { ty, ordinal, .. } => {
                writer.write_uint(7);
                writer.write_tag(ty);
                writer.write_uint(*ordinal as u64);
            }
            Self::Unit => writer.write_uint(8),
        }
    }
}

/// A row whose role names are held in canonical order.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row(pub BTreeMap<String, Value>);

impl Row {
    pub fn get(&self, role: &str) -> Option<&Value> {
        self.0.get(role)
    }
}

impl Canonical for Row {
    fn canon_write(&self, writer: &mut CanonWriter) {
        writer.write_uint(self.0.len() as u64);
        for (role, value) in &self.0 {
            writer.write_tag(role);
            value.canon_write(writer);
        }
    }
}

/// Per-kind transaction and identity behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelationKind {
    Entity,
    Ground,
    State,
    Event,
    Derived,
}

/// Static declaration metadata needed by transaction validation and rule
/// evaluation.  A generated workspace constructs these directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    pub kind: RelationKind,
    pub roles: Vec<String>,
    pub key: Vec<String>,
    pub open: bool,
}

impl Relation {
    /// Canonical key grouping bytes. These are distinct from an edge/node
    /// identity: state supersession and key-conflict detection need the key
    /// projection even when identity covers an entire row.
    pub fn key_bytes(&self, row: &Row) -> Vec<u8> {
        let mut writer = CanonWriter::new();
        writer.write_tag(&self.name);
        for role in &self.key {
            row.get(role)
                .unwrap_or_else(|| panic!("row missing declared key role `{role}`"))
                .canon_write(&mut writer);
        }
        writer.finish()
    }

    pub fn node_id(&self, row: &Row) -> NodeId {
        NodeId::from_canon(&self.key_bytes(row))
    }

    pub fn edge_id(&self, row: &Row) -> EdgeId {
        let mut writer = CanonWriter::new();
        writer.write_tag(&self.name);
        row.canon_write(&mut writer);
        EdgeId::from_canon(&writer.finish())
    }

    pub fn digest(&self, row: &Row) -> Digest {
        match self.kind {
            RelationKind::Entity => self.node_id(row).digest(),
            _ => self.edge_id(row).digest(),
        }
    }

    pub fn ref_value(&self, row: &Row) -> Value {
        match self.kind {
            RelationKind::Entity => Value::Node(self.node_id(row)),
            _ => Value::Edge(self.edge_id(row)),
        }
    }

    pub fn candidate_digest(&self, row: &Row) -> Digest {
        let mut writer = CanonWriter::new();
        writer.write_tag(&self.name);
        row.canon_write(&mut writer);
        Digest::of(Domain::Value, &writer.finish())
    }
}

/// A variable reference or a literal in a pattern/head role.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Term {
    Var(String),
    Const(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// Truncating integer division (Rust `/` on `i128`; floor for the
    /// non-negative operands every Ring 0 semantic path uses) — issue #47
    /// Part 2's basis-point fixed-point ruling (no float `Value`).
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Var(String),
    Const(Value),
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Try(String, Vec<Expr>),
    /// `if cond { then } else { els }` — needed to execute compiled function
    /// bodies (issue #47); the flagship's `surcharge` is a single `if`.
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    /// `let name = value in body` — a compiled function block's binding
    /// (issue #47 Slice 2).
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clause {
    Edge {
        relation: String,
        bind_id: Option<String>,
        args: Vec<(String, Term)>,
    },
    Without(Vec<Clause>),
    History {
        relation: String,
        args: Vec<(String, Term)>,
    },
    When(Expr),
    Let(String, Expr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    Tuple {
        relation: String,
        args: Vec<(String, Term)>,
    },
    Mask {
        relation: String,
        target: String,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub phase: u32,
    pub head: Head,
    pub body: Vec<Clause>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Advisory,
    Strict,
    Audit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constraint {
    pub id: String,
    pub severity: Severity,
    pub body: Vec<Clause>,
}

pub type TotalFn = fn(&[Value]) -> Value;
pub type PartialFn = fn(&[Value]) -> Result<Value, Value>;

/// A user function compiled from source (issue #47): its parameter names and a
/// body expression the evaluator runs by binding actuals into a fresh `Env`.
/// This is what lets `surcharge` (and other total fns) execute from their
/// BrixMS source instead of a hand-registered native [`TotalFn`] pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnDef {
    pub params: Vec<String>,
    pub body: Expr,
}

/// A complete generated runtime program. Function pointers are generated
/// native code; they are not dynamically interpreted host callbacks.
#[derive(Clone, Default)]
pub struct Program {
    pub relations: BTreeMap<String, Relation>,
    pub rules: BTreeMap<String, Rule>,
    pub constraints: BTreeMap<String, Constraint>,
    pub fns: BTreeMap<String, TotalFn>,
    pub partial_fns: BTreeMap<String, PartialFn>,
    /// Functions compiled from source (issue #47). Resolved *before* the
    /// [`fns`]/`builtin_total` fallback in [`eval_expr`], so a source-defined
    /// fn shadows any native builtin of the same name.
    pub fn_defs: BTreeMap<String, FnDef>,
}

/// One live candidate row and its ground/derived sources.  The native
/// evaluator keeps source counts explicit so retracting one assertion never
/// accidentally removes a row still supported by another assertion or rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub row: Row,
    pub claims: BTreeSet<ClaimId>,
    pub supports: BTreeSet<Support>,
}

impl Record {
    pub fn is_live(&self) -> bool {
        !self.claims.is_empty() || !self.supports.is_empty()
    }
}

/// Rule-match identity backing one derived row.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Support {
    pub rule: String,
    pub match_digest: Digest,
}

pub type Extent = BTreeMap<Vec<u8>, Record>;

/// The committed ground state and append-only ground history of one native
/// namespace. Derived extents are deliberately absent: every revision starts
/// them fresh and settles them to a new least fixpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroundState {
    pub live: BTreeMap<String, Extent>,
    pub history: BTreeMap<String, Extent>,
}

#[derive(Clone, Debug)]
pub enum TransactionOp {
    Ensure { relation: String, row: Row },
    Assert { relation: String, row: Row },
    Set { relation: String, row: Row },
    Event { relation: String, row: Row },
    Retract { relation: String, claim: ClaimId },
}

/// A transaction reads exactly one settled snapshot and either produces one
/// later revision or leaves the prior state intact.
#[derive(Clone, Debug, Default)]
pub struct Transaction {
    pub intent: Vec<u8>,
    pub ops: Vec<TransactionOp>,
}

/// A native program store: transactions are applied atomically to ground
/// state, then the compiler-projected program is settled before publication.
/// This is deliberately separate from the legacy `CanonRow` delta scheduler;
/// generated binaries execute this typed IR directly.
pub struct Store {
    program: Program,
    ground: GroundState,
    next_revision: u64,
    current: Settled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    Transaction(TransactionError),
    StrictViolation { at_revision: u64 },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transaction(error) => write!(f, "transaction failed: {error}"),
            Self::StrictViolation { at_revision } => {
                write!(
                    f,
                    "strict constraint violated at candidate revision {at_revision}"
                )
            }
        }
    }
}

impl std::error::Error for CommitError {}

impl Store {
    pub fn new(program: Program) -> Self {
        let ground = GroundState::default();
        let current = settle(&program, &ground, 0);
        Self {
            program,
            ground,
            next_revision: 1,
            current,
        }
    }

    pub fn current(&self) -> &Settled {
        &self.current
    }

    pub fn current_dump(&self) -> Vec<u8> {
        dump_bytes(&self.current, &self.program)
    }

    pub fn commit(&mut self, transaction: &Transaction) -> Result<&Settled, CommitError> {
        let ground = apply_transaction(&self.program, &self.ground, transaction)
            .map_err(CommitError::Transaction)?;
        let revision = self.next_revision;
        let settled = settle(&self.program, &ground, revision);
        if !settled.strict_ok(&self.program) {
            return Err(CommitError::StrictViolation {
                at_revision: revision,
            });
        }
        self.ground = ground;
        self.current = settled;
        self.next_revision += 1;
        Ok(&self.current)
    }
}

impl Transaction {
    pub fn new(intent: impl Into<Vec<u8>>) -> Self {
        Self {
            intent: intent.into(),
            ops: Vec::new(),
        }
    }

    pub fn claim_id(&self, ordinal: usize) -> ClaimId {
        let mut writer = CanonWriter::new();
        writer.write_bytes(&self.intent);
        writer.write_uint(ordinal as u64);
        ClaimId::from_canon(&writer.finish())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionError {
    UnknownRelation(String),
    WrongRelationKind {
        relation: String,
        operation: &'static str,
    },
    GroundKeyConflict {
        relation: String,
        key: Vec<u8>,
    },
    EventContentMismatch {
        relation: String,
    },
    EntityFieldConflict {
        relation: String,
    },
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRelation(relation) => write!(f, "unknown relation `{relation}`"),
            Self::WrongRelationKind {
                relation,
                operation,
            } => {
                write!(
                    f,
                    "operation `{operation}` is not valid on relation `{relation}`"
                )
            }
            Self::GroundKeyConflict { relation, .. } => {
                write!(f, "ground key conflict on `{relation}`")
            }
            Self::EventContentMismatch { relation } => {
                write!(f, "event `{relation}` reasserted with different content")
            }
            Self::EntityFieldConflict { relation } => {
                write!(f, "entity `{relation}` ensured with conflicting fields")
            }
        }
    }
}

impl std::error::Error for TransactionError {}

/// Apply a transaction against `base` without mutating it. Callers publish
/// the returned state only after the candidate revision settles and satisfies
/// strict constraints.
pub fn apply_transaction(
    program: &Program,
    base: &GroundState,
    transaction: &Transaction,
) -> Result<GroundState, TransactionError> {
    let mut working = base.clone();
    for (ordinal, operation) in transaction.ops.iter().enumerate() {
        match operation {
            TransactionOp::Ensure { relation, row } => {
                let def = relation_def(program, relation)?;
                require_kind(def, RelationKind::Entity, relation, "ensure")?;
                upsert(
                    &mut working,
                    def,
                    row,
                    transaction.claim_id(ordinal),
                    TransactionError::EntityFieldConflict {
                        relation: relation.clone(),
                    },
                )?;
            }
            TransactionOp::Assert { relation, row } => {
                let def = relation_def(program, relation)?;
                require_kind(def, RelationKind::Ground, relation, "assert")?;
                upsert(
                    &mut working,
                    def,
                    row,
                    transaction.claim_id(ordinal),
                    TransactionError::GroundKeyConflict {
                        relation: relation.clone(),
                        key: def.key_bytes(row),
                    },
                )?;
            }
            TransactionOp::Event { relation, row } => {
                let def = relation_def(program, relation)?;
                require_kind(def, RelationKind::Event, relation, "event")?;
                upsert(
                    &mut working,
                    def,
                    row,
                    transaction.claim_id(ordinal),
                    TransactionError::EventContentMismatch {
                        relation: relation.clone(),
                    },
                )?;
            }
            TransactionOp::Set { relation, row } => {
                let def = relation_def(program, relation)?;
                require_kind(def, RelationKind::State, relation, "set")?;
                let key = def.key_bytes(row);
                working
                    .live
                    .entry(relation.clone())
                    .or_default()
                    .retain(|_, record| def.key_bytes(&record.row) != key);
                insert_claim(
                    working.live.entry(relation.clone()).or_default(),
                    row.clone(),
                    transaction.claim_id(ordinal),
                );
                record_history(
                    &mut working,
                    relation,
                    row.clone(),
                    transaction.claim_id(ordinal),
                );
            }
            TransactionOp::Retract { relation, claim } => {
                relation_def(program, relation)?;
                if let Some(extent) = working.live.get_mut(relation) {
                    for record in extent.values_mut() {
                        record.claims.remove(claim);
                    }
                    extent.retain(|_, record| record.is_live());
                }
            }
        }
    }
    Ok(working)
}

fn relation_def<'a>(
    program: &'a Program,
    relation: &str,
) -> Result<&'a Relation, TransactionError> {
    program
        .relations
        .get(relation)
        .ok_or_else(|| TransactionError::UnknownRelation(relation.into()))
}

fn require_kind(
    relation: &Relation,
    expected: RelationKind,
    name: &str,
    operation: &'static str,
) -> Result<(), TransactionError> {
    (relation.kind == expected)
        .then_some(())
        .ok_or_else(|| TransactionError::WrongRelationKind {
            relation: name.into(),
            operation,
        })
}

fn row_key(row: &Row) -> Vec<u8> {
    row.canon_bytes()
}

fn insert_claim(extent: &mut Extent, row: Row, claim: ClaimId) {
    extent
        .entry(row_key(&row))
        .or_insert_with(|| Record {
            row,
            ..Record::default()
        })
        .claims
        .insert(claim);
}

fn upsert(
    state: &mut GroundState,
    relation: &Relation,
    row: &Row,
    claim: ClaimId,
    conflict: TransactionError,
) -> Result<(), TransactionError> {
    let key = relation.key_bytes(row);
    let row_key = row_key(row);
    let extent = state.live.entry(relation.name.clone()).or_default();
    let existing = extent
        .iter()
        .find(|(_, record)| relation.key_bytes(&record.row) == key)
        .map(|(key, _)| key.clone());
    match existing {
        Some(existing) if existing == row_key => {
            extent
                .get_mut(&existing)
                .expect("extent entry disappeared")
                .claims
                .insert(claim);
        }
        Some(_) => return Err(conflict),
        None => insert_claim(extent, row.clone(), claim),
    }
    record_history(state, &relation.name, row.clone(), claim);
    Ok(())
}

fn record_history(state: &mut GroundState, relation: &str, row: Row, claim: ClaimId) {
    insert_claim(
        state.history.entry(relation.into()).or_default(),
        row,
        claim,
    );
}

/// The published, fully settled view of one native revision. Derived extents
/// are rebuilt from ground state for each call, so a caller never observes a
/// partially settled candidate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settled {
    pub at_revision: u64,
    pub extents: BTreeMap<String, Extent>,
    /// The same candidates as `extents`, but *before* mask filtering — a
    /// masked row's own support/claim provenance must still appear in the
    /// dump (Appendix I.1: "...including error edges, KeyConflicts, masks,
    /// and provenance answers") even though the row itself is hidden from
    /// `extents`. Mirrors the oracle's `all_candidates`/`live` split
    /// (`brix-oracle/src/eval.rs`), which keeps exactly this distinction.
    all_extents: BTreeMap<String, Extent>,
    pub masked: Vec<MaskRecord>,
    pub violations: Vec<Violation>,
    /// `RuleError` edges from partial-fn `?` failures in rule bodies (issue #47
    /// Part 3). Empty when no partial failed (the common case).
    pub rule_errors: Vec<RuleError>,
}

/// One masked row: `Masked(target, by, atPhase, atRevision)` (Appendix A,
/// Part III §6) — the same family the oracle's `MaskedEdge` records. `by` is
/// the edge that caused the masking (the rule head's `reason` role); before
/// this fix the native evaluator computed `by` (`eval.rs`'s oracle
/// equivalent has always read it) but discarded it, so masks never reached
/// the dump at all (`dump_bytes` wrote a hardcoded empty list).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MaskRecord {
    /// The masked row's own key bytes within its relation's extent. Used
    /// only to filter live candidates during settlement (`visible_extents`);
    /// not written to the canonical dump, which identifies the row via
    /// `target` instead.
    row_key: Vec<u8>,
    pub target: EdgeId,
    pub by: EdgeId,
    pub relation: String,
    pub rule: String,
    pub at_phase: u32,
}

/// A sealed constraint failure over one final match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub constraint: String,
    pub match_digest: Digest,
}

/// A `RuleError(rule, site, partialMatch, error, atRevision)` edge (Part III §9,
/// issue #47 Part 3): a partial function bound with `?` in a rule body failed at
/// clause `site` (`"{rule}#{clauseIndex}"`) for the partially-built match
/// `partial_match`, carrying the `Err` payload. Mirrors the oracle's
/// `RuleErrorEdge` field-for-field so the settled dumps compare byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuleError {
    pub rule: String,
    pub site: String,
    pub partial_match: Digest,
    pub error: Value,
    pub at_revision: u64,
}

/// Canonical settled dump compatible with the oracle's relation, support,
/// claim, mask, rule-error, and violation layout. Rule errors are now emitted
/// from real partial-fn `?` failures (issue #47 Part 3). Key conflicts are still
/// an empty list: the native engine hard-rejects transactions on entity key
/// conflicts at transaction time rather than settling them into a `KeyConflict`
/// family (a separate follow-up). Retaining every family position makes this an
/// ABI extension, never a second generated-binary format.
pub fn dump_bytes(settled: &Settled, program: &Program) -> Vec<u8> {
    let mut writer = CanonWriter::new();
    writer.write_uint(settled.at_revision);
    writer.write_uint(settled.extents.len() as u64);
    for (relation, extent) in &settled.extents {
        writer.write_tag(relation);
        writer.write_uint(extent.len() as u64);
        for record in extent.values() {
            record.row.canon_write(&mut writer);
            writer.write_uint(record.claims.len() as u64);
            writer.write_uint(record.supports.len() as u64);
        }
    }

    let mut supports = Vec::new();
    let mut claims = Vec::new();
    for (relation, extent) in &settled.all_extents {
        let def = &program.relations[relation];
        for record in extent.values() {
            let edge = def.digest(&record.row);
            supports.extend(record.supports.iter().map(|support| {
                (
                    edge,
                    relation.as_str(),
                    support.rule.as_str(),
                    support.match_digest,
                )
            }));
            claims.extend(
                record
                    .claims
                    .iter()
                    .map(|claim| (edge, relation.as_str(), *claim)),
            );
        }
    }
    supports.sort();
    writer.write_uint(supports.len() as u64);
    for (edge, relation, rule, match_digest) in supports {
        writer.write_bytes(edge.as_bytes());
        writer.write_tag(relation);
        writer.write_tag(rule);
        writer.write_bytes(match_digest.as_bytes());
        writer.write_uint(settled.at_revision);
    }
    claims.sort();
    writer.write_uint(claims.len() as u64);
    for (edge, relation, claim) in claims {
        writer.write_bytes(edge.as_bytes());
        writer.write_tag(relation);
        writer.write_bytes(claim.digest().as_bytes());
        writer.write_uint(settled.at_revision);
    }
    let mut masks = settled.masked.clone();
    masks.sort();
    writer.write_uint(masks.len() as u64);
    for mask in &masks {
        writer.write_bytes(mask.target.digest().as_bytes());
        writer.write_bytes(mask.by.digest().as_bytes());
        writer.write_tag(&mask.relation);
        writer.write_tag(&mask.rule);
        writer.write_uint(mask.at_phase as u64);
        writer.write_uint(settled.at_revision);
    }
    // key conflicts (still empty — brix-rt hard-rejects entity key conflicts at
    // transaction time; the KeyConflict family is a separate follow-up).
    writer.write_uint(0);
    // rule errors (issue #47 Part 3): matches the oracle's `dump.rs` layout —
    // per edge `tag(rule) · tag(site) · bytes(partialMatch) · error(canon) ·
    // uint(atRevision)`, in sorted order.
    let mut errors = settled.rule_errors.clone();
    errors.sort();
    writer.write_uint(errors.len() as u64);
    for e in &errors {
        writer.write_tag(&e.rule);
        writer.write_tag(&e.site);
        writer.write_bytes(e.partial_match.as_bytes());
        e.error.canon_write(&mut writer);
        writer.write_uint(e.at_revision);
    }
    writer.write_uint(settled.violations.len() as u64);
    for violation in &settled.violations {
        writer.write_tag(&violation.constraint);
        writer.write_bytes(violation.match_digest.as_bytes());
        writer.write_uint(settled.at_revision);
    }
    writer.finish()
}

pub fn dump_digest(settled: &Settled, program: &Program) -> Digest {
    Digest::of(Domain::Value, &dump_bytes(settled, program))
}

/// Execute a blank-line-delimited typed transaction stream. Each operation is
/// `assert|ensure|set|event Relation role=value,...`; values use explicit
/// canonical tags (`int:7`, `nat:7`, `bool:true`, `str:hello`, `unit`, or
/// `enum:Type#0`). One canonical dump is returned for every published
/// revision as `revision digest hex-bytes`.
pub fn run_text(program: Program, input: &str) -> Result<String, StreamError> {
    let mut store = Store::new(program);
    let mut transaction = Transaction::new(stream_intent(0));
    let mut ordinal = 0usize;
    let mut has_operations = false;
    let mut output = String::new();
    for (line_number, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            if has_operations {
                commit_text(&mut output, &mut store, &transaction)?;
                ordinal += 1;
                transaction = Transaction::new(stream_intent(ordinal));
                has_operations = false;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        transaction
            .ops
            .push(parse_operation(line, line_number + 1)?);
        has_operations = true;
    }
    if has_operations {
        commit_text(&mut output, &mut store, &transaction)?;
    }
    Ok(output)
}

fn commit_text(
    output: &mut String,
    store: &mut Store,
    transaction: &Transaction,
) -> Result<(), StreamError> {
    store
        .commit(transaction)
        .map_err(|error| StreamError::Commit(error.to_string()))?;
    let settled = store.current();
    let bytes = store.current_dump();
    output.push_str(&format!(
        "{} {} {}\n",
        settled.at_revision,
        Digest::of(Domain::Value, &bytes).to_hex(),
        encode_hex(&bytes)
    ));
    Ok(())
}

fn parse_operation(line: &str, line_number: usize) -> Result<TransactionOp, StreamError> {
    let mut fields = line.split_ascii_whitespace();
    let operation = fields.next().ok_or(StreamError::InvalidLine {
        line: line_number,
        message: "missing operation",
    })?;
    let relation = fields.next().ok_or(StreamError::InvalidLine {
        line: line_number,
        message: "missing relation",
    })?;
    let row_fields = fields.next().ok_or(StreamError::InvalidLine {
        line: line_number,
        message: "missing row",
    })?;
    if fields.next().is_some() {
        return Err(StreamError::InvalidLine {
            line: line_number,
            message: "expected an operation, relation, and row",
        });
    }
    let row = parse_row(row_fields, line_number)?;
    let relation = relation.to_owned();
    match operation {
        "assert" => Ok(TransactionOp::Assert { relation, row }),
        "ensure" => Ok(TransactionOp::Ensure { relation, row }),
        "set" => Ok(TransactionOp::Set { relation, row }),
        "event" => Ok(TransactionOp::Event { relation, row }),
        _ => Err(StreamError::InvalidLine {
            line: line_number,
            message: "operation must be assert, ensure, set, or event",
        }),
    }
}

fn parse_row(fields: &str, line_number: usize) -> Result<Row, StreamError> {
    let mut row = BTreeMap::new();
    for field in fields.split(',') {
        let (role, value) = field.split_once('=').ok_or(StreamError::InvalidLine {
            line: line_number,
            message: "row fields must be role=value",
        })?;
        if role.is_empty() || row.contains_key(role) {
            return Err(StreamError::InvalidLine {
                line: line_number,
                message: "row roles must be non-empty and unique",
            });
        }
        row.insert(role.to_owned(), parse_value(value, line_number)?);
    }
    Ok(Row(row))
}

fn parse_value(input: &str, line_number: usize) -> Result<Value, StreamError> {
    let invalid = || StreamError::InvalidLine {
        line: line_number,
        message: "invalid typed value",
    };
    if input == "unit" {
        return Ok(Value::Unit);
    }
    if let Some(value) = input.strip_prefix("int:") {
        return value.parse().map(Value::Int).map_err(|_| invalid());
    }
    if let Some(value) = input.strip_prefix("nat:") {
        return value.parse().map(Value::Nat).map_err(|_| invalid());
    }
    if let Some(value) = input.strip_prefix("bool:") {
        return value.parse().map(Value::Bool).map_err(|_| invalid());
    }
    if let Some(value) = input.strip_prefix("str:") {
        return Ok(Value::Str(value.into()));
    }
    if let Some(value) = input.strip_prefix("enum:") {
        let (ty, ordinal) = value.split_once('#').ok_or_else(invalid)?;
        let ordinal = ordinal.parse().map_err(|_| invalid())?;
        return Ok(Value::Enum {
            ty: ty.into(),
            ordinal,
            name: ordinal.to_string(),
        });
    }
    if let Some(value) = input.strip_prefix("node:") {
        return decode_digest(value, line_number).map(|digest| Value::Node(NodeId(digest)));
    }
    if let Some(value) = input.strip_prefix("edge:") {
        return decode_digest(value, line_number).map(|digest| Value::Edge(EdgeId(digest)));
    }
    Err(invalid())
}

fn decode_digest(input: &str, line_number: usize) -> Result<Digest, StreamError> {
    if input.len() != 64 {
        return Err(StreamError::InvalidLine {
            line: line_number,
            message: "identity values must contain 64 hexadecimal characters",
        });
    }
    let mut bytes = [0u8; 32];
    for (slot, pair) in bytes.iter_mut().zip(input.as_bytes().chunks_exact(2)) {
        let Some(high) = hex_nibble(pair[0]) else {
            return Err(StreamError::InvalidLine {
                line: line_number,
                message: "identity values must be hexadecimal",
            });
        };
        let Some(low) = hex_nibble(pair[1]) else {
            return Err(StreamError::InvalidLine {
                line: line_number,
                message: "identity values must be hexadecimal",
            });
        };
        *slot = (high << 4) | low;
    }
    Ok(Digest::from_bytes(bytes))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamError {
    InvalidLine { line: usize, message: &'static str },
    Commit(String),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLine { line, message } => write!(f, "stream line {line}: {message}"),
            Self::Commit(message) => write!(f, "transaction rejected: {message}"),
        }
    }
}

impl std::error::Error for StreamError {}

fn stream_intent(ordinal: usize) -> Vec<u8> {
    format!("brix-stdin-{ordinal}").into_bytes()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

impl Settled {
    pub fn strict_ok(&self, program: &Program) -> bool {
        self.violations.iter().all(|violation| {
            program.constraints[&violation.constraint].severity != Severity::Strict
        })
    }
}

type Env = BTreeMap<String, Value>;

/// Naively settle every positive rule in each compiler-assigned phase. The
/// delta scheduler can later optimize this exact fixed point without changing
/// the published model.
pub fn settle(program: &Program, ground: &GroundState, at_revision: u64) -> Settled {
    let mut candidates: BTreeMap<String, Extent> = program
        .relations
        .iter()
        .map(|(name, relation)| {
            (
                name.clone(),
                if relation.kind == RelationKind::Derived {
                    Extent::new()
                } else {
                    ground.live.get(name).cloned().unwrap_or_default()
                },
            )
        })
        .collect();
    let mut masked: Vec<MaskRecord> = Vec::new();
    // RuleError provenance (issue #47 Part 3): accumulated across every phase and
    // fixpoint iteration, deduped on `(rule, site, partialMatch)` so a site that
    // fails on every naive re-derivation yields exactly one edge.
    let mut rule_errors: Vec<RuleError> = Vec::new();
    let mut rule_error_seen: BTreeSet<(String, String, Digest)> = BTreeSet::new();
    let phases: BTreeSet<u32> = program.rules.values().map(|rule| rule.phase).collect();
    for phase in phases {
        loop {
            let snapshot = visible_extents(&candidates, &masked);
            let mut changed = false;
            for rule in program.rules.values().filter(|rule| rule.phase == phase) {
                let envs = eval_rule_body(
                    program,
                    &snapshot,
                    &ground.history,
                    &rule.body,
                    &rule.id,
                    at_revision,
                    &mut rule_errors,
                    &mut rule_error_seen,
                );
                match &rule.head {
                    Head::Tuple { relation, args } => {
                        let target = candidates.entry(relation.clone()).or_default();
                        for env in envs {
                            let row = Row(args
                                .iter()
                                .map(|(role, term)| {
                                    let value = match term {
                                        Term::Var(variable) => env[variable].clone(),
                                        Term::Const(value) => value.clone(),
                                    };
                                    (role.clone(), value)
                                })
                                .collect());
                            let support = Support {
                                rule: rule.id.clone(),
                                match_digest: env_digest(&env),
                            };
                            let record = target.entry(row_key(&row)).or_insert_with(|| Record {
                                row,
                                ..Record::default()
                            });
                            changed |= record.supports.insert(support);
                        }
                    }
                    Head::Mask {
                        relation,
                        target,
                        reason,
                    } => {
                        let relation_def = &program.relations[relation];
                        let extent = candidates.get(relation).cloned().unwrap_or_default();
                        for env in envs {
                            let Value::Edge(target_edge) = env[target] else {
                                panic!("mask target is an edge reference")
                            };
                            let Value::Edge(by_edge) = env[reason] else {
                                panic!("mask reason is an edge reference")
                            };
                            if let Some((key, _)) = extent.iter().find(|(_, record)| {
                                relation_def.edge_id(&record.row) == target_edge
                            }) {
                                let record = MaskRecord {
                                    row_key: key.clone(),
                                    target: target_edge,
                                    by: by_edge,
                                    relation: relation.clone(),
                                    rule: rule.id.clone(),
                                    at_phase: phase,
                                };
                                if !masked.contains(&record) {
                                    masked.push(record);
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    let visible = visible_extents(&candidates, &masked);
    let mut violations = Vec::new();
    for constraint in program.constraints.values() {
        for env in eval_body(
            program,
            &visible,
            &ground.history,
            &constraint.body,
            vec![Env::new()],
        ) {
            violations.push(Violation {
                constraint: constraint.id.clone(),
                match_digest: env_digest(&env),
            });
        }
    }
    violations.sort();
    Settled {
        at_revision,
        extents: visible,
        all_extents: candidates,
        masked,
        violations,
        rule_errors,
    }
}

fn visible_extents(
    candidates: &BTreeMap<String, Extent>,
    masked: &[MaskRecord],
) -> BTreeMap<String, Extent> {
    let masked_keys: BTreeMap<&str, BTreeSet<&[u8]>> =
        masked.iter().fold(BTreeMap::new(), |mut acc, record| {
            acc.entry(record.relation.as_str())
                .or_default()
                .insert(&record.row_key);
            acc
        });
    candidates
        .iter()
        .map(|(relation, extent)| {
            let visible = extent
                .iter()
                .filter(|(key, _)| {
                    !masked_keys
                        .get(relation.as_str())
                        .is_some_and(|keys| keys.contains(key.as_slice()))
                })
                .map(|(key, record)| (key.clone(), record.clone()))
                .collect();
            (relation.clone(), visible)
        })
        .collect()
}

fn eval_body(
    program: &Program,
    live: &BTreeMap<String, Extent>,
    history: &BTreeMap<String, Extent>,
    clauses: &[Clause],
    mut envs: Vec<Env>,
) -> Vec<Env> {
    for clause in clauses {
        envs = match clause {
            Clause::Edge {
                relation,
                bind_id,
                args,
            } => {
                let relation_def = &program.relations[relation];
                let extent = &live[relation];
                envs.iter()
                    .flat_map(|env| {
                        extent.values().filter_map(|record| {
                            let mut next = unify(env, args, &record.row)?;
                            if let Some(bind) = bind_id {
                                let value = relation_def.ref_value(&record.row);
                                if next.get(bind).is_some_and(|existing| existing != &value) {
                                    return None;
                                }
                                next.insert(bind.clone(), value);
                            }
                            Some(next)
                        })
                    })
                    .collect()
            }
            Clause::History { relation, args } => envs
                .iter()
                .flat_map(|env| {
                    history
                        .get(relation)
                        .into_iter()
                        .flat_map(|extent| extent.values())
                        .filter_map(|record| unify(env, args, &record.row))
                })
                .collect(),
            Clause::Without(inner) => envs
                .into_iter()
                .filter(|env| {
                    eval_body(program, live, history, inner, vec![env.clone()]).is_empty()
                })
                .collect(),
            Clause::When(expr) => envs
                .into_iter()
                .filter(|env| eval_expr(program, env, expr).as_bool() == Some(true))
                .collect(),
            Clause::Let(binding, expr) => envs
                .into_iter()
                .map(|mut env| {
                    let value = eval_expr(program, &env, expr);
                    env.insert(binding.clone(), value);
                    env
                })
                .collect(),
        };
        if envs.is_empty() {
            break;
        }
    }
    envs
}

/// Evaluate a rule body, recording a [`RuleError`] for each partial-fn `?`
/// failure (issue #47 Part 3). A `Clause::Let(v, Try(..))` runs the partial
/// through [`call_partial`]: `Ok` binds `v`; `Err` drops the match and derives a
/// `RuleError` at this clause site, deduped on `(rule, site, partialMatch)` so
/// the naive fixpoint's re-derivation yields exactly one edge per failing site.
/// Every other clause delegates to [`eval_body`] unchanged.
#[allow(clippy::too_many_arguments)]
fn eval_rule_body(
    program: &Program,
    live: &BTreeMap<String, Extent>,
    history: &BTreeMap<String, Extent>,
    clauses: &[Clause],
    rule_id: &str,
    at_revision: u64,
    rule_errors: &mut Vec<RuleError>,
    seen: &mut BTreeSet<(String, String, Digest)>,
) -> Vec<Env> {
    let mut envs = vec![Env::new()];
    for (site_idx, clause) in clauses.iter().enumerate() {
        if envs.is_empty() {
            break;
        }
        envs = match clause {
            Clause::Let(binding, Expr::Try(name, args)) => {
                let mut out = Vec::new();
                for env in envs {
                    let vals: Vec<Value> =
                        args.iter().map(|a| eval_expr(program, &env, a)).collect();
                    match call_partial(program, name, &vals) {
                        Ok(value) => {
                            let mut next = env;
                            next.insert(binding.clone(), value);
                            out.push(next);
                        }
                        Err(error) => {
                            let site = format!("{rule_id}#{site_idx}");
                            let partial_match = env_digest(&env);
                            if seen.insert((rule_id.to_string(), site.clone(), partial_match)) {
                                rule_errors.push(RuleError {
                                    rule: rule_id.to_string(),
                                    site,
                                    partial_match,
                                    error,
                                    at_revision,
                                });
                            }
                        }
                    }
                }
                out
            }
            other => eval_body(program, live, history, std::slice::from_ref(other), envs),
        };
    }
    envs
}

fn unify(env: &Env, args: &[(String, Term)], row: &Row) -> Option<Env> {
    let mut next = env.clone();
    for (role, term) in args {
        let value = row.get(role)?;
        match term {
            Term::Var(variable) => match next.get(variable) {
                Some(bound) if bound != value => return None,
                Some(_) => {}
                None => {
                    next.insert(variable.clone(), value.clone());
                }
            },
            Term::Const(expected) if expected != value => return None,
            Term::Const(_) => {}
        }
    }
    Some(next)
}

fn env_digest(env: &Env) -> Digest {
    let mut writer = CanonWriter::new();
    writer.write_uint(env.len() as u64);
    for (name, value) in env {
        writer.write_tag(name);
        value.canon_write(&mut writer);
    }
    Digest::of(Domain::Value, &writer.finish())
}

fn eval_expr(program: &Program, env: &Env, expr: &Expr) -> Value {
    match expr {
        Expr::Var(variable) => env[variable].clone(),
        Expr::Const(value) => value.clone(),
        Expr::BinOp(operator, left, right) => eval_binop(
            *operator,
            &eval_expr(program, env, left),
            &eval_expr(program, env, right),
        ),
        Expr::Call(name, args) => {
            let args = args
                .iter()
                .map(|arg| eval_expr(program, env, arg))
                .collect::<Vec<_>>();
            // A function compiled from source (issue #47) resolves first: bind
            // actuals to its parameter names in a fresh env and run its body.
            if let Some(def) = program.fn_defs.get(name) {
                let call_env: Env = def
                    .params
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();
                return eval_expr(program, &call_env, &def.body);
            }
            match program
                .fns
                .get(name)
                .copied()
                .or_else(|| builtin_total(name))
            {
                Some(function) => function(&args),
                None => panic!("unregistered total function `{name}`"),
            }
        }
        Expr::If { cond, then, els } => match eval_expr(program, env, cond) {
            Value::Bool(true) => eval_expr(program, env, then),
            Value::Bool(false) => eval_expr(program, env, els),
            other => panic!("`if` condition must be Bool, got {other:?}"),
        },
        Expr::Let { name, value, body } => {
            let mut inner = env.clone();
            inner.insert(name.clone(), eval_expr(program, env, value));
            eval_expr(program, &inner, body)
        }
        Expr::Try(name, args) => {
            let args = args
                .iter()
                .map(|arg| eval_expr(program, env, arg))
                .collect::<Vec<_>>();
            // A `?` on a partial call in expression position (nested, not the
            // rule-clause form handled in `eval_body`). Resolve the compiled
            // body first (issue #47 Part 3); a failure here has no clause to
            // attribute a `RuleError` to, so it propagates as a panic until
            // nested-`?` provenance lands.
            match call_partial(program, name, &args) {
                Ok(value) => value,
                Err(_) => panic!("unhandled partial function failure in `{name}`"),
            }
        }
    }
}

/// Evaluate a **partial** function body to a `Result<Value, Value>` (issue #47
/// Part 3). Total sub-expressions evaluate normally and wrap `Ok`; the fallible
/// leaves are validated constructors (`Type.try(x)`) and nested `?` calls. `if`
/// and `let` recurse so a `.try` inside a branch stays in fallible position.
fn eval_fallible(program: &Program, env: &Env, expr: &Expr) -> Result<Value, Value> {
    match expr {
        Expr::If { cond, then, els } => match eval_expr(program, env, cond) {
            Value::Bool(true) => eval_fallible(program, env, then),
            Value::Bool(false) => eval_fallible(program, env, els),
            other => panic!("`if` condition must be Bool, got {other:?}"),
        },
        Expr::Let { name, value, body } => {
            let mut inner = env.clone();
            inner.insert(name.clone(), eval_expr(program, env, value));
            eval_fallible(program, &inner, body)
        }
        Expr::Call(name, args) if is_validated_ctor(name) => {
            let args = args
                .iter()
                .map(|arg| eval_expr(program, env, arg))
                .collect::<Vec<_>>();
            validated_ctor(name, &args)
        }
        Expr::Try(name, args) => {
            let args = args
                .iter()
                .map(|arg| eval_expr(program, env, arg))
                .collect::<Vec<_>>();
            // A nested `?`: unwrap-or-propagate. As a tail this equals the
            // callee's own `Result`.
            call_partial(program, name, &args)
        }
        // Any other body form is total: evaluate and wrap `Ok`.
        other => Ok(eval_expr(program, env, other)),
    }
}

/// Call a partial function by name, returning its `Result`. A function compiled
/// from source (issue #47) resolves first — its body runs through
/// [`eval_fallible`]; otherwise a hand-registered/builtin `PartialFn` answers.
fn call_partial(program: &Program, name: &str, args: &[Value]) -> Result<Value, Value> {
    if let Some(def) = program.fn_defs.get(name) {
        let call_env: Env = def
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        return eval_fallible(program, &call_env, &def.body);
    }
    match program.partial_fns.get(name).copied() {
        Some(function) => function(args),
        None => panic!("unregistered partial function `{name}`"),
    }
}

/// Whether `name` is a builtin validated fallible constructor `Type.try(x)`
/// (issue #47 Part 3). Scoped to the refinement types the flagship exercises.
fn is_validated_ctor(name: &str) -> bool {
    name == "Probability.try"
}

/// Admit a value into a refinement type, or fail with a `ValidationError`. All
/// arithmetic is integer basis points (Part 2 ruling): a `Probability` is
/// `0..=10_000`.
fn validated_ctor(name: &str, args: &[Value]) -> Result<Value, Value> {
    match name {
        "Probability.try" => {
            let bp = args
                .first()
                .and_then(Value::as_i128)
                .expect("Probability.try expects a numeric (basis-point) argument");
            if (0..=10_000).contains(&bp) {
                Ok(Value::Int(bp as i64))
            } else {
                Err(validation_error())
            }
        }
        _ => panic!("unknown validated constructor `{name}`"),
    }
}

/// The canonical `ValidationError` failure value (issue #47 Part 3). Both
/// engines construct it identically so a `RuleError`'s `error` payload compares
/// byte-for-byte.
fn validation_error() -> Value {
    Value::Enum {
        ty: "ValidationError".to_string(),
        ordinal: 0,
        name: "OutOfRange".to_string(),
    }
}

/// Native total-function fallback. `surcharge` used to live here as a
/// hand-transcription of its BrixMS source; it is now compiled from source
/// (issue #47 Slice 1.5) and resolves via `Program::fn_defs`. What remains
/// is `brix.math.clamp(x, lo, hi)` — a prelude-seeded signature
/// (`brixc::lower::resolve::seed_prelude`) with no source body to compile,
/// so it stays a builtin: integer clamp on basis-point `Value::Int`s, per
/// the issue #47 Part 2 fixed-point ruling (no float arithmetic).
fn builtin_total(name: &str) -> Option<TotalFn> {
    (name == "brix.math.clamp").then_some(|args: &[Value]| {
        let x = args[0].as_i128().expect("clamp: non-numeric value");
        let lo = args[1].as_i128().expect("clamp: non-numeric lo");
        let hi = args[2].as_i128().expect("clamp: non-numeric hi");
        Value::Int(x.max(lo).min(hi).try_into().unwrap_or(i64::MAX))
    })
}

fn eval_binop(operator: BinOp, left: &Value, right: &Value) -> Value {
    use BinOp::*;
    match operator {
        Add | Sub | Mul | Div => {
            let (left, right) = (
                left.as_i128().expect("numeric lhs"),
                right.as_i128().expect("numeric rhs"),
            );
            Value::Int(
                match operator {
                    Add => left + right,
                    Sub => left - right,
                    Mul => left * right,
                    // Truncating toward zero (issue #47 Part 2 ruling) —
                    // matches the oracle's `eval_binop`; no float `Value`.
                    Div => left / right,
                    _ => unreachable!(),
                }
                .try_into()
                .unwrap_or(i64::MAX),
            )
        }
        Eq => Value::Bool(left == right),
        Ne => Value::Bool(left != right),
        Lt | Le | Gt | Ge => {
            let (left, right) = (
                left.as_i128().expect("numeric lhs"),
                right.as_i128().expect("numeric rhs"),
            );
            Value::Bool(match operator {
                Lt => left < right,
                Le => left <= right,
                Gt => left > right,
                Ge => left >= right,
                _ => unreachable!(),
            })
        }
        And => Value::Bool(left.as_bool().unwrap_or(false) && right.as_bool().unwrap_or(false)),
        Or => Value::Bool(left.as_bool().unwrap_or(false) || right.as_bool().unwrap_or(false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity() -> Relation {
        Relation {
            name: "Client".into(),
            kind: RelationKind::Entity,
            roles: vec!["code".into(), "tier".into()],
            key: vec!["code".into()],
            open: false,
        }
    }

    #[test]
    fn entity_identity_is_keyed_but_candidate_identity_is_content_sensitive() {
        let relation = entity();
        let a = Row(BTreeMap::from([
            ("code".into(), Value::Str("acme".into())),
            ("tier".into(), Value::Int(1)),
        ]));
        let b = Row(BTreeMap::from([
            ("code".into(), Value::Str("acme".into())),
            ("tier".into(), Value::Int(2)),
        ]));
        assert_eq!(relation.node_id(&a), relation.node_id(&b));
        assert_ne!(relation.candidate_digest(&a), relation.candidate_digest(&b));
    }

    #[test]
    fn enum_display_name_does_not_change_canonical_bytes() {
        let a = Value::Enum {
            ty: "Status".into(),
            ordinal: 1,
            name: "Open".into(),
        };
        let b = Value::Enum {
            ty: "Status".into(),
            ordinal: 1,
            name: "renamed for display".into(),
        };
        assert_eq!(a.canon_bytes(), b.canon_bytes());
    }

    #[test]
    fn enum_display_name_does_not_change_value_identity() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let a = Value::Enum {
            ty: "Status".into(),
            ordinal: 1,
            name: "Open".into(),
        };
        let b = Value::Enum {
            ty: "Status".into(),
            ordinal: 1,
            name: "renamed for display".into(),
        };
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
        let hash = |v: &Value| {
            let mut hasher = DefaultHasher::new();
            v.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn transaction_is_atomic_on_an_entity_field_conflict() {
        let mut program = Program::default();
        program.relations.insert("Client".into(), entity());
        let first = Transaction {
            intent: b"first".to_vec(),
            ops: vec![TransactionOp::Ensure {
                relation: "Client".into(),
                row: Row(BTreeMap::from([
                    ("code".into(), Value::Str("acme".into())),
                    ("tier".into(), Value::Int(1)),
                ])),
            }],
        };
        let base = apply_transaction(&program, &GroundState::default(), &first).unwrap();
        let conflicting = Transaction {
            intent: b"conflict".to_vec(),
            ops: vec![TransactionOp::Ensure {
                relation: "Client".into(),
                row: Row(BTreeMap::from([
                    ("code".into(), Value::Str("acme".into())),
                    ("tier".into(), Value::Int(2)),
                ])),
            }],
        };
        assert!(matches!(
            apply_transaction(&program, &base, &conflicting),
            Err(TransactionError::EntityFieldConflict { .. })
        ));
        assert_eq!(base.live["Client"].len(), 1);
    }

    #[test]
    fn positive_rule_settles_to_a_fixed_point() {
        let relation = |name: &str, kind| Relation {
            name: name.into(),
            kind,
            roles: vec!["value".into()],
            key: vec!["value".into()],
            open: false,
        };
        let mut program = Program::default();
        program
            .relations
            .insert("Input".into(), relation("Input", RelationKind::Ground));
        program
            .relations
            .insert("Output".into(), relation("Output", RelationKind::Derived));
        program.rules.insert(
            "Copy".into(),
            Rule {
                id: "Copy".into(),
                phase: 0,
                head: Head::Tuple {
                    relation: "Output".into(),
                    args: vec![("value".into(), Term::Var("value".into()))],
                },
                body: vec![Clause::Edge {
                    relation: "Input".into(),
                    bind_id: None,
                    args: vec![("value".into(), Term::Var("value".into()))],
                }],
            },
        );
        let txn = Transaction {
            intent: b"input".to_vec(),
            ops: vec![TransactionOp::Assert {
                relation: "Input".into(),
                row: Row(BTreeMap::from([("value".into(), Value::Int(7))])),
            }],
        };
        let ground = apply_transaction(&program, &GroundState::default(), &txn).unwrap();
        let settled = settle(&program, &ground, 1);
        assert_eq!(settled.extents["Output"].len(), 1);
    }

    #[test]
    fn masked_row_is_hidden_and_recorded_with_its_cause() {
        // `Source` is *derived* (not ground) so its masked row still carries
        // a `Support`, not just a `Claim` — this is what the flagship's own
        // `Override` (masking `ComputedPrice`, itself derived by
        // `PriceOrder`) exercises, and what caught the real gap this test
        // guards: `dump_bytes` used to collect supports/claims by walking
        // only the post-mask `extents`, silently dropping a masked row's
        // provenance from the dump entirely (Appendix I.1 requires masked
        // rows' provenance to still appear).
        let ground_relation = |name: &str| Relation {
            name: name.into(),
            kind: RelationKind::Ground,
            roles: vec!["value".into()],
            key: vec!["value".into()],
            open: false,
        };
        let mut program = Program::default();
        program
            .relations
            .insert("Raw".into(), ground_relation("Raw"));
        program
            .relations
            .insert("Cause".into(), ground_relation("Cause"));
        program.relations.insert(
            "Source".into(),
            Relation {
                name: "Source".into(),
                kind: RelationKind::Derived,
                roles: vec!["value".into()],
                key: vec!["value".into()],
                open: false,
            },
        );
        program.rules.insert(
            "Derive".into(),
            Rule {
                id: "Derive".into(),
                phase: 0,
                head: Head::Tuple {
                    relation: "Source".into(),
                    args: vec![("value".into(), Term::Var("value".into()))],
                },
                body: vec![Clause::Edge {
                    relation: "Raw".into(),
                    bind_id: None,
                    args: vec![("value".into(), Term::Var("value".into()))],
                }],
            },
        );
        program.rules.insert(
            "MaskIt".into(),
            Rule {
                id: "MaskIt".into(),
                phase: 0,
                head: Head::Mask {
                    relation: "Source".into(),
                    target: "s".into(),
                    reason: "c".into(),
                },
                body: vec![
                    Clause::Edge {
                        relation: "Source".into(),
                        bind_id: Some("s".into()),
                        args: vec![("value".into(), Term::Var("v".into()))],
                    },
                    Clause::Edge {
                        relation: "Cause".into(),
                        bind_id: Some("c".into()),
                        args: vec![],
                    },
                ],
            },
        );
        let txn = Transaction {
            intent: b"mask".to_vec(),
            ops: vec![
                TransactionOp::Assert {
                    relation: "Raw".into(),
                    row: Row(BTreeMap::from([("value".into(), Value::Int(1))])),
                },
                TransactionOp::Assert {
                    relation: "Cause".into(),
                    row: Row(BTreeMap::from([("value".into(), Value::Int(99))])),
                },
            ],
        };
        let ground = apply_transaction(&program, &GroundState::default(), &txn).unwrap();
        let settled = settle(&program, &ground, 1);

        assert!(settled.extents["Source"].is_empty());
        assert_eq!(settled.masked.len(), 1);
        let mask = &settled.masked[0];
        assert_eq!(mask.relation, "Source");
        assert_eq!(mask.rule, "MaskIt");
        assert_eq!(mask.at_phase, 0);
        let source_row = Row(BTreeMap::from([("value".into(), Value::Int(1))]));
        assert_eq!(
            mask.target,
            program.relations["Source"].edge_id(&source_row)
        );
        assert_eq!(
            mask.by,
            program.relations["Cause"]
                .edge_id(&Row(BTreeMap::from([("value".into(), Value::Int(99))])))
        );

        // The masked row is gone from the live extent...
        assert!(settled.extents["Source"].is_empty());
        // ...but its derivation support must still be retained for the dump
        // (this is the fix: `all_extents` is pre-mask, `extents` post-mask).
        let all_source = &settled.all_extents["Source"];
        assert_eq!(all_source.len(), 1);
        let record = all_source.values().next().unwrap();
        assert_eq!(record.supports.len(), 1);
        assert_eq!(record.supports.iter().next().unwrap().rule, "Derive");

        // The dump's mask family must actually carry the record, not the
        // hardcoded-empty placeholder this replaced.
        let bytes = dump_bytes(&settled, &program);
        let empty_masks = {
            let mut empty = settled.clone();
            empty.masked.clear();
            dump_bytes(&empty, &program)
        };
        assert_ne!(bytes, empty_masks);

        // And the masked row's support must appear in the dump too — not
        // just its mask record (the bug this test was rewritten to catch).
        let bytes_without_all_extents_support = {
            let mut without = settled.clone();
            without.all_extents.get_mut("Source").unwrap().clear();
            dump_bytes(&without, &program)
        };
        assert_ne!(bytes, bytes_without_all_extents_support);
    }
}
