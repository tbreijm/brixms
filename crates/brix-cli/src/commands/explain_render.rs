//! Rendering for `brix why`/`whynot` structured derivation explanations
//! (ADR-0030): converts `brix_lower::finite_decision`'s `CandidateExplanation`
//! into the CLI's JSON wire shape and into the human `because:` tree appended
//! after the existing `why`/`whynot` output.
//!
//! This module owns all rendering; `why.rs` only calls
//! [`explanation_to_json`] and [`render_explanation_human`] from its output
//! section. Every value it prints was already computed by
//! `brix_lower::finite_decision::explain` from the real evaluator — this
//! module only formats it, and escapes source/value text through the same
//! hostile-text helpers every other human-output path in this crate uses.

use brix_lower::finite_decision::{
    CandidateExplanation, FactExplain, FactOrigin, NodeRef, SelectionComparison, TraceNode,
    TraceOutcome,
};

use crate::commands::{escape_diagnostic_human, fmt_value_human};
use crate::json::{
    to_tagged_value, ExplanationJson, FactExplainJson, FactOriginJson, SelectionJson,
    TraceNodeJson, TraceOutcomeJson,
};

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Convert a [`CandidateExplanation`] into its `brix.cli.result@1` wire shape.
pub fn explanation_to_json(expl: &CandidateExplanation) -> ExplanationJson {
    ExplanationJson {
        candidate: expl.candidate.clone(),
        guard: trace_node_to_json(&expl.guard),
        value: trace_node_to_json(&expl.value),
        facts: expl.facts.iter().map(fact_explain_to_json).collect(),
        selection: expl.selection.as_ref().map(selection_to_json),
        truncated: expl.truncated,
    }
}

fn trace_node_to_json(node: &TraceNode) -> TraceNodeJson {
    TraceNodeJson {
        source: node.source.clone(),
        outcome: trace_outcome_to_json(&node.outcome),
        children: node.children.iter().map(trace_node_to_json).collect(),
    }
}

fn trace_outcome_to_json(outcome: &TraceOutcome) -> TraceOutcomeJson {
    match outcome {
        TraceOutcome::Value(v) => TraceOutcomeJson::Value {
            value: to_tagged_value(v),
        },
        TraceOutcome::NotEvaluated => TraceOutcomeJson::NotEvaluated,
        TraceOutcome::Fault(f) => TraceOutcomeJson::Fault {
            detail: f.to_string(),
        },
        TraceOutcome::Truncated => TraceOutcomeJson::Truncated,
        TraceOutcome::Summarized { shown, total } => TraceOutcomeJson::Summarized {
            shown: *shown,
            total: *total,
        },
    }
}

fn fact_explain_to_json(fact: &FactExplain) -> FactExplainJson {
    FactExplainJson {
        name: fact.name.clone(),
        origin: match &fact.origin {
            FactOrigin::Rule { deps } => FactOriginJson::Rule { deps: deps.clone() },
            FactOrigin::Let => FactOriginJson::Let,
            FactOrigin::Input => FactOriginJson::Input,
        },
        value: to_tagged_value(&fact.value),
        grade: fact.grade.map(|g| format!("{g:?}")),
        trace: fact.trace.as_ref().map(trace_node_to_json),
    }
}

fn selection_to_json(sel: &SelectionComparison) -> SelectionJson {
    SelectionJson {
        candidate: sel.candidate.clone(),
        priority: sel.priority.to_string(),
        is_winner: sel.is_winner,
        winner: sel.winner.clone(),
        winner_priority: sel.winner_priority.to_string(),
        decided_by_tiebreak: sel.decided_by_tiebreak,
        candidate_tiebreak: sel.candidate_tiebreak_hex.clone(),
        winner_tiebreak: sel.winner_tiebreak_hex.clone(),
    }
}

// ---------------------------------------------------------------------------
// Human
// ---------------------------------------------------------------------------

/// Render the indented `because:` tree appended after `why`/`whynot`'s
/// existing human output. Every existing output line is untouched; this is
/// additive text only.
pub fn render_explanation_human(expl: &CandidateExplanation) -> String {
    let mut out = String::new();
    out.push_str("because:\n");

    out.push_str("  guard: ");
    out.push_str(&node_head(&expl.guard));
    out.push('\n');
    for child in &expl.guard.children {
        render_node(&mut out, child, 4);
    }

    if !expl.facts.is_empty() {
        out.push_str("  facts:\n");
        for fact in &expl.facts {
            render_fact(&mut out, fact, 4);
        }
    }

    out.push_str("  value: ");
    out.push_str(&node_head(&expl.value));
    out.push('\n');
    for child in &expl.value.children {
        render_node(&mut out, child, 4);
    }

    if let Some(sel) = &expl.selection {
        out.push_str("  selection: ");
        out.push_str(&render_selection_line(sel));
        out.push('\n');
    }

    if expl.truncated {
        out.push_str("  (truncated: node budget exhausted)\n");
    }

    out
}

/// The one-line "source => value" (or fault/not-evaluated/truncated marker)
/// head of a trace node, with hostile source and fault text escaped for
/// terminal display.
fn node_head(node: &TraceNode) -> String {
    let src = escape_diagnostic_human(&node.source);
    match &node.outcome {
        TraceOutcome::Value(v) => format!("{src} => {}", fmt_value_human(v)),
        TraceOutcome::NotEvaluated => format!("{src}: not evaluated"),
        TraceOutcome::Fault(f) => {
            format!(
                "{src} => fault: {}",
                escape_diagnostic_human(&f.to_string())
            )
        }
        TraceOutcome::Truncated => format!("{src}: truncated"),
        TraceOutcome::Summarized { shown, total } => {
            format!("{src} (showing {shown} of {total} elements)")
        }
    }
}

fn grade_suffix(node_ref: Option<NodeRef>) -> &'static str {
    match node_ref {
        Some(NodeRef::Rule) | Some(NodeRef::Input) => " @Derived",
        Some(NodeRef::Let) | None => "",
    }
}

fn render_node(out: &mut String, node: &TraceNode, indent: usize) {
    out.push_str(&" ".repeat(indent));
    out.push_str(&node_head(node));
    out.push_str(grade_suffix(node.node_ref));
    out.push('\n');
    for child in &node.children {
        render_node(out, child, indent + 2);
    }
}

fn render_fact(out: &mut String, fact: &FactExplain, indent: usize) {
    out.push_str(&" ".repeat(indent));
    let name = escape_diagnostic_human(&fact.name);
    let value = fmt_value_human(&fact.value);
    out.push_str(&format!("{name} => {value}"));
    if fact.grade.is_some() {
        out.push_str(" @Derived");
    }
    if matches!(fact.origin, FactOrigin::Input) {
        out.push_str(" (external input)");
    }
    out.push('\n');
    if let Some(trace) = &fact.trace {
        render_node(out, trace, indent + 2);
    }
}

fn render_selection_line(sel: &SelectionComparison) -> String {
    if sel.is_winner {
        format!(
            "priority {} — least calendar key among admitted candidates",
            sel.priority
        )
    } else {
        let winner = escape_diagnostic_human(&sel.winner);
        let basis = if sel.decided_by_tiebreak {
            format!(
                "canonical tie-break (this: {}, winner: {})",
                sel.candidate_tiebreak_hex, sel.winner_tiebreak_hex
            )
        } else {
            "priority".to_string()
        };
        format!(
            "priority {} vs winner '{winner}' priority {} — winner leads on {basis}",
            sel.priority, sel.winner_priority
        )
    }
}
