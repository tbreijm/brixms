//! Structured, bounded derivation explanations for finite-decision candidates.
//!
//! **Same semantics, no second evaluator.** Every value reported here comes
//! from calling [`crate::l3_v2::eval`] — the real evaluator — on the exact
//! subexpression it names. This module never computes arithmetic,
//! comparison, or `match` results itself; the only "logic" it repeats is the
//! *selection* rule already fixed by the language (which `&&`/`||` operand
//! is short-circuited, which `match` arm is taken), and that rule is applied
//! only to decide which subexpression to walk into next, using a value
//! `eval` itself already produced for the operand or scrutinee in question.
//! So a trace can misrepresent nothing `eval` would not also say, and it can
//! never disagree with [`crate::finite_decision::runtime::FiniteDecisionRuntime::run`]
//! on any value both report.
//!
//! **Bounded.** [`MAX_EXPLAIN_NODES`] caps the total number of [`TraceNode`]s
//! materialized across the whole explanation (guard, value, and every fact
//! trace put together); once exhausted, remaining subtrees are replaced with
//! an explicit [`TraceOutcome::Truncated`] marker rather than cut silently.
//!
//! **Informational only.** Nothing here writes to, or reads mutable state
//! from, a [`crate::finite_decision::runtime::FiniteDecisionRun`]. Building
//! an explanation cannot change a run's result, its program/context/snapshot
//! identity, or any audit bundle or receipt, and it never upgrades a grade:
//! every fact and input it reports keeps exactly the grade the run already
//! assigned it.

use std::collections::BTreeSet;

use brix_semantic::Outcome;
use soc_core::calendar::Key;
use soc_regimes::finite_frontier::{NamedCandidate, WhyExplanation};

use crate::finite_decision::plan::{FiniteDecisionPlan, FiniteDecisionProposal};
use crate::finite_decision::runtime::FiniteDecisionRun;
use crate::l3_v2::{
    eval, ArithOpV2, CmpOpV2, EvalEnv, EvalFault, L3ExprV2, L3PatternV2, L3ValueV2,
};

/// How many elements of a list form's source (or result, for a
/// comprehension) get expanded into their own child trace (ADR-0040). A
/// fold/filter/map/comprehension shows a *bounded summary* rather than one
/// child per element — a `sum` over a `max 4096` derived list would otherwise
/// make its own explanation as large as the list itself, defeating the whole
/// point of a node budget. Every element is still visited by the real
/// evaluator exactly as [`crate::l3_v2::eval`] runs it; this cap only bounds
/// how many of those visits get their own [`TraceNode`] here.
const MAX_LIST_TRACE_ELEMENTS: usize = 5;

/// Maximum number of [`TraceNode`]s materialized in one explanation
/// (guard trace, value trace, and every transitively-read fact's trace,
/// combined). Chosen to comfortably cover any single guard or value
/// expression under the plan's own [`crate::finite_decision::plan::MAX_EXPR_NODES`]
/// bound while still refusing to recurse without limit.
pub const MAX_EXPLAIN_NODES: usize = 512;

/// How many nested helper-function bodies get expanded inline into a trace.
/// A [`L3ExprV2::Call`] node always reports its own arguments and result;
/// only the body *itself* is capped, so a chain of helper calls does not
/// blow up the trace.
const MAX_HELPER_EXPANSION_DEPTH: usize = 1;

/// What a leaf [`TraceNode`] names, when it names a top-level binding at all.
///
/// `None` on every non-leaf node, every literal, and every reference to a
/// local binding (a function parameter or `match`-arm binder) — those are
/// already visible at their call or arm site and are not top-level facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRef {
    /// Reads an already-committed rule fact.
    Rule,
    /// Reads a top-level `let` binding.
    Let,
    /// Reads a bound external input.
    Input,
}

/// The outcome recorded at one [`TraceNode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceOutcome {
    /// The subexpression evaluated to this value.
    Value(L3ValueV2),
    /// The real evaluator never runs this subexpression here: a
    /// short-circuited `&&`/`||` operand, or an untaken `match` arm.
    NotEvaluated,
    /// Evaluating this subexpression faulted.
    Fault(EvalFault),
    /// The node budget was exhausted before this subtree could be built.
    Truncated,
    /// The real evaluator ran every one of `total` elements here (a fold's or
    /// comprehension generator's source list, or a comprehension's result),
    /// but only the first `shown` were expanded into their own child trace
    /// (ADR-0040, [`MAX_LIST_TRACE_ELEMENTS`]) — a deliberate, bounded
    /// summary, not a truncation the node budget forced.
    Summarized { shown: usize, total: usize },
}

/// One node of a bounded evaluation trace: a source-like rendering of a
/// subexpression, its outcome, and its children in evaluation order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceNode {
    /// A source-like rendering of this subexpression. Not guaranteed to
    /// re-parse byte-for-byte (names are the lowered plan's, not the
    /// original source span), but readable Brix-like syntax.
    pub source: String,
    pub outcome: TraceOutcome,
    pub children: Vec<TraceNode>,
    /// Set only on a leaf that names a top-level rule fact, `let` binding,
    /// or external input.
    pub node_ref: Option<NodeRef>,
}

impl TraceNode {
    /// The value this node evaluated to, if it did.
    pub fn value(&self) -> Option<&L3ValueV2> {
        match &self.outcome {
            TraceOutcome::Value(v) => Some(v),
            _ => None,
        }
    }
}

/// Where a fact transitively read by a guard or value expression comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactOrigin {
    /// An already-committed rule, naming the earlier rules it itself reads.
    Rule { deps: Vec<String> },
    /// A top-level `let` binding.
    Let,
    /// A bound external input — a leaf; it has no further trace.
    Input,
}

/// One rule, `let`, or input transitively read while evaluating a guard or
/// value expression, deduplicated by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactExplain {
    pub name: String,
    pub origin: FactOrigin,
    pub value: L3ValueV2,
    /// `None` for a `let` binding, which the outcome lattice does not grade.
    pub grade: Option<Outcome>,
    /// The body's own trace. `None` only for [`FactOrigin::Input`], which is
    /// an external leaf with no body to trace.
    pub trace: Option<TraceNode>,
}

/// The calendar comparison between an admitted candidate and the
/// deliberation's actual winner, restating exactly the [`Key`] ordering
/// [`soc_regimes::finite_frontier::explain_why`] already computed — never a
/// re-derived comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionComparison {
    pub candidate: String,
    pub priority: u64,
    /// Whether `candidate` is itself the winner.
    pub is_winner: bool,
    pub winner: String,
    pub winner_priority: u64,
    /// Whether the two candidates' priorities were equal, so the canonical
    /// tie-break digest (not priority) decided it.
    pub decided_by_tiebreak: bool,
    pub candidate_tiebreak_hex: String,
    pub winner_tiebreak_hex: String,
}

impl SelectionComparison {
    fn winner_of(candidate: &NamedCandidate, key: Key) -> Self {
        Self {
            candidate: candidate.name.clone(),
            priority: candidate.priority,
            is_winner: true,
            winner: candidate.name.clone(),
            winner_priority: candidate.priority,
            decided_by_tiebreak: false,
            candidate_tiebreak_hex: key.tiebreak.to_hex(),
            winner_tiebreak_hex: key.tiebreak.to_hex(),
        }
    }

    fn against(
        candidate: &NamedCandidate,
        key: Key,
        winner: &NamedCandidate,
        winner_key: Key,
    ) -> Self {
        Self {
            candidate: candidate.name.clone(),
            priority: candidate.priority,
            is_winner: false,
            winner: winner.name.clone(),
            winner_priority: winner.priority,
            decided_by_tiebreak: key.priority == winner_key.priority,
            candidate_tiebreak_hex: key.tiebreak.to_hex(),
            winner_tiebreak_hex: winner_key.tiebreak.to_hex(),
        }
    }
}

/// Derive the [`SelectionComparison`] from an already-computed
/// [`WhyExplanation`] — reusing its `Key`s rather than restating the
/// calendar's own ordering. `None` for a candidate that was never admitted
/// (there is no winner to compare against).
pub fn selection_from_why(why: &WhyExplanation) -> Option<SelectionComparison> {
    match why {
        WhyExplanation::Selected { key, candidate } => {
            Some(SelectionComparison::winner_of(candidate, *key))
        }
        WhyExplanation::AdmittedNotSelected {
            key,
            candidate,
            selected_key,
            selected_candidate,
        } => Some(SelectionComparison::against(
            candidate,
            *key,
            selected_candidate,
            *selected_key,
        )),
        WhyExplanation::NotAdmitted { .. }
        | WhyExplanation::CandidateNotFound
        | WhyExplanation::EvaluationFaulted { .. }
        | WhyExplanation::NoCandidateAdmitted => None,
    }
}

/// A full, structured explanation for one candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateExplanation {
    pub candidate: String,
    /// The admission guard's evaluation trace; its root value is exactly the
    /// Bool the runtime's admission decision was made from.
    pub guard: TraceNode,
    /// The proposal's value expression's evaluation trace.
    pub value: TraceNode,
    /// Every rule, `let`, and input transitively read by the guard or value,
    /// deduplicated by name.
    pub facts: Vec<FactExplain>,
    /// The calendar comparison against the actual winner, present only for
    /// an admitted candidate (selected or admitted-not-selected).
    pub selection: Option<SelectionComparison>,
    /// Whether the node budget was exhausted anywhere in this explanation.
    pub truncated: bool,
}

/// The result of asking for a candidate's explanation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplainOutcome {
    Explained(Box<CandidateExplanation>),
    /// No candidate by this name exists in the freshly re-derived pool.
    CandidateNotFound,
}

// ---------------------------------------------------------------------------
// Source-like rendering
// ---------------------------------------------------------------------------

fn render(e: &L3ExprV2) -> String {
    match e {
        L3ExprV2::Int(n) => n.to_string(),
        L3ExprV2::Str(s) => render_str_literal(s),
        L3ExprV2::Bool(b) => b.to_string(),
        L3ExprV2::LetRef(name) | L3ExprV2::RuleFact(name) => name.clone(),
        L3ExprV2::NullaryVariant { variant, .. } => variant.clone(),
        L3ExprV2::Ctor { variant, args, .. } => {
            if args.is_empty() {
                variant.clone()
            } else {
                format!(
                    "{variant}({})",
                    args.iter().map(render).collect::<Vec<_>>().join(", ")
                )
            }
        }
        L3ExprV2::Record {
            nominal_config,
            fields,
        } => {
            let inner = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", render(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{nominal_config} {{ {inner} }}")
        }
        L3ExprV2::Field(base, field) => format!("{}.{field}", render_operand(base)),
        L3ExprV2::Arith(op, a, b) => format!(
            "{} {} {}",
            render_operand(a),
            arith_symbol(*op),
            render_operand(b)
        ),
        L3ExprV2::Cmp(op, a, b) => format!(
            "{} {} {}",
            render_operand(a),
            cmp_symbol(*op),
            render_operand(b)
        ),
        L3ExprV2::And(a, b) => format!("{} && {}", render_operand(a), render_operand(b)),
        L3ExprV2::Or(a, b) => format!("{} || {}", render_operand(a), render_operand(b)),
        L3ExprV2::Not(a) => format!("!{}", render_operand(a)),
        L3ExprV2::IntDivMod(op, a, b) => format!("{}({}, {})", op.name(), render(a), render(b)),
        L3ExprV2::Match { scrutinee, arms } => {
            let arms_src = arms
                .iter()
                .map(|(pat, body)| format!("{} => {}", render_pattern(pat), render(body)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("match {} {{ {} }}", render_operand(scrutinee), arms_src)
        }
        L3ExprV2::NumericBuiltin(op, args) => format!(
            "{}({})",
            op.name(),
            args.iter().map(render).collect::<Vec<_>>().join(", ")
        ),
        L3ExprV2::Call { func, args } => format!(
            "{func}({})",
            args.iter().map(render).collect::<Vec<_>>().join(", ")
        ),
        L3ExprV2::Fold {
            op,
            list,
            binder,
            body,
        } => format!(
            "{}({}, {binder} => {})",
            op.name(),
            render(list),
            render(body)
        ),
        L3ExprV2::Filter { list, binder, cond } => {
            format!("filter({}, {binder} => {})", render(list), render(cond))
        }
        L3ExprV2::Map { list, binder, body } => {
            format!("map({}, {binder} => {})", render(list), render(body))
        }
        L3ExprV2::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            let gens = generators
                .iter()
                .map(|(binder, source)| format!("{binder} in {}", render(source)))
                .collect::<Vec<_>>()
                .join(", ");
            let where_src = where_clause
                .as_ref()
                .map(|w| format!(" where {}", render(w)))
                .unwrap_or_default();
            format!("for {gens}{where_src} yield {}", render(yield_expr))
        }
        L3ExprV2::ListLit(items) => {
            format!(
                "[{}]",
                items.iter().map(render).collect::<Vec<_>>().join(", ")
            )
        }
        L3ExprV2::In(needle, haystack) => {
            format!("{} in {}", render_operand(needle), render_operand(haystack))
        }
        L3ExprV2::Len(list) => format!("len({})", render(list)),
        L3ExprV2::Distinct(list) => format!("distinct({})", render(list)),
    }
}

fn render_operand(e: &L3ExprV2) -> String {
    if needs_parens(e) {
        format!("({})", render(e))
    } else {
        render(e)
    }
}

fn needs_parens(e: &L3ExprV2) -> bool {
    matches!(
        e,
        L3ExprV2::And(..)
            | L3ExprV2::Or(..)
            | L3ExprV2::Not(..)
            | L3ExprV2::Cmp(..)
            | L3ExprV2::Arith(..)
            | L3ExprV2::Match { .. }
            | L3ExprV2::In(..)
    )
}

/// A source-like rendering of a runtime value (ADR-0037, ADR-0040), used only
/// for a bounded-summary element's own trace source (`"o = Order { ... }"`).
/// Deliberately not shared with `crates/brix-cli`'s `fmt_value_human`:
/// `brix-lower` cannot depend on `brix-cli`, and this rendering serves a
/// different purpose (a `TraceNode` source string, never terminal output), so
/// duplicating the small match is simpler than inverting that dependency.
fn render_value(v: &L3ValueV2) -> String {
    match v {
        L3ValueV2::Int(n) => n.to_string(),
        L3ValueV2::F64(n) => format!("f64(\"{n}\")"),
        L3ValueV2::Decimal(n) => format!("decimal(\"{}\")", brix_canon::decimal_format(*n)),
        L3ValueV2::Bool(b) => b.to_string(),
        L3ValueV2::Str(s) => render_str_literal(s),
        L3ValueV2::Ctor { variant, args, .. } => {
            if args.is_empty() {
                variant.clone()
            } else {
                format!(
                    "{variant}({})",
                    args.iter().map(render_value).collect::<Vec<_>>().join(", ")
                )
            }
        }
        L3ValueV2::Record {
            nominal_config,
            fields,
        } => {
            let inner = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", render_value(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{nominal_config} {{ {inner} }}")
        }
        L3ValueV2::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(render_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn render_pattern(p: &L3PatternV2) -> String {
    let L3PatternV2::Ctor { variant, binders } = p;
    if binders.is_empty() {
        variant.clone()
    } else {
        let inner = binders
            .iter()
            .map(|b| b.clone().unwrap_or_else(|| "_".to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{variant}({inner})")
    }
}

fn render_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn arith_symbol(op: ArithOpV2) -> &'static str {
    match op {
        ArithOpV2::Add => "+",
        ArithOpV2::Sub => "-",
        ArithOpV2::Mul => "*",
        ArithOpV2::Div => "/",
    }
}

fn cmp_symbol(op: CmpOpV2) -> &'static str {
    match op {
        CmpOpV2::Lt => "<",
        CmpOpV2::Le => "<=",
        CmpOpV2::Gt => ">",
        CmpOpV2::Ge => ">=",
        CmpOpV2::Eq => "==",
        CmpOpV2::Ne => "!=",
    }
}

// ---------------------------------------------------------------------------
// Trace building
// ---------------------------------------------------------------------------

struct TraceBuilder {
    remaining: usize,
    truncated: bool,
    rule_refs: Vec<String>,
    rule_refs_seen: BTreeSet<String>,
    let_refs: Vec<String>,
    let_refs_seen: BTreeSet<String>,
    input_refs: Vec<String>,
    input_refs_seen: BTreeSet<String>,
}

impl TraceBuilder {
    fn new(max_nodes: usize) -> Self {
        Self {
            remaining: max_nodes,
            truncated: false,
            rule_refs: Vec::new(),
            rule_refs_seen: BTreeSet::new(),
            let_refs: Vec::new(),
            let_refs_seen: BTreeSet::new(),
            input_refs: Vec::new(),
            input_refs_seen: BTreeSet::new(),
        }
    }

    fn note_rule(&mut self, name: &str) {
        if self.rule_refs_seen.insert(name.to_string()) {
            self.rule_refs.push(name.to_string());
        }
    }

    fn note_let(&mut self, name: &str) {
        if self.let_refs_seen.insert(name.to_string()) {
            self.let_refs.push(name.to_string());
        }
    }

    fn note_input(&mut self, name: &str) {
        if self.input_refs_seen.insert(name.to_string()) {
            self.input_refs.push(name.to_string());
        }
    }

    fn eval_top(&self, e: &L3ExprV2, env: &EvalEnv) -> TraceOutcome {
        match eval(e, env) {
            Ok(v) => TraceOutcome::Value(v),
            Err(f) => TraceOutcome::Fault(f),
        }
    }

    /// A node that the real evaluator never runs: a short-circuited
    /// `&&`/`||` operand, or an untaken `match` arm. Never recurses — there
    /// is nothing evaluated underneath it to show.
    fn not_evaluated(&mut self, source: String) -> TraceNode {
        if self.remaining == 0 {
            self.truncated = true;
            return TraceNode {
                source,
                outcome: TraceOutcome::Truncated,
                children: Vec::new(),
                node_ref: None,
            };
        }
        self.remaining -= 1;
        TraceNode {
            source,
            outcome: TraceOutcome::NotEvaluated,
            children: Vec::new(),
            node_ref: None,
        }
    }

    fn leaf(&mut self, e: &L3ExprV2, env: &EvalEnv, node_ref: Option<NodeRef>) -> TraceNode {
        TraceNode {
            source: render(e),
            outcome: self.eval_top(e, env),
            children: Vec::new(),
            node_ref,
        }
    }

    /// Build the trace for `e`, honoring the node budget, `&&`/`||`
    /// short-circuiting, and `match` arm selection — every value along the
    /// way comes from [`eval`], never from re-derived arithmetic or
    /// comparison logic of this module's own.
    fn build(&mut self, e: &L3ExprV2, env: &EvalEnv, helper_depth: usize) -> TraceNode {
        if self.remaining == 0 {
            self.truncated = true;
            return TraceNode {
                source: render(e),
                outcome: TraceOutcome::Truncated,
                children: Vec::new(),
                node_ref: None,
            };
        }
        self.remaining -= 1;

        match e {
            L3ExprV2::Int(_)
            | L3ExprV2::Str(_)
            | L3ExprV2::Bool(_)
            | L3ExprV2::NullaryVariant { .. } => self.leaf(e, env, None),
            L3ExprV2::RuleFact(name) => {
                self.note_rule(name);
                self.leaf(e, env, Some(NodeRef::Rule))
            }
            L3ExprV2::LetRef(name) => {
                let node_ref = if env.is_local(name) {
                    None
                } else if env.is_let_binding(name) {
                    self.note_let(name);
                    Some(NodeRef::Let)
                } else if env.is_bound_input(name) {
                    self.note_input(name);
                    Some(NodeRef::Input)
                } else {
                    None
                };
                self.leaf(e, env, node_ref)
            }
            L3ExprV2::Ctor { args, .. } | L3ExprV2::NumericBuiltin(_, args) => {
                let children: Vec<TraceNode> = args
                    .iter()
                    .map(|a| self.build(a, env, helper_depth))
                    .collect();
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children,
                    node_ref: None,
                }
            }
            L3ExprV2::Record { fields, .. } => {
                let children: Vec<TraceNode> = fields
                    .iter()
                    .map(|(_, v)| self.build(v, env, helper_depth))
                    .collect();
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children,
                    node_ref: None,
                }
            }
            L3ExprV2::Field(base, _) => {
                let child = self.build(base, env, helper_depth);
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![child],
                    node_ref: None,
                }
            }
            L3ExprV2::Arith(_, a, b) | L3ExprV2::Cmp(_, a, b) | L3ExprV2::IntDivMod(_, a, b) => {
                let ca = self.build(a, env, helper_depth);
                let cb = self.build(b, env, helper_depth);
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![ca, cb],
                    node_ref: None,
                }
            }
            L3ExprV2::Not(a) => {
                let ca = self.build(a, env, helper_depth);
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![ca],
                    node_ref: None,
                }
            }
            L3ExprV2::And(a, b) => {
                let ca = self.build(a, env, helper_depth);
                let cb = match &ca.outcome {
                    // `&&` short-circuits only on a `false` left operand —
                    // the exact condition `eval_internal_body`'s `And` case
                    // checks before ever touching `b`.
                    TraceOutcome::Value(L3ValueV2::Bool(true)) => self.build(b, env, helper_depth),
                    _ => self.not_evaluated(render(b)),
                };
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![ca, cb],
                    node_ref: None,
                }
            }
            L3ExprV2::Or(a, b) => {
                let ca = self.build(a, env, helper_depth);
                let cb = match &ca.outcome {
                    TraceOutcome::Value(L3ValueV2::Bool(false)) => self.build(b, env, helper_depth),
                    _ => self.not_evaluated(render(b)),
                };
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![ca, cb],
                    node_ref: None,
                }
            }
            L3ExprV2::Match { scrutinee, arms } => {
                self.build_match(e, scrutinee, arms, env, helper_depth)
            }
            L3ExprV2::Call { func, args } => self.build_call(e, func, args, env, helper_depth),
            L3ExprV2::Fold {
                list, binder, body, ..
            } => self.build_fold(e, list, binder, body, env, helper_depth),
            L3ExprV2::Filter { list, binder, cond } => {
                self.build_sampled_list_form(e, list, binder, cond, env, helper_depth)
            }
            L3ExprV2::Map { list, binder, body } => {
                self.build_sampled_list_form(e, list, binder, body, env, helper_depth)
            }
            L3ExprV2::Comprehension {
                generators,
                where_clause,
                yield_expr,
            } => {
                self.build_comprehension(e, generators, where_clause, yield_expr, env, helper_depth)
            }
            L3ExprV2::ListLit(items) => {
                let children: Vec<TraceNode> = items
                    .iter()
                    .map(|item| self.build(item, env, helper_depth))
                    .collect();
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children,
                    node_ref: None,
                }
            }
            L3ExprV2::In(needle, haystack) => {
                let cn = self.build(needle, env, helper_depth);
                let ch = self.build(haystack, env, helper_depth);
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![cn, ch],
                    node_ref: None,
                }
            }
            L3ExprV2::Len(list) | L3ExprV2::Distinct(list) => {
                let child = self.build(list, env, helper_depth);
                TraceNode {
                    source: render(e),
                    outcome: self.eval_top(e, env),
                    children: vec![child],
                    node_ref: None,
                }
            }
        }
    }

    /// Up to [`MAX_LIST_TRACE_ELEMENTS`] per-element child traces, in source
    /// order, plus a trailing [`TraceOutcome::Summarized`] marker when
    /// `items` holds more than that. Shared by `sum`/`count`/`all`/`any`/
    /// `min`/`max` (ADR-0037, ADR-0040) and `filter`/`map` (ADR-0040): each
    /// binds `binder` to one element and traces `body` under it, exactly the
    /// environment `eval_internal_body`'s own fold/filter/map evaluation
    /// binds.
    fn expand_bounded_elements(
        &mut self,
        items: &[L3ValueV2],
        binder: &str,
        body: &L3ExprV2,
        env: &EvalEnv,
        helper_depth: usize,
    ) -> Vec<TraceNode> {
        let shown = items.len().min(MAX_LIST_TRACE_ELEMENTS);
        let mut children = Vec::with_capacity(shown + 1);
        for item in items.iter().take(shown) {
            let elem_env = env.clone().with_local(binder.to_string(), item.clone());
            let body_trace = self.build(body, &elem_env, helper_depth);
            let outcome = body_trace.outcome.clone();
            children.push(TraceNode {
                source: format!("{binder} = {}", render_value(item)),
                outcome,
                children: vec![body_trace],
                node_ref: None,
            });
        }
        if items.len() > shown {
            children.push(TraceNode {
                source: format!("... ({} more element(s))", items.len() - shown),
                outcome: TraceOutcome::Summarized {
                    shown,
                    total: items.len(),
                },
                children: Vec::new(),
                node_ref: None,
            });
        }
        children
    }

    fn build_fold(
        &mut self,
        e: &L3ExprV2,
        list: &L3ExprV2,
        binder: &str,
        body: &L3ExprV2,
        env: &EvalEnv,
        helper_depth: usize,
    ) -> TraceNode {
        let list_child = self.build(list, env, helper_depth);
        let items = match list_child.value() {
            Some(L3ValueV2::List(items)) => Some(items.clone()),
            _ => None,
        };
        let mut children = vec![list_child];
        if let Some(items) = items {
            children.extend(self.expand_bounded_elements(&items, binder, body, env, helper_depth));
        }
        TraceNode {
            source: render(e),
            outcome: self.eval_top(e, env),
            children,
            node_ref: None,
        }
    }

    /// `filter`/`map` (ADR-0040): same bounded-element shape as
    /// [`Self::build_fold`], over the form's *source* list — for `filter`
    /// this is every element considered, not only the ones kept, so a reader
    /// can see why an early element was dropped as well as why one was kept.
    fn build_sampled_list_form(
        &mut self,
        e: &L3ExprV2,
        list: &L3ExprV2,
        binder: &str,
        body: &L3ExprV2,
        env: &EvalEnv,
        helper_depth: usize,
    ) -> TraceNode {
        self.build_fold(e, list, binder, body, env, helper_depth)
    }

    fn build_comprehension(
        &mut self,
        e: &L3ExprV2,
        generators: &[(String, L3ExprV2)],
        where_clause: &Option<Box<L3ExprV2>>,
        yield_expr: &L3ExprV2,
        env: &EvalEnv,
        helper_depth: usize,
    ) -> TraceNode {
        let _ = where_clause; // named in `render(e)` already.
        let _ = yield_expr;
        let mut children: Vec<TraceNode> = generators
            .iter()
            .map(|(_, source)| self.build(source, env, helper_depth))
            .collect();
        let outcome = self.eval_top(e, env);
        let result_items = match &outcome {
            TraceOutcome::Value(L3ValueV2::List(items)) => Some(items.clone()),
            _ => None,
        };
        if let Some(items) = result_items {
            let shown = items.len().min(MAX_LIST_TRACE_ELEMENTS);
            for (index, item) in items.iter().take(shown).enumerate() {
                children.push(TraceNode {
                    source: format!("[{index}] = {}", render_value(item)),
                    outcome: TraceOutcome::Value(item.clone()),
                    children: Vec::new(),
                    node_ref: None,
                });
            }
            if items.len() > shown {
                children.push(TraceNode {
                    source: format!("... ({} more element(s))", items.len() - shown),
                    outcome: TraceOutcome::Summarized {
                        shown,
                        total: items.len(),
                    },
                    children: Vec::new(),
                    node_ref: None,
                });
            }
        }
        TraceNode {
            source: render(e),
            outcome,
            children,
            node_ref: None,
        }
    }

    fn build_match(
        &mut self,
        e: &L3ExprV2,
        scrutinee: &L3ExprV2,
        arms: &[(L3PatternV2, L3ExprV2)],
        env: &EvalEnv,
        helper_depth: usize,
    ) -> TraceNode {
        let cscrutinee = self.build(scrutinee, env, helper_depth);
        // The exact arm-selection rule `eval_internal_body`'s `Match` case
        // uses: the scrutinee's constructor variant name (or `true`/`false`
        // for a Bool), first arm whose pattern names it wins. Owned, not
        // borrowed, so `cscrutinee` can move into `children` below.
        let (taken_variant, taken_args): (Option<String>, Vec<L3ValueV2>) =
            match &cscrutinee.outcome {
                TraceOutcome::Value(L3ValueV2::Ctor { variant, args, .. }) => {
                    (Some(variant.clone()), args.clone())
                }
                TraceOutcome::Value(L3ValueV2::Bool(b)) => (
                    Some(if *b { "true" } else { "false" }.to_string()),
                    Vec::new(),
                ),
                _ => (None, Vec::new()),
            };

        let mut children = vec![cscrutinee];
        let mut taken = false;
        for (pat, body) in arms {
            let L3PatternV2::Ctor { variant, binders } = pat;
            let arm_source = format!("{} => {}", render_pattern(pat), render(body));
            if !taken && taken_variant.as_deref() == Some(variant.as_str()) {
                taken = true;
                let mut arm_env = env.clone();
                for (binder, val) in binders.iter().zip(taken_args.iter()) {
                    if let Some(name) = binder {
                        arm_env = arm_env.with_local(name.clone(), val.clone());
                    }
                }
                let body_trace = self.build(body, &arm_env, helper_depth);
                let outcome = body_trace.outcome.clone();
                children.push(TraceNode {
                    source: arm_source,
                    outcome,
                    children: vec![body_trace],
                    node_ref: None,
                });
            } else {
                children.push(self.not_evaluated(arm_source));
            }
        }

        TraceNode {
            source: render(e),
            outcome: self.eval_top(e, env),
            children,
            node_ref: None,
        }
    }

    fn build_call(
        &mut self,
        e: &L3ExprV2,
        func: &str,
        args: &[L3ExprV2],
        env: &EvalEnv,
        helper_depth: usize,
    ) -> TraceNode {
        let mut children: Vec<TraceNode> = Vec::with_capacity(args.len() + 1);
        let mut arg_values: Vec<L3ValueV2> = Vec::with_capacity(args.len());
        let mut all_ok = true;
        for a in args {
            let c = self.build(a, env, helper_depth);
            match &c.outcome {
                TraceOutcome::Value(v) => arg_values.push(v.clone()),
                _ => all_ok = false,
            }
            children.push(c);
        }

        let outcome = self.eval_top(e, env);

        // Expand the helper body at most one level: a call reached while
        // already inside an expanded body shows only its own arguments and
        // result, not a further-nested body trace.
        if all_ok && helper_depth < MAX_HELPER_EXPANSION_DEPTH {
            if let Some(def) = env.functions().get(func) {
                if def.params.len() == arg_values.len() {
                    // The exact closed scope `eval_internal_body`'s `Call`
                    // case builds: only parameters bound into locals, with
                    // the same function table and that function's own
                    // schema table — no caller locals, lets, facts, or
                    // inputs leak in.
                    let mut fn_env = EvalEnv::new()
                        .with_functions(env.functions().clone())
                        .with_schemas(def.schemas.clone());
                    for ((param_name, _), val) in def.params.iter().zip(arg_values) {
                        fn_env = fn_env.with_local(param_name.clone(), val);
                    }
                    let body_trace = self.build(&def.body, &fn_env, helper_depth + 1);
                    children.push(body_trace);
                }
            }
        }

        TraceNode {
            source: render(e),
            outcome,
            children,
            node_ref: None,
        }
    }
}

/// Transitively expand every rule, `let`, and input reference collected
/// while building `builder`'s traces so far, sharing the same node budget,
/// and return them deduplicated by name in discovery order.
fn expand_facts(
    plan: &FiniteDecisionPlan,
    run: &FiniteDecisionRun,
    env: &EvalEnv,
    builder: &mut TraceBuilder,
) -> Vec<FactExplain> {
    let mut out = Vec::new();
    // Every top-level name (rule, `let`, input) is unique across all three
    // kinds by construction (`DuplicateItemName` is refused at lowering), so
    // one dedup set is safe regardless of which kind a name later resolves
    // to.
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = Vec::new();
    queue.append(&mut builder.rule_refs);
    queue.append(&mut builder.let_refs);
    queue.append(&mut builder.input_refs);

    let mut i = 0;
    while i < queue.len() {
        let name = queue[i].clone();
        i += 1;
        if !done.insert(name.clone()) {
            continue;
        }

        if let Some(rule) = plan.rules.iter().find(|r| r.name == name) {
            let Some(value) = run
                .facts
                .iter()
                .find(|f| f.rule == name)
                .map(|f| f.value.clone())
            else {
                // The run already succeeded, so every rule committed a
                // fact; this is unreachable on a consistent plan/run pair.
                // Fail closed rather than fabricate a value.
                continue;
            };
            let trace = builder.build(&rule.body, env, 0);
            queue.append(&mut builder.rule_refs);
            queue.append(&mut builder.let_refs);
            queue.append(&mut builder.input_refs);
            out.push(FactExplain {
                name,
                origin: FactOrigin::Rule {
                    deps: rule.depends_on.clone(),
                },
                value,
                grade: Some(Outcome::Derived),
                trace: Some(trace),
            });
        } else if let Some((_, let_expr)) = plan.lets.iter().find(|(n, _)| n == &name) {
            let Ok(value) = eval(let_expr, env) else {
                // Likewise unreachable: `run` already evaluated every `let`
                // cleanly, over this same environment, before it succeeded.
                continue;
            };
            let trace = builder.build(let_expr, env, 0);
            queue.append(&mut builder.rule_refs);
            queue.append(&mut builder.let_refs);
            queue.append(&mut builder.input_refs);
            out.push(FactExplain {
                name,
                origin: FactOrigin::Let,
                value,
                grade: None,
                trace: Some(trace),
            });
        } else if let Some(input) = run.inputs.iter().find(|b| b.name == name) {
            out.push(FactExplain {
                name,
                origin: FactOrigin::Input,
                value: input.value.clone(),
                grade: Some(input.grade),
                trace: None,
            });
        }
        // Otherwise the name resolves to neither a rule, a `let`, nor a
        // bound input, which cannot happen for a plan that lowered
        // successfully; skip rather than fabricate an entry.
    }
    out
}

/// Build the structured explanation for `target_name`'s guard and value,
/// against the shared evaluation environment `env` — exactly the one the
/// real `run()` folded every input, `let`, and rule fact into, so every
/// value this reports is the run's own value, re-affirmed by re-evaluating
/// its own subexpression, never re-derived by different logic.
///
/// `selection` should come from [`selection_from_why`] (or the equivalent
/// `whynot` framing), reusing the runtime's own calendar comparison rather
/// than restating it.
pub fn explain_candidate(
    plan: &FiniteDecisionPlan,
    run: &FiniteDecisionRun,
    env: &EvalEnv,
    target_name: &str,
    selection: Option<SelectionComparison>,
) -> ExplainOutcome {
    let Some(proposal): Option<&FiniteDecisionProposal> = plan.find_proposal(target_name) else {
        return ExplainOutcome::CandidateNotFound;
    };

    let mut builder = TraceBuilder::new(MAX_EXPLAIN_NODES);
    let guard = builder.build(&proposal.guard, env, 0);
    let value = builder.build(&proposal.value, env, 0);
    let facts = expand_facts(plan, run, env, &mut builder);

    ExplainOutcome::Explained(Box::new(CandidateExplanation {
        candidate: proposal.name.clone(),
        guard,
        value,
        facts,
        selection,
        truncated: builder.truncated,
    }))
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::l3_v2::DivModOpV2;

    #[test]
    fn test_render_binary_and_call_forms() {
        let e = L3ExprV2::And(
            Box::new(L3ExprV2::Cmp(
                CmpOpV2::Ge,
                Box::new(L3ExprV2::RuleFact("stock".to_string())),
                Box::new(L3ExprV2::Int(50)),
            )),
            Box::new(L3ExprV2::Call {
                func: "eligible".to_string(),
                args: vec![L3ExprV2::LetRef("order".to_string())],
            }),
        );
        assert_eq!(render(&e), "(stock >= 50) && eligible(order)");
    }

    #[test]
    fn test_render_not_and_match() {
        let e = L3ExprV2::Not(Box::new(L3ExprV2::And(
            Box::new(L3ExprV2::Bool(true)),
            Box::new(L3ExprV2::Bool(false)),
        )));
        assert_eq!(render(&e), "!(true && false)");

        let m = L3ExprV2::Match {
            scrutinee: Box::new(L3ExprV2::Bool(true)),
            arms: vec![
                (
                    L3PatternV2::Ctor {
                        variant: "true".to_string(),
                        binders: vec![],
                    },
                    L3ExprV2::Int(1),
                ),
                (
                    L3PatternV2::Ctor {
                        variant: "false".to_string(),
                        binders: vec![],
                    },
                    L3ExprV2::Int(0),
                ),
            ],
        };
        assert_eq!(render(&m), "match true { true => 1, false => 0 }");
    }

    #[test]
    fn test_render_div_mod_and_string_literal() {
        let e = L3ExprV2::IntDivMod(
            DivModOpV2::DivFloor,
            Box::new(L3ExprV2::Int(7)),
            Box::new(L3ExprV2::Int(2)),
        );
        assert_eq!(render(&e), "div_floor(7, 2)");
        assert_eq!(DivModOpV2::ModEuclid.name(), "mod_euclid");

        assert_eq!(render_str_literal("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
