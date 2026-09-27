//! `brix kb <op>` — the persistent, revisable knowledge base (ADR-0041).
//!
//! Thin CLI layer over the `brix-kb` library crate: every operation
//! (`init`/`assert`/`retract`/`program`/`log`/`show`/`diff`/`audit`/`verify`)
//! is implemented there; this module only parses nothing further (that is
//! `crate::cli::KbOp`, already parsed), calls into `brix-kb`, and renders the
//! result as human text or `--json`, reusing the same rendering helpers
//! `brix run`/`brix audit` use in `crate::commands`.

use std::path::PathBuf;

use brix_kb::pipeline::ReplayResult;
use brix_kb::revision::{Change, Status};
use brix_kb::{KbError, RevisionRecord};
use brix_lower::finite_decision::FiniteDecisionStop;
use serde_json::json;

use crate::cli::KbOp;
use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS};
use crate::commands::{
    bound_input_to_json, candidate_disposition_to_json, decision_to_json, fact_to_json,
    format_finite_decision_human, unknown_reason_to_code_and_detail,
};

/// The local JSON schema tag for `brix kb` output. Deliberately not
/// `brix.cli.result@1` (`crate::json::CliResultJson`): that schema's field set
/// is fixed to a single decision (`program`/`context`/`facts`/`candidates`/
/// `decision`), and `brix kb log`/`diff` report on more than one.
const KB_JSON_SCHEMA: &str = "brix.cli.kb-result@1";

pub fn execute_kb(op: &KbOp, json_out: bool) -> u8 {
    match op {
        KbOp::Init {
            dir,
            program,
            input_paths,
            package_paths,
        } => run_write_op(
            "kb init",
            dir,
            json_out,
            brix_kb::ops::init(dir, program, input_paths, package_paths),
        ),
        KbOp::Assert {
            dir,
            input_paths,
            package_paths,
        } => run_write_op(
            "kb assert",
            dir,
            json_out,
            brix_kb::ops::assert_inputs(dir, input_paths, package_paths),
        ),
        KbOp::Retract {
            dir,
            names,
            package_paths,
        } => run_write_op(
            "kb retract",
            dir,
            json_out,
            brix_kb::ops::retract_inputs(dir, names, package_paths),
        ),
        KbOp::Program {
            dir,
            program,
            package_paths,
        } => run_write_op(
            "kb program",
            dir,
            json_out,
            brix_kb::ops::set_program(dir, program, package_paths),
        ),
        KbOp::Log { dir, package_paths } => execute_log(dir, package_paths, json_out),
        KbOp::Show {
            dir,
            rev,
            package_paths,
        } => execute_show(dir, *rev, package_paths, json_out),
        KbOp::Diff {
            dir,
            rev_a,
            rev_b,
            package_paths,
        } => execute_diff(dir, *rev_a, *rev_b, package_paths, json_out),
        KbOp::Audit {
            dir,
            rev,
            bundle_out,
            force,
            package_paths,
        } => execute_audit(dir, *rev, bundle_out, *force, package_paths, json_out),
        KbOp::Verify { dir, package_paths } => execute_verify(dir, package_paths, json_out),
    }
}

fn print_error(cmd: &str, err: &KbError, json_out: bool) -> u8 {
    if json_out {
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": cmd,
            "ok": false,
            "status": err.status,
            "diagnostics": [err.diagnostic()],
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        eprintln!("brix {cmd}: {}: {}", err.status, err.message);
    }
    err.exit_code
}

fn change_json(change: &Change) -> serde_json::Value {
    match change {
        Change::Init => json!({"kind": "init"}),
        Change::Assert { names } => json!({"kind": "assert", "names": names}),
        Change::Retract { names } => json!({"kind": "retract", "names": names}),
        Change::Program {
            previous_program_id,
            dropped_inputs,
        } => json!({
            "kind": "program",
            "previous_program_id": previous_program_id.digest().to_hex(),
            "dropped_inputs": dropped_inputs,
        }),
    }
}

fn change_human(change: &Change) -> String {
    match change {
        Change::Init => "init".to_string(),
        Change::Assert { names } => format!("assert({})", names.join(", ")),
        Change::Retract { names } => format!("retract({})", names.join(", ")),
        Change::Program {
            previous_program_id,
            dropped_inputs,
        } => {
            if dropped_inputs.is_empty() {
                format!(
                    "program(from {})",
                    &previous_program_id.digest().to_hex()[..8]
                )
            } else {
                format!(
                    "program(from {}, dropped: {})",
                    &previous_program_id.digest().to_hex()[..8],
                    dropped_inputs.join(", ")
                )
            }
        }
    }
}

fn record_json(record: &RevisionRecord) -> serde_json::Value {
    json!({
        "seq": record.seq,
        "parent": record.parent.map(|d| d.to_hex()),
        "program_id": record.program_id.digest().to_hex(),
        "program_path": record.program_path,
        "snapshot_id": record.snapshot_id.digest().to_hex(),
        "snapshot_path": record.snapshot_path,
        "change": change_json(&record.change),
    })
}

/// The decision-report half of a revision's output, shared by `init`/
/// `assert`/`retract`/`program`/`show`/`log`.
struct DecisionReport {
    status: &'static str,
    human: String,
    json: serde_json::Value,
    /// The candidate name and rendered value, for a one-line summary in `log`.
    decision_summary: Option<String>,
}

fn decision_report(record: &RevisionRecord, replay: &ReplayResult) -> DecisionReport {
    match replay {
        ReplayResult::MissingInputs { missing } => DecisionReport {
            status: "missing-inputs",
            human: format!("status: missing-inputs\nmissing: {}\n", missing.join(", ")),
            json: json!({"status": "missing-inputs", "missing": missing}),
            decision_summary: Some(format!("missing({})", missing.join(", "))),
        },
        ReplayResult::Ran { run, .. } => {
            let context_hex = run.context.digest().to_hex();
            let snapshot_hex = if run.inputs.is_empty() {
                None
            } else {
                Some(record.snapshot_id.digest().to_hex())
            };
            let human =
                format_finite_decision_human(run, Some(&context_hex), snapshot_hex.as_deref());

            let winning = run.decision.as_ref().map(|d| d.candidate.as_str());
            let facts: Vec<_> = run.facts.iter().map(fact_to_json).collect();
            let candidates: Vec<_> = run
                .dispositions
                .iter()
                .map(|d| candidate_disposition_to_json(d, winning))
                .collect();
            let decision = run.decision.as_ref().map(decision_to_json);
            let inputs: Vec<_> = run.inputs.iter().map(bound_input_to_json).collect();
            let (status, diagnostics) = match &run.stop {
                FiniteDecisionStop::Selected(_) => ("selected", Vec::new()),
                FiniteDecisionStop::Quiescent { .. } => ("quiescent", Vec::new()),
                FiniteDecisionStop::Unknown(reason) => {
                    let (code, detail) = unknown_reason_to_code_and_detail(reason);
                    ("unknown", vec![format!("{code}: {detail}")])
                }
            };
            let decision_summary = match &run.stop {
                FiniteDecisionStop::Selected(sel) => Some(format!(
                    "{}={}",
                    sel.candidate,
                    crate::commands::fmt_value_human(&sel.value)
                )),
                FiniteDecisionStop::Quiescent { .. } => Some("quiescent".to_string()),
                FiniteDecisionStop::Unknown(_) => None,
            };
            DecisionReport {
                status,
                human,
                json: json!({
                    "status": status,
                    "context": context_hex,
                    "inputs": inputs,
                    "facts": facts,
                    "candidates": candidates,
                    "decision": decision,
                    "diagnostics": diagnostics,
                }),
                decision_summary,
            }
        }
    }
}

fn is_ok_status(status: &str) -> bool {
    matches!(status, "selected" | "quiescent")
}

fn run_write_op(
    cmd: &str,
    dir: &std::path::Path,
    json_out: bool,
    result: Result<brix_kb::OpOutcome, KbError>,
) -> u8 {
    let outcome = match result {
        Ok(o) => o,
        Err(e) => return print_error(cmd, &e, json_out),
    };
    let report = decision_report(&outcome.record, &outcome.replay);
    let ok = is_ok_status(report.status);

    if json_out {
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": cmd,
            "ok": ok,
            "dir": dir.display().to_string(),
            "revision": record_json(&outcome.record),
            "result": report.json,
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        println!(
            "knowledge base: {}\nrevision: {}\nparent: {}\nchange: {}\nprogram: {}\nsnapshot: {}",
            dir.display(),
            outcome.record.seq,
            outcome
                .record
                .parent
                .map(|d| d.to_hex())
                .unwrap_or_else(|| "none".to_string()),
            change_human(&outcome.record.change),
            outcome.record.program_id.digest().to_hex(),
            outcome.record.snapshot_id.digest().to_hex(),
        );
        print!("{}", report.human);
    }
    if ok {
        EXIT_SUCCESS
    } else {
        EXIT_REJECTED_OR_UNKNOWN
    }
}

fn execute_log(dir: &std::path::Path, package_paths: &[PathBuf], json_out: bool) -> u8 {
    let entries = match brix_kb::ops::log(dir, package_paths) {
        Ok(e) => e,
        Err(e) => return print_error("kb log", &e, json_out),
    };

    if json_out {
        let rows: Vec<serde_json::Value> = entries
            .iter()
            .map(|o| {
                let report = decision_report(&o.record, &o.replay);
                json!({
                    "revision": record_json(&o.record),
                    "status": report.status,
                    "decision": report.decision_summary,
                })
            })
            .collect();
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": "kb log",
            "ok": true,
            "dir": dir.display().to_string(),
            "revisions": rows,
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        for o in &entries {
            let report = decision_report(&o.record, &o.replay);
            let prog_prefix = &o.record.program_id.digest().to_hex()[..8];
            let snap_prefix = &o.record.snapshot_id.digest().to_hex()[..8];
            println!(
                "{}: {}  program={prog_prefix}  snapshot={snap_prefix}  status={}  decision={}",
                o.record.seq,
                change_human(&o.record.change),
                report.status,
                report.decision_summary.as_deref().unwrap_or("none"),
            );
        }
    }
    EXIT_SUCCESS
}

fn execute_show(
    dir: &std::path::Path,
    rev: Option<u64>,
    package_paths: &[PathBuf],
    json_out: bool,
) -> u8 {
    let outcome = match brix_kb::ops::show(dir, rev, package_paths) {
        Ok(o) => o,
        Err(e) => return print_error("kb show", &e, json_out),
    };
    let report = decision_report(&outcome.record, &outcome.replay);

    if json_out {
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": "kb show",
            "ok": true,
            "dir": dir.display().to_string(),
            "revision": record_json(&outcome.record),
            "result": report.json,
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        println!(
            "revision: {}\nparent: {}\nchange: {}\nprogram: {}\nsnapshot: {}",
            outcome.record.seq,
            outcome
                .record
                .parent
                .map(|d| d.to_hex())
                .unwrap_or_else(|| "none".to_string()),
            change_human(&outcome.record.change),
            outcome.record.program_id.digest().to_hex(),
            outcome.record.snapshot_id.digest().to_hex(),
        );
        print!("{}", report.human);
    }
    EXIT_SUCCESS
}

fn execute_diff(
    dir: &std::path::Path,
    rev_a: u64,
    rev_b: u64,
    package_paths: &[PathBuf],
    json_out: bool,
) -> u8 {
    let report = match brix_kb::diff::diff(dir, rev_a, rev_b, package_paths) {
        Ok(r) => r,
        Err(e) => return print_error("kb diff", &e, json_out),
    };
    let render_input =
        |v: &brix_lower::input::InputValue| crate::commands::fmt_value_human(&v.to_l3_value());

    if json_out {
        let inputs_json = |entries: &[brix_kb::diff::InputChange]| -> serde_json::Value {
            json!(entries
                .iter()
                .map(|e| json!({
                    "name": e.name,
                    "old": e.old.as_ref().map(render_input),
                    "new": e.new.as_ref().map(render_input),
                }))
                .collect::<Vec<_>>())
        };
        let facts_json: Vec<_> = report
            .facts_changed
            .iter()
            .map(|f| {
                json!({
                    "name": f.name,
                    "old": crate::commands::fmt_value_human(&f.old),
                    "new": crate::commands::fmt_value_human(&f.new),
                    "why_inputs": f.why_inputs,
                    "why_rules": f.why_rules,
                })
            })
            .collect();
        let candidates_json: Vec<_> = report
            .candidates_changed
            .iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "old_status": c.old_status.to_string(),
                    "new_status": c.new_status.to_string(),
                })
            })
            .collect();
        let decl_json: Vec<_> = report
            .decl_changes
            .iter()
            .map(|d| json!({"kind": d.kind, "name": d.name, "change": d.change}))
            .collect();
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": "kb diff",
            "ok": true,
            "dir": dir.display().to_string(),
            "rev_a": report.rev_a,
            "rev_b": report.rev_b,
            "status_a": report.status_a.as_str(),
            "status_b": report.status_b.as_str(),
            "program_changed": report.program_changed,
            "declarations_changed": decl_json,
            "inputs_added": inputs_json(&report.inputs_added),
            "inputs_removed": inputs_json(&report.inputs_removed),
            "inputs_changed": inputs_json(&report.inputs_changed),
            "facts_changed": facts_json,
            "facts_unchanged_count": report.facts_unchanged_count,
            "facts_added": report.facts_added,
            "facts_removed": report.facts_removed,
            "candidates_changed": candidates_json,
            "decision_a": report.decision_a.as_ref().map(|(c, v)| json!({"candidate": c, "value": crate::commands::fmt_value_human(v)})),
            "decision_b": report.decision_b.as_ref().map(|(c, v)| json!({"candidate": c, "value": crate::commands::fmt_value_human(v)})),
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        println!(
            "diff: revision {} -> revision {}",
            report.rev_a, report.rev_b
        );
        if report.program_changed {
            println!("program: changed");
            for d in &report.decl_changes {
                println!("  {} '{}' {}", d.kind, d.name, d.change);
            }
        } else {
            println!("program: unchanged");
        }
        if !report.inputs_added.is_empty()
            || !report.inputs_removed.is_empty()
            || !report.inputs_changed.is_empty()
        {
            println!("inputs:");
            for e in &report.inputs_added {
                println!("  + {}: {}", e.name, render_input(e.new.as_ref().unwrap()));
            }
            for e in &report.inputs_removed {
                println!("  - {}: {}", e.name, render_input(e.old.as_ref().unwrap()));
            }
            for e in &report.inputs_changed {
                println!(
                    "  ~ {}: {} -> {}",
                    e.name,
                    render_input(e.old.as_ref().unwrap()),
                    render_input(e.new.as_ref().unwrap())
                );
            }
        }
        if !report.facts_changed.is_empty() {
            println!("facts:");
            for f in &report.facts_changed {
                let mut why_parts = Vec::new();
                if !f.why_inputs.is_empty() {
                    why_parts.push(format!("input(s) {}", f.why_inputs.join(", ")));
                }
                if !f.why_rules.is_empty() {
                    why_parts.push(format!("rule(s) {}", f.why_rules.join(", ")));
                }
                let why = if why_parts.is_empty() {
                    String::new()
                } else {
                    format!("  why: {}", why_parts.join("; "))
                };
                println!(
                    "  ~ {}: {} -> {}{why}",
                    f.name,
                    crate::commands::fmt_value_human(&f.old),
                    crate::commands::fmt_value_human(&f.new)
                );
            }
        }
        println!("facts unchanged: {}", report.facts_unchanged_count);
        if !report.facts_added.is_empty() {
            println!("facts added: {}", report.facts_added.join(", "));
        }
        if !report.facts_removed.is_empty() {
            println!("facts removed: {}", report.facts_removed.join(", "));
        }
        if !report.candidates_changed.is_empty() {
            println!("candidates:");
            for c in &report.candidates_changed {
                println!("  ~ {}: {} -> {}", c.name, c.old_status, c.new_status);
            }
        }
        let render_decision = |d: &Option<(String, brix_lower::l3_v2::L3ValueV2)>,
                               status: Status| match d {
            Some((cand, val)) => format!(
                "{cand}={} ({})",
                crate::commands::fmt_value_human(val),
                status.as_str()
            ),
            None => format!("none ({})", status.as_str()),
        };
        println!(
            "decision: {} -> {}",
            render_decision(&report.decision_a, report.status_a),
            render_decision(&report.decision_b, report.status_b),
        );
    }
    EXIT_SUCCESS
}

fn execute_audit(
    dir: &std::path::Path,
    rev: u64,
    bundle_out: &std::path::Path,
    force: bool,
    package_paths: &[PathBuf],
    json_out: bool,
) -> u8 {
    let outcome = match brix_kb::ops::audit_revision(dir, rev, bundle_out, force, package_paths) {
        Ok(o) => o,
        Err(e) => return print_error("kb audit", &e, json_out),
    };
    if json_out {
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": "kb audit",
            "ok": true,
            "dir": dir.display().to_string(),
            "revision": record_json(&outcome.record),
            "status": "audited",
            "artifacts": [{
                "kind": "audit-bundle",
                "path": bundle_out.display().to_string(),
                "bundle_id": outcome.bundle_id_hex,
                "final_chain_digest": outcome.final_chain_hex,
                "count": outcome.receipts_count.to_string(),
            }],
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        println!(
            "revision: {}\nstatus: audited\nbundle: {}\nbundle_id: {}\nfinal_chain: {}\nreceipts: {}",
            outcome.record.seq,
            bundle_out.display(),
            outcome.bundle_id_hex,
            outcome.final_chain_hex,
            outcome.receipts_count,
        );
    }
    EXIT_SUCCESS
}

fn execute_verify(dir: &std::path::Path, package_paths: &[PathBuf], json_out: bool) -> u8 {
    let report = match brix_kb::ops::verify(dir, package_paths) {
        Ok(r) => r,
        Err(e) => return print_error("kb verify", &e, json_out),
    };
    if json_out {
        let res = json!({
            "schema": KB_JSON_SCHEMA,
            "command": "kb verify",
            "ok": true,
            "dir": dir.display().to_string(),
            "status": "verified",
            "revisions_checked": report.revisions_checked,
        });
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        println!(
            "status: verified\nrevisions_checked: {}",
            report.revisions_checked
        );
    }
    EXIT_SUCCESS
}
