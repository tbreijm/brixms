//! brix-phase — Dependency graph, SCC, phase assignment, minimal offending
//! path (App. F).
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! Algorithm, matching Appendix F's five numbered steps, run over the
//! lane-neutral [`RuleFacts`] input (see [`input`]) rather than any single
//! lane's own program representation:
//! 1. positive edge r1 -> r2 when r2 reads (ordinarily, live) a relation r1
//!    derives;
//! 2. strict edge r1 => r2 when r2 reads r1's relation through `without`, an
//!    aggregate sub-pattern, or (not implemented: witness reads — Complete
//!    checking is static semantics, out of this pass's scope);
//! 3. mask edges: producers(R) => M(R), and M(R) => every ordinary live
//!    read-site of R excluding the target binding inside each `m ∈ M(R)`'s
//!    own body; `history` reads create no edge;
//! 4. errata 0002 (predicate-level condensation): stratification is
//!    predicate-level, not rule-level — two or more ordinary producers of
//!    the same relation must settle in the same phase even if neither reads
//!    the other's output directly (e.g. transitive closure's `Base`/`Trans`
//!    both deriving `Reach`). Modeled as a synthetic positive cycle over the
//!    sorted producer ids of each relation, before SCC condensation. Mask
//!    heads are excluded (they keep their own node, ordered solely by the
//!    strict mask edges from step 3);
//! 5. condense SCCs of *positive* edges only; a strict/mask edge with both
//!    endpoints in one positive-SCC is a compile-time cycle error, reported
//!    with the shortest positive-edge witness path back from `to` to `from`
//!    within that component;
//! 6. phases are the topological order of the condensation, with
//!    strict/mask edges (projected to the SCC level) as extra ordering
//!    constraints; a residual cycle at that level is also an error, reported
//!    with the shortest witness path through the condensation graph.
//!
//! Within a phase (step 7): the evaluator runs positive recursion to a
//! least fixpoint, order-free — owned by each phase's consumer (e.g.
//! `brix-oracle::eval`).

pub mod input;

pub use input::{Produces, ReadSite, RelId, RuleFacts, RuleId};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use brix_diag::{CanonValue, Diagnostic, Span};

/// Stable diagnostic for a strict or mask edge that makes stratification
/// impossible.  `BRX4xxx` is the semantic/phase-analysis code range.
pub const NON_MONOTONE_CYCLE: &str = "BRX4001";

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
    /// level. `from`/`to` name the offending edge; `path` is the shortest
    /// witness cycle through it (`from -> to -> ... -> from`).
    CycleThroughNonMonotoneEdge {
        from: RuleId,
        to: RuleId,
        reason: &'static str,
        path: Vec<RuleId>,
    },
}

impl std::fmt::Display for PhaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhaseError::CycleThroughNonMonotoneEdge {
                from,
                to,
                reason,
                path,
            } => {
                write!(
                    f,
                    "cycle through a non-monotone edge: {from} -> {to} ({reason}); witness: "
                )?;
                for (i, r) in path.iter().enumerate() {
                    if i > 0 {
                        write!(f, " -> ")?;
                    }
                    write!(f, "{r}")?;
                }
                Ok(())
            }
        }
    }
}
impl std::error::Error for PhaseError {}

impl PhaseError {
    /// Project the minimal phase witness into the shared diagnostic channel.
    /// Rule facts have no source span, so callers that retain source metadata
    /// may refine the empty primary span; the structural path itself is fully
    /// preserved for JSON/SARIF and later source mapping.
    pub fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::CycleThroughNonMonotoneEdge {
                from,
                to,
                reason,
                path,
            } => Diagnostic::error(NON_MONOTONE_CYCLE, Span::empty(0), self.to_string())
                .with_structure(CanonValue::Object(BTreeMap::from([
                    ("from".to_owned(), CanonValue::String(from.clone())),
                    (
                        "path".to_owned(),
                        CanonValue::List(path.iter().cloned().map(CanonValue::String).collect()),
                    ),
                    (
                        "reason".to_owned(),
                        CanonValue::String((*reason).to_owned()),
                    ),
                    ("to".to_owned(), CanonValue::String(to.clone())),
                ]))),
        }
    }
}

/// Run Appendix F over `rules` and return the phase order.
pub fn infer_phases(rules: &[RuleFacts]) -> Result<Vec<Phase>, PhaseError> {
    let mut rule_ids: Vec<RuleId> = rules.iter().map(|r| r.id.clone()).collect();
    rule_ids.sort();

    let mut ordinary_producers: BTreeMap<RelId, Vec<RuleId>> = BTreeMap::new();
    let mut mask_producers: BTreeMap<RelId, Vec<RuleId>> = BTreeMap::new();
    let mut reads_by_rule: BTreeMap<RuleId, &[ReadSite]> = BTreeMap::new();

    for rf in rules {
        match &rf.produces {
            Produces::Relation(rel) => ordinary_producers
                .entry(rel.clone())
                .or_default()
                .push(rf.id.clone()),
            Produces::Mask { relation } => mask_producers
                .entry(relation.clone())
                .or_default()
                .push(rf.id.clone()),
        }
        reads_by_rule.insert(rf.id.clone(), rf.reads.as_slice());
    }
    for v in ordinary_producers.values_mut() {
        v.sort();
    }
    for v in mask_producers.values_mut() {
        v.sort();
    }

    // Step 1/2: positive + strict edges from ordinary relation reads.
    let mut positive_edges: BTreeSet<(RuleId, RuleId)> = BTreeSet::new();
    let mut strict_edges: BTreeSet<(RuleId, RuleId)> = BTreeSet::new();

    for (consumer_id, sites) in &reads_by_rule {
        for site in *sites {
            if let Some(producers) = ordinary_producers.get(&site.relation) {
                for producer in producers {
                    // A self-referential positive read (ordinary recursion)
                    // is still a real edge — it is what puts the rule in a
                    // nontrivial SCC with itself.
                    let edge = (producer.clone(), consumer_id.clone());
                    if site.strict {
                        strict_edges.insert(edge);
                    } else {
                        positive_edges.insert(edge);
                    }
                }
            }
        }
    }

    // Step 3: mask edges — only relations that have a masker at all.
    for (relation, maskers) in &mask_producers {
        let producers = ordinary_producers
            .get(relation)
            .cloned()
            .unwrap_or_default();
        for p in &producers {
            for m in maskers {
                strict_edges.insert((p.clone(), m.clone()));
            }
        }
        for m in maskers {
            for (consumer_id, sites) in &reads_by_rule {
                for site in *sites {
                    if site.relation != *relation {
                        continue;
                    }
                    if consumer_id == m && site.is_mask_target {
                        continue; // excluded: the mask rule's own target binding
                    }
                    strict_edges.insert((m.clone(), consumer_id.clone()));
                }
            }
        }
    }

    // Step 4 (errata 0002): union co-producers of the same relation into
    // one positive cycle before condensation — see module doc.
    for producer_ids in ordinary_producers.values() {
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

    // Step 5: condense SCCs of positive edges only.
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
    let mut component_of: BTreeMap<RuleId, usize> = BTreeMap::new();
    let mut component_members: Vec<BTreeSet<RuleId>> = Vec::with_capacity(sccs.len());
    for (i, comp) in sccs.iter().enumerate() {
        component_members.push(comp.iter().cloned().collect());
        for r in comp {
            component_of.insert(r.clone(), i);
        }
    }

    // Any strict/mask edge inside one SCC is a direct cycle error, reported
    // with the shortest positive-edge witness path back to `from`.
    for (from, to) in &strict_edges {
        if component_of[from] == component_of[to] {
            let allowed = &component_members[component_of[from]];
            let back = shortest_rule_path(to, from, &positive_adj, allowed);
            let mut path = vec![from.clone()];
            path.extend(back);
            return Err(PhaseError::CycleThroughNonMonotoneEdge {
                from: from.clone(),
                to: to.clone(),
                reason: "strict or mask edge inside one positive-recursion component",
                path,
            });
        }
    }

    // Step 6: condensation DAG — SCC-level edges from every positive or
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
        // A residual cycle exists in the condensation; find the shortest
        // witness cycle through it rather than reporting an arbitrary edge.
        let (&(a, b), (from, to)) = condensed_edge_witness
            .iter()
            .find(|(&(a, b), _)| component_reaches(b, a, &condensed_adj))
            .expect("topo_sort reported a cycle but no condensed edge closes one");
        let comp_path = shortest_component_path(b, a, &condensed_adj);
        let mut path = vec![from.clone(), to.clone()];
        for window in comp_path.windows(2) {
            let (ci, cj) = (window[0], window[1]);
            if let Some((_, w_to)) = condensed_edge_witness.get(&(ci, cj)) {
                if path.last() != Some(w_to) {
                    path.push(w_to.clone());
                }
            }
        }
        if path.last() != Some(from) {
            path.push(from.clone());
        }
        PhaseError::CycleThroughNonMonotoneEdge {
            from: from.clone(),
            to: to.clone(),
            reason: "non-monotone edge closes a cycle across several phase components",
            path,
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

/// Whether `goal` is reachable from `start` in the condensation graph —
/// used to pick, among all condensed edges, one that actually participates
/// in the residual cycle `topo_sort` detected.
fn component_reaches(start: usize, goal: usize, adj: &[BTreeSet<usize>]) -> bool {
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    visited.insert(start);
    queue.push_back(start);
    while let Some(u) = queue.pop_front() {
        if u == goal {
            return true;
        }
        for &v in &adj[u] {
            if visited.insert(v) {
                queue.push_back(v);
            }
        }
    }
    false
}

/// Shortest path `start -> ... -> goal` over component indices in `adj`.
/// Panics if `goal` is unreachable from `start` — callers only invoke this
/// once `component_reaches` has confirmed reachability.
fn shortest_component_path(start: usize, goal: usize, adj: &[BTreeSet<usize>]) -> Vec<usize> {
    let mut prev: BTreeMap<usize, usize> = BTreeMap::new();
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    visited.insert(start);
    queue.push_back(start);
    while let Some(u) = queue.pop_front() {
        if u == goal {
            break;
        }
        for &v in &adj[u] {
            if visited.insert(v) {
                prev.insert(v, u);
                queue.push_back(v);
            }
        }
    }
    let mut path = vec![goal];
    let mut cur = goal;
    while cur != start {
        cur = prev[&cur];
        path.push(cur);
    }
    path.reverse();
    path
}

/// Shortest path `from -> ... -> to` over rule ids in `adj`, restricted to
/// nodes in `allowed`. Panics if `to` is unreachable — callers only invoke
/// this within one SCC, where reachability both ways is guaranteed.
fn shortest_rule_path(
    from: &RuleId,
    to: &RuleId,
    adj: &BTreeMap<RuleId, BTreeSet<RuleId>>,
    allowed: &BTreeSet<RuleId>,
) -> Vec<RuleId> {
    let mut prev: BTreeMap<RuleId, RuleId> = BTreeMap::new();
    let mut visited: BTreeSet<RuleId> = BTreeSet::new();
    let mut queue: VecDeque<RuleId> = VecDeque::new();
    visited.insert(from.clone());
    queue.push_back(from.clone());
    while let Some(u) = queue.pop_front() {
        if &u == to {
            break;
        }
        if let Some(succs) = adj.get(&u) {
            for v in succs {
                if !allowed.contains(v) || visited.contains(v) {
                    continue;
                }
                visited.insert(v.clone());
                prev.insert(v.clone(), u.clone());
                queue.push_back(v.clone());
            }
        }
    }
    let mut path = vec![to.clone()];
    let mut cur = to.clone();
    while &cur != from {
        cur = prev
            .get(&cur)
            .expect("path exists: from/to share one positive-SCC")
            .clone();
        path.push(cur.clone());
    }
    path.reverse();
    path
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
