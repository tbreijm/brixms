//! The evaluator: `Settled(P, r) = least fixpoint of the rules of program
//! revision P, evaluated phase by phase over Base(r)` (Part III §4).
//!
//! Boring by design (OWNER.md): every revision recomputes the full fixpoint
//! from scratch — `all_candidates` only ever grows within one `settle()`
//! call (a fresh call starts empty for every `Derived`-kind relation), the
//! live view is *refiltered from that accumulator*, not incrementally
//! patched, and there is no cross-call cache. Semi-naive delta evaluation
//! (Part III §5's own description of positive-recursion settlement) is an
//! optimization over the identical least fixpoint; naive re-evaluation to a
//! fixpoint is used here because it is simpler and the result is provably
//! the same set.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain, EdgeId};

use crate::phase::Phase;
use crate::program::{
    BinOp, Clause, Expr, Head, Program, RelName, RelationDef, RuleId, Severity, Var,
};
use crate::provenance::{
    ClaimEdge, KeyConflictEdge, MaskedEdge, Provenance, RuleErrorEdge, SupportEdge, SupportRef,
    ViolationEdge,
};
use crate::row::{row_key, CanonBytes, EdgeRecord, Extent, Row};
use crate::value::Value;

pub type Env = BTreeMap<Var, Value>;

fn env_digest(env: &Env) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(env.len() as u64);
    for (k, v) in env {
        w.write_tag(k);
        v.canon_write(&mut w);
    }
    Digest::of(Domain::Value, &w.finish())
}

/// The fully settled result of one revision (Part III §4). `extents` is the
/// live view — masks and derived key-conflicts already applied — for every
/// relation in the program.
#[derive(Clone, Debug)]
pub struct Settled {
    pub at_revision: u64,
    pub extents: BTreeMap<RelName, Extent>,
    pub provenance: Provenance,
}

impl Settled {
    pub fn extent(&self, relation: &str) -> Option<&Extent> {
        self.extents.get(relation)
    }

    /// True iff no `strict` constraint has a live `Violation` (Part IV §7:
    /// "`strict` rejects the offending transaction or program activation").
    pub fn strict_ok(&self, program: &Program) -> bool {
        self.provenance.violations.iter().all(|v| {
            program
                .constraints
                .get(&v.constraint)
                .map(|c| c.severity != Severity::Strict)
                .unwrap_or(true)
        })
    }
}

/// Everything a match needs to read: the current live (filtered) view, the
/// full ground history (for `history` clauses — Ground/State/Event kinds
/// only, see `program::Clause::History` docs), and the program's static
/// tables.
struct Ctx<'p> {
    program: &'p Program,
    live: &'p BTreeMap<RelName, Extent>,
    history: &'p BTreeMap<RelName, Extent>,
}

/// Unify `args` against `row`, extending `env`. Returns `None` on conflict
/// (a `Var` already bound to a different `Value`, or a `Const` mismatch).
fn unify(env: &Env, args: &[(String, crate::program::Term)], row: &Row) -> Option<Env> {
    let mut out = env.clone();
    for (role, term) in args {
        let row_val = row.get(role)?;
        match term {
            crate::program::Term::Var(v) => match out.get(v) {
                Some(existing) if existing != row_val => return None,
                Some(_) => {}
                None => {
                    out.insert(v.clone(), row_val.clone());
                }
            },
            crate::program::Term::Const(c) => {
                if c != row_val {
                    return None;
                }
            }
        }
    }
    Some(out)
}

fn eval_expr(env: &Env, expr: &Expr, ctx: &Ctx) -> Value {
    match expr {
        Expr::Var(v) => env
            .get(v)
            .unwrap_or_else(|| panic!("unbound variable `{v}` in expression"))
            .clone(),
        Expr::Const(c) => c.clone(),
        Expr::BinOp(op, a, b) => eval_binop(*op, &eval_expr(env, a, ctx), &eval_expr(env, b, ctx)),
        Expr::Call(name, args) => {
            let f = *ctx
                .program
                .fns
                .get(name)
                .unwrap_or_else(|| panic!("unregistered fn `{name}`"));
            let vals: Vec<Value> = args.iter().map(|a| eval_expr(env, a, ctx)).collect();
            f(&vals)
        }
        Expr::Try(name, args) => {
            // Only legal directly under `Clause::Let`; if reached here (a
            // nested `?`) evaluate best-effort by unwrapping.
            let f = *ctx
                .program
                .partial_fns
                .get(name)
                .unwrap_or_else(|| panic!("unregistered partial fn `{name}`"));
            let vals: Vec<Value> = args.iter().map(|a| eval_expr(env, a, ctx)).collect();
            match f(&vals) {
                Ok(v) => v,
                Err(e) => panic!("unhandled `?` failure outside `let`: {e:?}"),
            }
        }
        Expr::Count(clauses) => {
            let solutions = eval_body_inner(clauses, vec![Env::new()], ctx);
            Value::Nat(solutions.len() as u64)
        }
        Expr::Sum(clauses, yield_expr) => {
            let solutions = eval_body_inner(clauses, vec![Env::new()], ctx);
            let mut total: i128 = 0;
            for s in &solutions {
                let v = eval_expr(s, yield_expr, ctx);
                total += v.as_i128().unwrap_or_else(|| {
                    panic!("sum() yield expression did not evaluate to a number")
                });
            }
            Value::Int(total.try_into().unwrap_or(i64::MAX))
        }
    }
}

fn eval_binop(op: BinOp, a: &Value, b: &Value) -> Value {
    use BinOp::*;
    match op {
        Add | Sub | Mul => {
            let x = a.as_i128().expect("arithmetic on a non-numeric value");
            let y = b.as_i128().expect("arithmetic on a non-numeric value");
            let r = match op {
                Add => x + y,
                Sub => x - y,
                Mul => x * y,
                _ => unreachable!(),
            };
            Value::Int(r.try_into().unwrap_or(i64::MAX))
        }
        Eq => Value::Bool(a == b),
        Ne => Value::Bool(a != b),
        Lt | Le | Gt | Ge => {
            let x = a.as_i128().expect("comparison on a non-numeric value");
            let y = b.as_i128().expect("comparison on a non-numeric value");
            Value::Bool(match op {
                Lt => x < y,
                Le => x <= y,
                Gt => x > y,
                Ge => x >= y,
                _ => unreachable!(),
            })
        }
        And => Value::Bool(a.as_bool().unwrap_or(false) && b.as_bool().unwrap_or(false)),
        Or => Value::Bool(a.as_bool().unwrap_or(false) || b.as_bool().unwrap_or(false)),
    }
}

/// Evaluate a clause body against the current envs, without rule-error
/// bookkeeping (used for `without`-existence checks and aggregate
/// sub-patterns, neither of which surface `RuleError`s of their own in this
/// pass — see the `Clause::Without` arm docs in `program.rs`).
fn eval_body_inner(clauses: &[Clause], mut envs: Vec<Env>, ctx: &Ctx) -> Vec<Env> {
    for clause in clauses {
        if envs.is_empty() {
            break;
        }
        envs = match clause {
            Clause::Edge { rel, bind_id, args } => {
                let def = ctx
                    .program
                    .relations
                    .get(rel)
                    .unwrap_or_else(|| panic!("unknown relation `{rel}`"));
                let extent = ctx
                    .live
                    .get(rel)
                    .unwrap_or_else(|| panic!("relation `{rel}` has no live extent"));
                let mut out = Vec::new();
                for env in &envs {
                    for record in extent.values() {
                        if let Some(mut new_env) = unify(env, args, &record.row) {
                            if let Some(v) = bind_id {
                                let ref_value = def.ref_value(&record.row);
                                // `bind_id` joins like any other variable: if
                                // `v` is already bound (e.g. from an earlier
                                // clause's role-arg — `Move(vehicle: v)` then
                                // `v: Vehicle { ... }`, the flagship's own
                                // `PriceOrder`/`Capacity` pattern), this
                                // record only matches when its identity
                                // agrees with that binding. Previously this
                                // unconditionally overwrote `v`, silently
                                // turning an intended join into an
                                // unconstrained iteration over every row of
                                // `rel` — caught by issue #24's flagship
                                // acceptance test tripping a `Capacity`
                                // violation that shouldn't have fired.
                                match new_env.get(v) {
                                    Some(existing) if *existing != ref_value => continue,
                                    _ => {
                                        new_env.insert(v.clone(), ref_value);
                                    }
                                }
                            }
                            out.push(new_env);
                        }
                    }
                }
                out
            }
            Clause::History { rel, args } => {
                let extent = ctx
                    .history
                    .get(rel)
                    .unwrap_or_else(|| panic!("relation `{rel}` has no history extent"));
                let mut out = Vec::new();
                for env in &envs {
                    for record in extent.values() {
                        if let Some(new_env) = unify(env, args, &record.row) {
                            out.push(new_env);
                        }
                    }
                }
                out
            }
            Clause::Without(inner) => envs
                .into_iter()
                .filter(|env| eval_body_inner(inner, vec![env.clone()], ctx).is_empty())
                .collect(),
            Clause::When(e) => envs
                .into_iter()
                .filter(|env| eval_expr(env, e, ctx).as_bool() == Some(true))
                .collect(),
            Clause::Let(v, e) => {
                let mut out = Vec::new();
                for env in envs {
                    match e {
                        Expr::Try(name, args) => {
                            let f = *ctx
                                .program
                                .partial_fns
                                .get(name)
                                .unwrap_or_else(|| panic!("unregistered partial fn `{name}`"));
                            let vals: Vec<Value> =
                                args.iter().map(|a| eval_expr(&env, a, ctx)).collect();
                            if let Ok(val) = f(&vals) {
                                let mut ne = env;
                                ne.insert(v.clone(), val);
                                out.push(ne);
                            }
                            // Err case silently dropped here — the
                            // rule-error-recording variant is `eval_rule_body`.
                        }
                        _ => {
                            let val = eval_expr(&env, e, ctx);
                            let mut ne = env;
                            ne.insert(v.clone(), val);
                            out.push(ne);
                        }
                    }
                }
                out
            }
        };
    }
    envs
}

/// Same clause-body evaluator as `eval_body_inner`, but for `Clause::Let`
/// over an `Expr::Try` that fails, records a `RuleError` (Part III §9)
/// instead of silently dropping the match.
fn eval_rule_body(
    clauses: &[Clause],
    ctx: &Ctx,
    rule_id: &RuleId,
    at_revision: u64,
    rule_errors: &mut Vec<RuleErrorEdge>,
) -> Vec<Env> {
    let mut envs = vec![Env::new()];
    for (site_idx, clause) in clauses.iter().enumerate() {
        if envs.is_empty() {
            break;
        }
        envs = match clause {
            Clause::Let(v, Expr::Try(name, args)) => {
                let f = *ctx
                    .program
                    .partial_fns
                    .get(name)
                    .unwrap_or_else(|| panic!("unregistered partial fn `{name}`"));
                let mut out = Vec::new();
                for env in envs {
                    let vals: Vec<Value> = args.iter().map(|a| eval_expr(&env, a, ctx)).collect();
                    match f(&vals) {
                        Ok(val) => {
                            let mut ne = env;
                            ne.insert(v.clone(), val);
                            out.push(ne);
                        }
                        Err(error) => rule_errors.push(RuleErrorEdge {
                            rule: rule_id.clone(),
                            site: format!("{rule_id}#{site_idx}"),
                            partial_match: env_digest(&env),
                            error,
                            at_revision,
                        }),
                    }
                }
                out
            }
            other => eval_body_inner(std::slice::from_ref(other), envs, ctx),
        };
    }
    envs
}

/// Filter `all_candidates[relation]` to a live view: not masked, and (for
/// `Derived`-kind relations only) not part of an unresolved key conflict.
/// Returns the filtered extent plus, for `Derived` relations, the fresh
/// `KeyConflict` edges implied by the *current* candidate set (Part III
/// §8). Called repeatedly ("no caching") as candidates accumulate.
fn refresh_live(
    program: &Program,
    all_candidates: &BTreeMap<RelName, Extent>,
    masked_targets: &BTreeMap<RelName, BTreeSet<CanonBytes>>,
    at_revision: u64,
) -> (BTreeMap<RelName, Extent>, Vec<KeyConflictEdge>) {
    let mut live = BTreeMap::new();
    let mut conflicts = Vec::new();

    for (name, def) in &program.relations {
        let candidates = all_candidates.get(name).cloned().unwrap_or_default();
        let masked = masked_targets.get(name);

        // Key-conflict detection (Part III §8) applies to `Derived` and to
        // `Entity` relations: an `Entity` row may come either from a
        // transaction (`ensure`/`fresh`, ground-like) or from a rule's
        // `keyed by (...)` head (derived-like — Part III §3), and Part III
        // §8's opening sentence ("every relation with key(...)") is not
        // restricted to its four named sub-cases, which distinguish
        // ground-vs-derived origin rather than enumerate every relation
        // kind. Two `ensure`s of the same key with different non-key field
        // values (an unspecified case — see `spec/errata/0001-*.md`) are
        // therefore also exposed as a `KeyConflict`, never silently
        // resolved (Part III §8: "There is never a silent winner").
        if !matches!(
            def.kind,
            crate::program::RelKind::Derived | crate::program::RelKind::Entity
        ) {
            // Ground/State/Event: masking may still apply (a mask can
            // target any relation kind — Part III §6 does not restrict
            // it), but conflict handling for these kinds is a
            // **transaction**-time concern (Part III §8's first three
            // sub-cases), not a settlement-time one.
            let filtered: Extent = candidates
                .into_iter()
                .filter(|(key, _)| !masked.map(|m| m.contains(key)).unwrap_or(false))
                .collect();
            live.insert(name.clone(), filtered);
            continue;
        }

        // Group live (unmasked) candidates by key to find conflicts.
        let mut groups: BTreeMap<CanonBytes, Vec<(&CanonBytes, &EdgeRecord)>> = BTreeMap::new();
        for (key, record) in candidates.iter() {
            if masked.map(|m| m.contains(key)).unwrap_or(false) {
                continue;
            }
            groups
                .entry(def.key_bytes(&record.row))
                .or_default()
                .push((key, record));
        }

        let mut filtered = Extent::new();
        for (key_bytes, members) in groups {
            if members.len() <= 1 {
                for (k, r) in members {
                    filtered.insert(k.clone(), r.clone());
                }
                continue;
            }
            // Unresolved conflict: no ordinary live value (Part III §8).
            let mut candidate_ids = BTreeSet::new();
            let mut supports = BTreeSet::new();
            for (_, r) in &members {
                candidate_ids.insert(candidate_digest(def, &r.row));
                supports.extend(r.supports.iter().cloned());
            }
            conflicts.push(KeyConflictEdge {
                relation: name.clone(),
                key: key_bytes,
                candidates: candidate_ids,
                supports,
                at_revision,
            });
        }
        live.insert(name.clone(), filtered);
    }
    (live, conflicts)
}

/// A content-sensitive fingerprint for one candidate row under a
/// conflicted key. Deliberately distinct from `RelationDef::digest`
/// (identity): for `Entity` kind that hashes *only* the key fields, so it
/// cannot tell two disagreeing candidate rows under one key apart — every
/// candidate in a conflict group shares the same key by construction, so
/// using identity here would always collapse them to one `Digest`
/// (errata 0001: entity key-conflict candidates must be individually
/// visible, "never a silent winner," the same guarantee `Derived`/`Ground`
/// already get from their content-sensitive `edge_id`).
fn candidate_digest(def: &RelationDef, row: &Row) -> Digest {
    let mut w = CanonWriter::new();
    w.write_tag(&def.name);
    row.canon_write(&mut w);
    Digest::of(Domain::Value, &w.finish())
}

/// Run one phase's naive fixpoint: repeatedly evaluate every rule in the
/// phase against the current live view, adding newly-derived rows to
/// `all_candidates`/`masked_targets` until nothing changes (Part III §5,
/// Appendix F step 6: "within a phase: least fixpoint, semi-naive,
/// order-free" — this crate runs it naive; see module docs).
#[allow(clippy::too_many_arguments)]
fn run_phase(
    program: &Program,
    phase: &Phase,
    all_candidates: &mut BTreeMap<RelName, Extent>,
    masked_targets: &mut BTreeMap<RelName, BTreeSet<CanonBytes>>,
    history: &BTreeMap<RelName, Extent>,
    at_revision: u64,
    supports_out: &mut Vec<SupportEdge>,
    masked_out: &mut Vec<MaskedEdge>,
    rule_errors_out: &mut Vec<RuleErrorEdge>,
    bindings_out: &mut BTreeMap<Digest, BTreeMap<String, Value>>,
    rule_error_seen: &mut BTreeSet<(RuleId, String, Digest)>,
    masked_seen: &mut BTreeSet<(EdgeId, EdgeId, RelName, RuleId)>,
) {
    loop {
        let (live, _conflicts) = refresh_live(program, all_candidates, masked_targets, at_revision);
        let ctx = Ctx {
            program,
            live: &live,
            history,
        };
        let mut changed = false;

        for rule_id in &phase.rules {
            let rule = &program.rules[rule_id];
            let mut errs = Vec::new();
            let solutions = eval_rule_body(&rule.body, &ctx, rule_id, at_revision, &mut errs);
            // The naive fixpoint re-evaluates every rule from scratch each
            // iteration, so a site that fails on iteration 1 fails again on
            // every later iteration too. Gate on the (rule, site,
            // partialMatch) identity — same idempotent-insertion pattern as
            // `entry.supports.insert(..)` above — so re-derivation is a
            // no-op and exactly one `RuleError` survives per distinct
            // failing site/match, in deterministic (BTreeMap iteration)
            // emission order.
            for err in errs {
                let key = (err.rule.clone(), err.site.clone(), err.partial_match);
                if rule_error_seen.insert(key) {
                    rule_errors_out.push(err);
                }
            }

            match &rule.head {
                Head::Tuple { rel, args } => {
                    let def = &program.relations[rel];
                    for env in &solutions {
                        let mut row = Row::new();
                        for (role, term) in args {
                            let v = match term {
                                crate::program::Term::Var(v) => env
                                    .get(v)
                                    .unwrap_or_else(|| panic!("unbound head variable `{v}`"))
                                    .clone(),
                                crate::program::Term::Const(c) => c.clone(),
                            };
                            row.insert(role.clone(), v);
                        }
                        let key = row_key(&row);
                        let digest = env_digest(env);
                        bindings_out.insert(digest, env.clone());
                        let support = SupportRef {
                            rule: rule_id.clone(),
                            match_digest: digest,
                        };
                        let entry = all_candidates
                            .entry(rel.clone())
                            .or_default()
                            .entry(key)
                            .or_insert_with(|| EdgeRecord {
                                row: row.clone(),
                                ..Default::default()
                            });
                        if entry.supports.insert(support.clone()) {
                            changed = true;
                            supports_out.push(SupportEdge {
                                edge: def.digest(&entry.row),
                                relation: rel.clone(),
                                rule: support.rule,
                                match_digest: support.match_digest,
                                at_revision,
                            });
                        }
                    }
                }
                Head::Mask {
                    relation,
                    target,
                    reason,
                } => {
                    let def = &program.relations[relation];
                    for env in &solutions {
                        let target_val = env
                            .get(target)
                            .unwrap_or_else(|| panic!("mask target `{target}` unbound"));
                        let reason_val = env
                            .get(reason)
                            .unwrap_or_else(|| panic!("mask reason `{reason}` unbound"));
                        // Part III §6: both operands are edge references.
                        let target_edge = match target_val {
                            Value::Edge(e) => *e,
                            _ => panic!("mask target must be an EdgeRef (Part III §6)"),
                        };
                        let reason_edge = match reason_val {
                            Value::Edge(e) => *e,
                            _ => panic!("mask reason must be an EdgeRef (Part III §6)"),
                        };
                        // Find the target row (by recomputed EdgeId) so we
                        // can mark its canon-bytes key masked.
                        if let Some((row_key_bytes, _)) = all_candidates
                            .get(relation)
                            .into_iter()
                            .flatten()
                            .find(|(_, r)| def.edge_id(&r.row) == target_edge)
                        {
                            let row_key_bytes = row_key_bytes.clone();
                            let was_masked = masked_targets
                                .entry(relation.clone())
                                .or_default()
                                .insert(row_key_bytes);
                            if was_masked {
                                changed = true;
                            }
                        }
                        // Same latent duplicate-per-iteration issue as the
                        // `RuleError` accumulation above: gate on
                        // (target, by, relation, rule) so a mask re-derived
                        // on a later fixpoint iteration is a no-op.
                        let masked_key =
                            (target_edge, reason_edge, relation.clone(), rule_id.clone());
                        if masked_seen.insert(masked_key) {
                            masked_out.push(MaskedEdge {
                                target: target_edge,
                                by: reason_edge,
                                relation: relation.clone(),
                                rule: rule_id.clone(),
                                at_phase: phase.id,
                                at_revision,
                            });
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

/// Public entry point: settle `program` over `ground` (the committed ground
/// extents post-supersession/retraction, i.e. `Base(r)` — Part III §4),
/// `ground_history` (full history for `history`-clause reads, Ground/State/
/// Event kinds only), across `phases` (from `crate::phase::infer_phases` or
/// a precomputed phase list once brix-phase lands), producing revision
/// `at_revision`.
pub fn settle(
    program: &Program,
    phases: &[Phase],
    ground: &BTreeMap<RelName, Extent>,
    ground_history: &BTreeMap<RelName, Extent>,
    at_revision: u64,
) -> Settled {
    let mut all_candidates: BTreeMap<RelName, Extent> = BTreeMap::new();
    for (name, def) in &program.relations {
        if def.kind == crate::program::RelKind::Derived {
            all_candidates.insert(name.clone(), Extent::new());
        } else {
            all_candidates.insert(name.clone(), ground.get(name).cloned().unwrap_or_default());
        }
    }
    // Ground claims are provenance too (Part III §11).
    let mut claims = Vec::new();
    for (name, def) in &program.relations {
        if def.kind == crate::program::RelKind::Derived {
            continue;
        }
        for record in ground.get(name).into_iter().flat_map(|e| e.values()) {
            let edge = def.digest(&record.row);
            for claim in &record.claims {
                claims.push(ClaimEdge {
                    edge,
                    relation: name.clone(),
                    claim: *claim,
                    at_revision,
                });
            }
        }
    }

    let mut masked_targets: BTreeMap<RelName, BTreeSet<CanonBytes>> = BTreeMap::new();
    let mut supports = Vec::new();
    let mut masked = Vec::new();
    let mut rule_errors = Vec::new();
    let mut bindings = BTreeMap::new();
    // Identity-keyed dedup accumulators for the naive per-iteration
    // re-evaluation (see `run_phase` docs): shared across every phase of
    // this revision so a rule/site/mask can never be double-recorded even
    // if (hypothetically) re-derived from a later phase.
    let mut rule_error_seen: BTreeSet<(RuleId, String, Digest)> = BTreeSet::new();
    let mut masked_seen: BTreeSet<(EdgeId, EdgeId, RelName, RuleId)> = BTreeSet::new();

    for phase in phases {
        run_phase(
            program,
            phase,
            &mut all_candidates,
            &mut masked_targets,
            ground_history,
            at_revision,
            &mut supports,
            &mut masked,
            &mut rule_errors,
            &mut bindings,
            &mut rule_error_seen,
            &mut masked_seen,
        );
    }

    let (live, key_conflicts) =
        refresh_live(program, &all_candidates, &masked_targets, at_revision);

    // Constraints: evaluated once, over the fully settled, mask-and-conflict
    // -filtered snapshot (Part IV §7: "evaluated against the fully settled
    // candidate revision").
    let ctx = Ctx {
        program,
        live: &live,
        history: ground_history,
    };
    let mut violations = Vec::new();
    for constraint in program.constraints.values() {
        let solutions = eval_body_inner(&constraint.body, vec![Env::new()], &ctx);
        for env in solutions {
            let digest = env_digest(&env);
            bindings.insert(digest, env.clone());
            violations.push(ViolationEdge {
                constraint: constraint.id.clone(),
                match_digest: digest,
                bindings: env,
                at_revision,
            });
        }
    }

    Settled {
        at_revision,
        extents: live,
        provenance: Provenance {
            supports,
            claims,
            masked,
            key_conflicts,
            rule_errors,
            violations,
            match_bindings: bindings,
        },
    }
}
