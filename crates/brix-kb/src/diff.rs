//! `brix kb diff <revA> <revB>` — what changed between two revisions, and why
//! (ADR-0041).
//!
//! Every comparison replays both revisions fresh (`ops::load_and_replay`) —
//! this is the "impact analysis must be honest" requirement: there is no
//! incremental evaluator here, so nothing is inferred about what *should*
//! have changed. What "why" adds on top is the dependency graph
//! (`crate::deps`), used only to *narrow* the honest, fully-recomputed
//! before/after facts down to a short explanation of which changed inputs (or
//! rule dependencies) plausibly caused a given fact's value to change.

use std::collections::BTreeMap;
use std::path::PathBuf;

use brix_lower::finite_decision::{
    CandidateStatus, FiniteDecisionCommit, FiniteDecisionContract, FiniteDecisionFnParam,
    FiniteDecisionFunction, FiniteDecisionInput, FiniteDecisionPlan, FiniteDecisionProposal,
    FiniteDecisionRule, FiniteDecisionRun,
};
use brix_lower::input::InputValue;
use brix_lower::l3_v2::L3ValueV2;

use crate::deps::build_dep_graph;
use crate::error::KbError;
use crate::ops;
use crate::pipeline::ReplayResult;
use crate::revision::Status;

/// One input's change between revision A and revision B.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputChange {
    pub name: String,
    pub old: Option<InputValue>,
    pub new: Option<InputValue>,
}

/// One fact's change, with why it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactChange {
    pub name: String,
    pub old: L3ValueV2,
    pub new: L3ValueV2,
    /// Changed input names this fact transitively depends on.
    pub why_inputs: Vec<String>,
    /// Other changed facts (rules) this fact transitively depends on.
    pub why_rules: Vec<String>,
}

/// One candidate's disposition change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateChange {
    pub name: String,
    pub old_status: CandidateStatus,
    pub new_status: CandidateStatus,
}

/// One declaration's change when the program itself changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclChange {
    pub kind: &'static str,
    pub name: String,
    pub change: &'static str,
}

/// A fully computed diff between two revisions of the same knowledge base.
/// One `decide` block instance whose selected candidate differs between two
/// revisions (`None` means quiescent, or absent from that revision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityChange {
    pub decide: String,
    pub index: usize,
    pub old: Option<String>,
    pub new: Option<String>,
}

pub struct DiffReport {
    pub rev_a: u64,
    pub rev_b: u64,
    pub status_a: Status,
    pub status_b: Status,
    pub program_changed: bool,
    pub decl_changes: Vec<DeclChange>,
    pub inputs_added: Vec<InputChange>,
    pub inputs_removed: Vec<InputChange>,
    pub inputs_changed: Vec<InputChange>,
    pub facts_changed: Vec<FactChange>,
    pub facts_unchanged_count: usize,
    pub facts_added: Vec<String>,
    pub facts_removed: Vec<String>,
    pub candidates_changed: Vec<CandidateChange>,
    /// Per-entity decisions (ADR-0043) whose selected candidate differs.
    pub entities_changed: Vec<EntityChange>,
    pub decision_a: Option<(String, L3ValueV2)>,
    pub decision_b: Option<(String, L3ValueV2)>,
}

pub fn diff(
    root: &std::path::Path,
    rev_a: u64,
    rev_b: u64,
    package_paths: &[PathBuf],
) -> Result<DiffReport, KbError> {
    ops::read_manifest(root)?;
    let record_a = ops::read_revision(root, rev_a)?;
    let record_b = ops::read_revision(root, rev_b)?;
    let (plan_a, snapshot_a, replay_a) = ops::load_and_replay(root, &record_a, package_paths)?;
    let (plan_b, snapshot_b, replay_b) = ops::load_and_replay(root, &record_b, package_paths)?;

    let program_changed = record_a.program_id != record_b.program_id;
    let decl_changes = if program_changed {
        diff_declarations(&plan_a, &plan_b)
    } else {
        Vec::new()
    };

    let mut inputs_added = Vec::new();
    let mut inputs_removed = Vec::new();
    let mut inputs_changed = Vec::new();
    {
        let mut names: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
        names.extend(snapshot_a.values().keys());
        names.extend(snapshot_b.values().keys());
        for name in names {
            let old = snapshot_a.values().get(name);
            let new = snapshot_b.values().get(name);
            match (old, new) {
                (None, Some(n)) => inputs_added.push(InputChange {
                    name: name.clone(),
                    old: None,
                    new: Some(n.clone()),
                }),
                (Some(o), None) => inputs_removed.push(InputChange {
                    name: name.clone(),
                    old: Some(o.clone()),
                    new: None,
                }),
                (Some(o), Some(n)) if o != n => inputs_changed.push(InputChange {
                    name: name.clone(),
                    old: Some(o.clone()),
                    new: Some(n.clone()),
                }),
                _ => {}
            }
        }
    }
    let changed_input_names: std::collections::BTreeSet<String> = inputs_added
        .iter()
        .chain(inputs_removed.iter())
        .chain(inputs_changed.iter())
        .map(|c| c.name.clone())
        .collect();

    let graph_b = build_dep_graph(&plan_b);

    // Each side is compared on its own terms: a revision whose input
    // contract is incomplete ran nothing, so it contributes no facts,
    // candidates, or decisions, and everything the other side has shows as
    // added or removed rather than being dropped.
    let (run_a, run_b) = (ran(&replay_a), ran(&replay_b));

    let facts_of = |run: Option<&FiniteDecisionRun>| -> BTreeMap<String, L3ValueV2> {
        run.iter()
            .flat_map(|r| r.facts.iter())
            .map(|f| (f.rule.clone(), f.value.clone()))
            .collect()
    };
    let (facts_a, facts_b) = (facts_of(run_a), facts_of(run_b));
    let mut facts_changed_raw: Vec<(String, L3ValueV2, L3ValueV2)> = Vec::new();
    let mut facts_unchanged_count = 0usize;
    let mut facts_added = Vec::new();
    let mut facts_removed = Vec::new();
    let all_fact_names: std::collections::BTreeSet<&String> =
        facts_a.keys().chain(facts_b.keys()).collect();
    for name in all_fact_names {
        match (facts_a.get(name), facts_b.get(name)) {
            (Some(a), Some(b)) if a == b => facts_unchanged_count += 1,
            (Some(a), Some(b)) => facts_changed_raw.push((name.clone(), a.clone(), b.clone())),
            (Some(_), None) => facts_removed.push(name.clone()),
            (None, Some(_)) => facts_added.push(name.clone()),
            (None, None) => {}
        }
    }
    let changed_fact_names: std::collections::BTreeSet<String> = facts_changed_raw
        .iter()
        .map(|(n, _, _)| n.clone())
        .collect();

    // Candidates of every commit pool (ADR-0039); a candidate belongs to
    // exactly one pool, so names are unique across them.
    let dispositions_of = |run: Option<&FiniteDecisionRun>| -> BTreeMap<String, CandidateStatus> {
        run.iter()
            .flat_map(|r| r.commits.iter())
            .flat_map(|c| c.dispositions.iter())
            .map(|d| (d.name.clone(), d.status.clone()))
            .collect()
    };
    let (disp_a, disp_b) = (dispositions_of(run_a), dispositions_of(run_b));
    let mut candidates_changed = Vec::new();
    for (name, sb) in &disp_b {
        if let Some(sa) = disp_a.get(name) {
            if sa != sb {
                candidates_changed.push(CandidateChange {
                    name: name.clone(),
                    old_status: sa.clone(),
                    new_status: sb.clone(),
                });
            }
        }
    }

    // Per-entity decisions (ADR-0043), compared by block and element index.
    let entities_of =
        |run: Option<&FiniteDecisionRun>| -> BTreeMap<(String, usize), Option<String>> {
            run.iter()
                .flat_map(|r| r.decides.iter())
                .flat_map(|d| {
                    d.instances.iter().map(move |i| {
                        (
                            (d.decide.clone(), i.index),
                            i.decision.as_ref().map(|s| s.candidate.clone()),
                        )
                    })
                })
                .collect()
        };
    let (ent_a, ent_b) = (entities_of(run_a), entities_of(run_b));
    let mut entities_changed = Vec::new();
    let all_entities: std::collections::BTreeSet<&(String, usize)> =
        ent_a.keys().chain(ent_b.keys()).collect();
    for key in all_entities {
        let (old, new) = (ent_a.get(key).cloned(), ent_b.get(key).cloned());
        if old != new {
            entities_changed.push(EntityChange {
                decide: key.0.clone(),
                index: key.1,
                old: old.flatten(),
                new: new.flatten(),
            });
        }
    }

    let facts_changed = facts_changed_raw
        .into_iter()
        .map(|(name, old, new)| {
            let reach = graph_b.rules.get(&name).cloned().unwrap_or_default();
            let why_inputs: Vec<String> = reach
                .inputs
                .iter()
                .filter(|n| changed_input_names.contains(n.as_str()))
                .cloned()
                .collect();
            let why_rules: Vec<String> = reach
                .rules
                .iter()
                .filter(|r| changed_fact_names.contains(r.as_str()))
                .cloned()
                .collect();
            FactChange {
                name,
                old,
                new,
                why_inputs,
                why_rules,
            }
        })
        .collect();

    let decision_of = |run: Option<&FiniteDecisionRun>| {
        run.and_then(|r| r.decision.as_ref())
            .map(|d| (d.candidate.clone(), d.value.clone()))
    };

    Ok(DiffReport {
        rev_a,
        rev_b,
        status_a: record_a.result.status,
        status_b: record_b.result.status,
        program_changed,
        decl_changes,
        inputs_added,
        inputs_removed,
        inputs_changed,
        facts_changed,
        facts_unchanged_count,
        facts_added,
        facts_removed,
        candidates_changed,
        entities_changed,
        decision_a: decision_of(run_a),
        decision_b: decision_of(run_b),
    })
}

/// The run a replay produced, or `None` when its input contract was
/// incomplete and nothing ran.
fn ran(replay: &ReplayResult) -> Option<&FiniteDecisionRun> {
    match replay {
        ReplayResult::Ran { run, .. } => Some(run),
        ReplayResult::MissingInputs { .. } => None,
    }
}

fn diff_by_name<T>(
    old: &[T],
    new: &[T],
    name_of: impl Fn(&T) -> &str,
    eq: impl Fn(&T, &T) -> bool,
    kind: &'static str,
    out: &mut Vec<DeclChange>,
) {
    let old_map: BTreeMap<&str, &T> = old.iter().map(|t| (name_of(t), t)).collect();
    let new_map: BTreeMap<&str, &T> = new.iter().map(|t| (name_of(t), t)).collect();
    for (name, o) in &old_map {
        match new_map.get(name) {
            None => out.push(DeclChange {
                kind,
                name: name.to_string(),
                change: "removed",
            }),
            Some(n) => {
                if !eq(o, n) {
                    out.push(DeclChange {
                        kind,
                        name: name.to_string(),
                        change: "changed",
                    });
                }
            }
        }
    }
    for name in new_map.keys() {
        if !old_map.contains_key(name) {
            out.push(DeclChange {
                kind,
                name: name.to_string(),
                change: "added",
            });
        }
    }
}

fn diff_declarations(a: &FiniteDecisionPlan, b: &FiniteDecisionPlan) -> Vec<DeclChange> {
    let mut out = Vec::new();

    diff_by_name(
        &a.inputs,
        &b.inputs,
        |i: &FiniteDecisionInput| i.name.as_str(),
        |x, y| x.ty == y.ty,
        "input",
        &mut out,
    );
    diff_by_name(
        &a.rules,
        &b.rules,
        |r: &FiniteDecisionRule| r.name.as_str(),
        |x, y| {
            let dx: std::collections::BTreeSet<&String> = x.depends_on.iter().collect();
            let dy: std::collections::BTreeSet<&String> = y.depends_on.iter().collect();
            dx == dy && x.body == y.body
        },
        "rule",
        &mut out,
    );
    diff_by_name(
        &a.proposals,
        &b.proposals,
        |p: &FiniteDecisionProposal| p.name.as_str(),
        |x, y| {
            let dx: std::collections::BTreeSet<&String> = x.deps.iter().collect();
            let dy: std::collections::BTreeSet<&String> = y.deps.iter().collect();
            dx == dy && x.priority == y.priority && x.guard == y.guard && x.value == y.value
        },
        "propose",
        &mut out,
    );
    diff_by_name(
        &a.functions,
        &b.functions,
        |f: &FiniteDecisionFunction| f.name.as_str(),
        |x, y| {
            params_eq(&x.params, &y.params) && x.ret_contract == y.ret_contract && x.body == y.body
        },
        "fn",
        &mut out,
    );
    diff_by_name(
        &a.lets,
        &b.lets,
        |l: &(String, brix_lower::l3_v2::L3ExprV2)| l.0.as_str(),
        |x, y| x.1 == y.1,
        "let",
        &mut out,
    );

    let mut config_names: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    config_names.extend(a.schemas.keys());
    config_names.extend(b.schemas.keys());
    for name in config_names {
        match (a.schemas.get(name), b.schemas.get(name)) {
            (Some(_), None) => out.push(DeclChange {
                kind: "config",
                name: name.clone(),
                change: "removed",
            }),
            (None, Some(_)) => out.push(DeclChange {
                kind: "config",
                name: name.clone(),
                change: "added",
            }),
            (Some(x), Some(y)) if x != y => out.push(DeclChange {
                kind: "config",
                name: name.clone(),
                change: "changed",
            }),
            _ => {}
        }
    }

    let a_commits: std::collections::BTreeMap<&String, _> = a
        .commits
        .iter()
        .map(|c| (&c.name, commit_signature(c)))
        .collect();
    let b_commits: std::collections::BTreeMap<&String, _> = b
        .commits
        .iter()
        .map(|c| (&c.name, commit_signature(c)))
        .collect();
    let mut commit_names: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    commit_names.extend(a_commits.keys().copied());
    commit_names.extend(b_commits.keys().copied());
    for name in commit_names {
        match (a_commits.get(name), b_commits.get(name)) {
            (Some(_), None) => out.push(DeclChange {
                kind: "commit",
                name: name.to_string(),
                change: "removed",
            }),
            (None, Some(_)) => out.push(DeclChange {
                kind: "commit",
                name: name.to_string(),
                change: "added",
            }),
            (Some(x), Some(y)) if x != y => out.push(DeclChange {
                kind: "commit",
                name: name.to_string(),
                change: "changed",
            }),
            _ => {}
        }
    }

    out
}

fn params_eq(a: &[FiniteDecisionFnParam], b: &[FiniteDecisionFnParam]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.name == y.name && contract_eq(x.contract.as_ref(), y.contract.as_ref()))
}

fn contract_eq(a: Option<&FiniteDecisionContract>, b: Option<&FiniteDecisionContract>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.ty == y.ty && x.schema_ty == y.schema_ty,
        _ => false,
    }
}

fn commit_signature(c: &FiniteDecisionCommit) -> (String, std::collections::BTreeSet<String>) {
    (c.name.clone(), c.candidates.iter().cloned().collect())
}
