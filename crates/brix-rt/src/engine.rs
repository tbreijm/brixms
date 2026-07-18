//! Program IR owned by the native generated-engine runtime.
//!
//! `brix-oracle` remains the independent reference evaluator.  This module
//! is intentionally a separate runtime representation: generated workspaces
//! register a checked program here instead of calling back into the oracle.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, ClaimId, Digest, Domain, EdgeId, NodeId};

/// A runtime value occurring in a relation row or rule environment.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

impl Value {
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

/// A complete generated runtime program. Function pointers are generated
/// native code; they are not dynamically interpreted host callbacks.
#[derive(Clone, Default)]
pub struct Program {
    pub relations: BTreeMap<String, Relation>,
    pub rules: BTreeMap<String, Rule>,
    pub constraints: BTreeMap<String, Constraint>,
    pub fns: BTreeMap<String, TotalFn>,
    pub partial_fns: BTreeMap<String, PartialFn>,
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
    pub masked: BTreeMap<String, BTreeSet<Vec<u8>>>,
    pub violations: Vec<Violation>,
}

/// A sealed constraint failure over one final match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub constraint: String,
    pub match_digest: Digest,
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
    let mut masked: BTreeMap<String, BTreeSet<Vec<u8>>> = BTreeMap::new();
    let phases: BTreeSet<u32> = program.rules.values().map(|rule| rule.phase).collect();
    for phase in phases {
        loop {
            let snapshot = visible_extents(&candidates, &masked);
            let mut changed = false;
            for rule in program.rules.values().filter(|rule| rule.phase == phase) {
                let envs = eval_body(
                    program,
                    &snapshot,
                    &ground.history,
                    &rule.body,
                    vec![Env::new()],
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
                        relation, target, ..
                    } => {
                        let relation_def = &program.relations[relation];
                        let extent = candidates.get(relation).cloned().unwrap_or_default();
                        for env in envs {
                            let Value::Edge(edge) = env[target] else {
                                panic!("mask target is an edge reference")
                            };
                            if let Some((key, _)) = extent
                                .iter()
                                .find(|(_, record)| relation_def.edge_id(&record.row) == edge)
                            {
                                changed |= masked
                                    .entry(relation.clone())
                                    .or_default()
                                    .insert(key.clone());
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
        masked,
        violations,
    }
}

fn visible_extents(
    candidates: &BTreeMap<String, Extent>,
    masked: &BTreeMap<String, BTreeSet<Vec<u8>>>,
) -> BTreeMap<String, Extent> {
    candidates
        .iter()
        .map(|(relation, extent)| {
            let visible = extent
                .iter()
                .filter(|(key, _)| !masked.get(relation).is_some_and(|keys| keys.contains(*key)))
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
        Expr::Call(name, args) => program.fns[name](
            &args
                .iter()
                .map(|arg| eval_expr(program, env, arg))
                .collect::<Vec<_>>(),
        ),
        Expr::Try(name, args) => program.partial_fns[name](
            &args
                .iter()
                .map(|arg| eval_expr(program, env, arg))
                .collect::<Vec<_>>(),
        )
        .expect("unhandled partial function failure"),
    }
}

fn eval_binop(operator: BinOp, left: &Value, right: &Value) -> Value {
    use BinOp::*;
    match operator {
        Add | Sub | Mul => {
            let (left, right) = (
                left.as_i128().expect("numeric lhs"),
                right.as_i128().expect("numeric rhs"),
            );
            Value::Int(
                match operator {
                    Add => left + right,
                    Sub => left - right,
                    Mul => left * right,
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
}
