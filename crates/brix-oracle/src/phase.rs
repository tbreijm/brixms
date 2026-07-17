//! Phase inference — Appendix F, transcribed directly (not via `petgraph`;
//! this crate hand-rolls a small deterministic Tarjan SCC instead of taking
//! a dependency shared with the not-yet-landed brix-phase lane, keeping the
//! oracle self-contained per the task's ordering note).
//!
//! Algorithm, matching Appendix F's five numbered steps:
//! 1. positive edge r1 -> r2 when r2 reads (ordinarily, live) a relation r1
//!    derives;
//! 2. strict edge r1 => r2 when r2 reads r1's relation through `without`, an
//!    aggregate sub-pattern, or (not implemented: witness reads — Complete
//!    checking is static semantics, out of this pass's scope);
//! 3. mask edges: producers(R) => M(R), and M(R) => every ordinary live
//!    read-site of R excluding the target binding inside each `m ∈ M(R)`'s
//!    own body; `history` reads create no edge;
//! 4. condense SCCs of *positive* edges only; a strict/mask edge with both
//!    endpoints in one positive-SCC is a compile-time cycle error;
//! 5. phases are the topological order of the condensation, with
//!    strict/mask edges (projected to the SCC level) as extra ordering
//!    constraints; a residual cycle at that level is also an error (a
//!    non-monotone edge closing a longer cycle through several SCCs).
//!
//! Within a phase (step 6): the evaluator runs positive recursion to a
//! least fixpoint, order-free — see `crate::eval`.

use std::collections::{BTreeMap, BTreeSet};

use crate::program::{Clause, Expr, Head, Program, RuleId};

/// One inferred phase: an ordered position and the positive-SCC of rules
/// that settle together (mutual positive recursion allowed within).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase {
    pub id: usize,
    pub rules: Vec<RuleId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhaseError {
    /// A strict or mask edge closes a cycle — either directly within one
    /// positive-SCC, or indirectly across several SCCs at the condensation
    /// level. `from`/`to` name one offending edge; full minimal-path
    /// extraction (the "extra day of care" Ring0_Build_Plan §1.5 assigns to
    /// brix-phase) is not reproduced here.
    CycleThroughNonMonotoneEdge {
        from: RuleId,
        to: RuleId,
        reason: &'static str,
    },
}

impl std::fmt::Display for PhaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhaseError::CycleThroughNonMonotoneEdge { from, to, reason } => write!(
                f,
                "cycle through a non-monotone edge: {from} -> {to} ({reason})"
            ),
        }
    }
}
impl std::error::Error for PhaseError {}

#[derive(Clone, Debug)]
struct ReadSite {
    rel: String,
    strict: bool,
    is_target_binding: bool,
}

/// Collect every relation this rule's body reads, with enough detail to
/// build both the positive/strict edges and the mask edges. `History`
/// clauses are walked but contribute nothing (Appendix F #3, Part III §6
/// rule 3: no dependency).
fn relation_reads(rule: &crate::program::Rule) -> Vec<ReadSite> {
    let mask_target: Option<(&str, &str)> = match &rule.head {
        Head::Mask {
            relation, target, ..
        } => Some((relation.as_str(), target.as_str())),
        _ => None,
    };
    let mut out = Vec::new();
    walk_clauses(&rule.body, false, mask_target, &mut out);
    out
}

fn walk_clauses(
    clauses: &[Clause],
    strict: bool,
    mask_target: Option<(&str, &str)>,
    out: &mut Vec<ReadSite>,
) {
    for c in clauses {
        match c {
            Clause::Edge { rel, bind_id, .. } => {
                let is_target_binding = mask_target
                    .map(|(mrel, mvar)| mrel == rel.as_str() && bind_id.as_deref() == Some(mvar))
                    .unwrap_or(false);
                out.push(ReadSite {
                    rel: rel.clone(),
                    strict,
                    is_target_binding,
                });
            }
            Clause::Without(inner) => walk_clauses(inner, true, mask_target, out),
            Clause::History { .. } => {}
            Clause::When(e) => walk_expr(e, out),
            Clause::Let(_, e) => walk_expr(e, out),
        }
    }
}

fn walk_expr(e: &Expr, out: &mut Vec<ReadSite>) {
    match e {
        Expr::Var(_) | Expr::Const(_) => {}
        Expr::BinOp(_, a, b) => {
            walk_expr(a, out);
            walk_expr(b, out);
        }
        Expr::Call(_, args) | Expr::Try(_, args) => {
            for a in args {
                walk_expr(a, out);
            }
        }
        Expr::Count(clauses) => walk_clauses(clauses, true, None, out),
        Expr::Sum(clauses, yield_expr) => {
            walk_clauses(clauses, true, None, out);
            walk_expr(yield_expr, out);
        }
    }
}

/// Run Appendix F over `program` and return the phase order.
pub fn infer_phases(program: &Program) -> Result<Vec<Phase>, PhaseError> {
    let rule_ids: Vec<RuleId> = program.rules.keys().cloned().collect(); // BTreeMap: sorted, deterministic

    // Step 1/2: positive + strict edges from ordinary relation reads.
    let mut positive_edges: BTreeSet<(RuleId, RuleId)> = BTreeSet::new();
    let mut strict_edges: BTreeSet<(RuleId, RuleId)> = BTreeSet::new();
    let mut reads_by_rule: BTreeMap<RuleId, Vec<ReadSite>> = BTreeMap::new();

    for (id, rule) in &program.rules {
        reads_by_rule.insert(id.clone(), relation_reads(rule));
    }

    for (consumer_id, sites) in &reads_by_rule {
        for site in sites {
            for producer in program.producers(&site.rel) {
                // A self-referential positive read (ordinary recursion) is
                // still a real edge — it is what puts the rule in a
                // nontrivial SCC with itself.
                let edge = (producer.id.clone(), consumer_id.clone());
                if site.strict {
                    strict_edges.insert(edge);
                } else {
                    positive_edges.insert(edge);
                }
            }
        }
    }

    // Step 3: mask edges.
    for relation in program.relations.keys() {
        let producers = program.producers(relation);
        let maskers = program.mask_producers(relation);
        if maskers.is_empty() {
            continue;
        }
        for p in &producers {
            for m in &maskers {
                strict_edges.insert((p.id.clone(), m.id.clone()));
            }
        }
        for m in &maskers {
            for (consumer_id, sites) in &reads_by_rule {
                for site in sites {
                    if site.rel != *relation {
                        continue;
                    }
                    if consumer_id == &m.id && site.is_target_binding {
                        continue; // excluded: the mask rule's own target binding
                    }
                    strict_edges.insert((m.id.clone(), consumer_id.clone()));
                }
            }
        }
    }

    // Stratification is predicate-level, not rule-level: when two or more
    // ordinary Tuple-head rules derive the *same* relation, they must settle
    // in the same phase even if neither reads the other's output directly
    // (e.g. transitive-closure's `Base`/`Trans` both deriving `Reach`). A
    // rule-granular positive-edge graph alone would split such co-producers
    // into separate SCCs/phases, which is wrong. Union them here by adding a
    // synthetic positive cycle p1->p2->...->pk->p1 over the *sorted*
    // Tuple-head producer ids of each relation, before SCC condensation.
    // `producers` already excludes Mask-head rules (see its doc comment) —
    // mask rules keep their own node and are ordered solely by the strict
    // mask edges from step 3.
    for relation in program.relations.keys() {
        let mut producer_ids: Vec<RuleId> = program
            .producers(relation)
            .into_iter()
            .map(|r| r.id.clone())
            .collect();
        producer_ids.sort();
        if producer_ids.len() < 2 {
            continue;
        }
        for pair in producer_ids.windows(2) {
            positive_edges.insert((pair[0].clone(), pair[1].clone()));
        }
        positive_edges.insert((
            producer_ids[producer_ids.len() - 1].clone(),
            producer_ids[0].clone(),
        ));
    }

    // Step 4: condense SCCs of positive edges only.
    let mut positive_adj: BTreeMap<RuleId, BTreeSet<RuleId>> = BTreeMap::new();
    for id in &rule_ids {
        positive_adj.entry(id.clone()).or_default();
    }
    for (from, to) in &positive_edges {
        positive_adj
            .entry(from.clone())
            .or_default()
            .insert(to.clone());
    }
    let sccs = tarjan_scc(&rule_ids, &positive_adj);
    // component index per rule id
    let mut component_of: BTreeMap<RuleId, usize> = BTreeMap::new();
    for (i, comp) in sccs.iter().enumerate() {
        for r in comp {
            component_of.insert(r.clone(), i);
        }
    }

    // Any strict/mask edge inside one SCC is a direct cycle error.
    for (from, to) in &strict_edges {
        if component_of[from] == component_of[to] {
            return Err(PhaseError::CycleThroughNonMonotoneEdge {
                from: from.clone(),
                to: to.clone(),
                reason: "strict or mask edge inside one positive-recursion component",
            });
        }
    }

    // Step 5: condensation DAG — SCC-level edges from every positive or
    // strict/mask edge crossing components — then topological sort.
    let n = sccs.len();
    let mut condensed_adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let mut condensed_edge_witness: BTreeMap<(usize, usize), (RuleId, RuleId)> = BTreeMap::new();
    for (from, to) in positive_edges.iter().chain(strict_edges.iter()) {
        let a = component_of[from];
        let b = component_of[to];
        if a != b {
            condensed_adj[a].insert(b);
            condensed_edge_witness
                .entry((a, b))
                .or_insert_with(|| (from.clone(), to.clone()));
        }
    }

    let order = topo_sort(n, &condensed_adj).ok_or_else(|| {
        // Report an arbitrary offending edge from the still-cyclic region;
        // exact minimal-path extraction is out of scope for this pass.
        let (&(a, b), (from, to)) = condensed_edge_witness
            .iter()
            .next()
            .expect("topo_sort reported a cycle but condensation has no cross-component edges");
        let _ = (a, b);
        PhaseError::CycleThroughNonMonotoneEdge {
            from: from.clone(),
            to: to.clone(),
            reason: "non-monotone edge closes a cycle across several phase components",
        }
    })?;

    let mut phases = Vec::with_capacity(order.len());
    for (phase_id, comp_idx) in order.into_iter().enumerate() {
        let mut rules = sccs[comp_idx].clone();
        rules.sort();
        phases.push(Phase {
            id: phase_id,
            rules,
        });
    }
    Ok(phases)
}

/// Deterministic (sorted-input) recursive Tarjan SCC. Returns components in
/// an arbitrary but stable order (driven entirely by the sorted `nodes`
/// iteration and each node's sorted adjacency set).
fn tarjan_scc(nodes: &[RuleId], adj: &BTreeMap<RuleId, BTreeSet<RuleId>>) -> Vec<Vec<RuleId>> {
    struct State<'a> {
        adj: &'a BTreeMap<RuleId, BTreeSet<RuleId>>,
        index: BTreeMap<RuleId, usize>,
        low: BTreeMap<RuleId, usize>,
        on_stack: BTreeSet<RuleId>,
        stack: Vec<RuleId>,
        counter: usize,
        out: Vec<Vec<RuleId>>,
    }
    fn strongconnect(v: &RuleId, s: &mut State) {
        s.index.insert(v.clone(), s.counter);
        s.low.insert(v.clone(), s.counter);
        s.counter += 1;
        s.stack.push(v.clone());
        s.on_stack.insert(v.clone());

        if let Some(succs) = s.adj.get(v) {
            let succs: Vec<RuleId> = succs.iter().cloned().collect();
            for w in succs {
                if !s.index.contains_key(&w) {
                    strongconnect(&w, s);
                    let low_w = s.low[&w];
                    let low_v = s.low[v];
                    s.low.insert(v.clone(), low_v.min(low_w));
                } else if s.on_stack.contains(&w) {
                    let idx_w = s.index[&w];
                    let low_v = s.low[v];
                    s.low.insert(v.clone(), low_v.min(idx_w));
                }
            }
        }

        if s.low[v] == s.index[v] {
            let mut comp = Vec::new();
            loop {
                let w = s.stack.pop().expect("stack underflow in Tarjan SCC");
                s.on_stack.remove(&w);
                let done = &w == v;
                comp.push(w);
                if done {
                    break;
                }
            }
            comp.sort();
            s.out.push(comp);
        }
    }

    let mut state = State {
        adj,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        counter: 0,
        out: Vec::new(),
    };
    for v in nodes {
        if !state.index.contains_key(v) {
            strongconnect(v, &mut state);
        }
    }
    state.out
}

/// Kahn's algorithm with deterministic tie-breaking (lowest component index
/// first among ready nodes). Returns `None` if the graph is not a DAG.
fn topo_sort(n: usize, adj: &[BTreeSet<usize>]) -> Option<Vec<usize>> {
    let mut indegree = vec![0usize; n];
    for edges in adj {
        for &to in edges {
            indegree[to] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(&next) = ready.iter().next() {
        ready.remove(&next);
        order.push(next);
        for &to in &adj[next] {
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.insert(to);
            }
        }
    }
    if order.len() == n {
        Some(order)
    } else {
        None
    }
}
