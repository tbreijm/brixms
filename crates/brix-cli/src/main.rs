//! `brix` — the Brix command-line driver (ADR-0010).
//!
//! `brix check <file.brix>`: parse a `.brix` program, lower it, and type-check
//! each `let` binding, printing its inferred type and epistemic grade
//! (`@Derived`/`@Audited`/`@Proven`). This is the first runnable Brix tool: it
//! exposes the L1 parser + L2 lowering as a command. Bindings outside the
//! current lowering fragment are reported honestly as not-yet-supported rather
//! than failing the whole file.
//!
//! `brix run <file.brix>`: lower the module's `config`/`let`/`rule` fragment
//! to an ADR-0012 L3 settlement plan and drive it to its ADR-0014 saturated
//! stop. Per ADR-0012 §7, a completed run reports **certified quiescence at a
//! world under a policy** — not a fixpoint of the program, and not a claim
//! about any other policy or revision. The output discipline is normative
//! (ADR-0012 §9 Stage C, final paragraph): every post-plan outcome prints the
//! program, context, presentation, observation profile, semantic run,
//! journal-chain, and ordered `Derived` step identities, plus exactly one
//! §5 status name; `Rejected` prints only deterministic plan diagnostics and
//! mints no identifier. "Quiescent" is only ever printed once this CLI's own
//! call to `check_quiescence_certificate` returns `Verified` — holding a
//! `SaturatedStop::Quiescent` is not sufficient — and is qualified whenever
//! `agenda_residue > 0`. "fixpoint", "settled", `Audited`, and `Proven` are
//! never printed for a settlement outcome.
//!
//! `brix audit <file.brix>`: runs the plan, then audits the resulting journal
//! (ADR-0012 §5, §9 Stage D). Per ADR-0002 §4.1 and ADR-0012 §7, auditing is a
//! separate authority from settlement: it never upgrades the settlement
//! outcome or the quiescence certificate, and only `AuditResult::Audited`
//! yields an `Audited` judgement — `AuditResult::Unknown` is reported as
//! unknown audit status, never as a pass or a failure of the underlying run.
//!
//! Out of scope here (L4, needs elaboration/explanation machinery that does
//! not exist yet): `brix prove`, `brix why`, `brix whynot`, `brix test`,
//! `brix sim`, and any REPL (issue #43).

use std::process::ExitCode;
use std::rc::Rc;

use brix_lower::{
    audit_l3_run, l3_adm, lower_l3_plan, run_l3_plan_with_interner, settlement_run_id, L3AdmChoice,
    L3PlanItem, L3PlanV1, L3Regime, L3RunReport, L3UnknownReasonV1, PlanLimitsV1, SettlementStopV1,
    L3_PROFILE_MARKER_V1,
};
use brix_lower::{check_module, CoverageOutcome};
use brix_syntax::parse;
use soc_core::{
    check_quiescence_certificate, Adm, AuditResult, DeclaredAssumptions, Interner, PresentationV1,
    QuiescenceCertificateId, SaturationBudget, SettlementRegime,
};
use soc_regimes::type_realization::Ty;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check") => {
            run_file_command(args.get(2), "usage: brix check <file.brix>", check_report)
        }
        Some("run") => run_file_command(args.get(2), "usage: brix run <file.brix>", run_report),
        Some("audit") => {
            run_file_command(args.get(2), "usage: brix audit <file.brix>", audit_report)
        }
        _ => {
            eprintln!("usage: brix {{check|run|audit}} <file.brix>");
            ExitCode::FAILURE
        }
    }
}

/// Read `path`, hand its contents to `f`, print the report, and translate
/// `f`'s `had_error` flag into a process exit status. Shared by all three
/// subcommands so each one only has to supply its own `&str -> (String,
/// bool)` report builder.
fn run_file_command(
    path: Option<&String>,
    usage: &str,
    f: impl Fn(&str) -> (String, bool),
) -> ExitCode {
    match path {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(src) => {
                let (report, had_error) = f(&src);
                print!("{report}");
                exit_code(had_error)
            }
            Err(e) => {
                eprintln!("brix: cannot read {path}: {e}");
                ExitCode::FAILURE
            }
        },
        None => {
            eprintln!("{usage}");
            ExitCode::FAILURE
        }
    }
}

fn exit_code(had_error: bool) -> ExitCode {
    if had_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Parse + lower + type-check `source`, returning a human-readable report and
/// whether any binding failed (parse error, or a binding that did not lower /
/// type-check / prove). Separated from `main` so it can be unit-tested.
fn check_report(source: &str) -> (String, bool) {
    let module = match parse(source) {
        Ok(m) => m,
        Err(e) => return (format!("parse error: {e}\n"), true),
    };

    let results = check_module(&module);
    if results.is_empty() {
        return ("(no `let` bindings to check)\n".to_string(), false);
    }

    let mut out = String::new();
    let mut had_error = false;
    for r in &results {
        match r {
            Ok(cr) => {
                let coverage = match &cr.coverage {
                    Some(CoverageOutcome::Proven) => "  [coverage: exhaustive @Proven]",
                    Some(CoverageOutcome::Unknown(_)) => "  [coverage: not certified]",
                    None => "",
                };
                out.push_str(&format!(
                    "  {} : {} @{:?}{coverage}\n",
                    cr.name,
                    fmt_ty(cr.ty.as_ref()),
                    cr.outcome
                ));
            }
            Err((name, err)) => {
                had_error = true;
                out.push_str(&format!("  {name} : — (not checked: {err:?})\n"));
            }
        }
    }
    (out, had_error)
}

/// Render an inferred type for display.
fn fmt_ty(ty: Option<&Ty>) -> String {
    match ty {
        None => "?".to_string(),
        Some(Ty::Con(name)) => name.to_string(),
        Some(Ty::Var(v)) => format!("?{v}"),
        Some(Ty::Fn(a, b)) => format!("({} -> {})", fmt_ty(Some(a)), fmt_ty(Some(b))),
        Some(Ty::Record(fields)) => {
            let elems: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", fmt_ty(Some(v))))
                .collect();
            format!("{{{}}}", elems.join(", "))
        }
        Some(Ty::Sum(name, _)) => name.clone(),
    }
}

// ---------------------------------------------------------------------------
// `brix run` / `brix audit` — ADR-0012 §9 Stage C's normative output
// discipline.
// ---------------------------------------------------------------------------

/// Parse and lower `source` into an L3 plan, or a deterministic rejection
/// diagnostic (ADR-0012 §5: `Rejected` is pre-run and mints no identifier —
/// this `Err` string is exactly, and only, that diagnostic text; no program,
/// context, run, or any other semantic identifier is ever fabricated here
/// because none was ever minted).
///
/// `PlanLimitsV1::generous()`: this CLI has no flag surface yet (no clap, per
/// the hand-rolled-args house style) to let a caller tune `max_selected_rules`
/// etc., so it accepts anything the fragment itself admits rather than
/// silently imposing an arbitrary undocumented ceiling.
fn lower_or_reject(source: &str) -> Result<L3PlanV1, String> {
    let module = parse(source).map_err(|e| format!("rejected: parse error: {e}\n"))?;
    lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .map_err(|e| format!("rejected: {e:?}\n"))
}

/// Count the plan's `rule` items (ADR-0012 §3.3: each is a one-shot
/// settlement proposal, so this bounds the number of commits any run of this
/// plan can ever make).
fn rule_count(plan: &L3PlanV1) -> u64 {
    plan.items
        .iter()
        .filter(|item| matches!(item, L3PlanItem::Rule { .. }))
        .count() as u64
}

/// A budget sufficient to drive `plan` all the way to its certified
/// quiescence. `soc_core::saturate::run_saturated`'s loop checks
/// `visible_steps >= budget.max_visible_steps` *before* each call, so an
/// N-rule plan needs one call per rule (each committing) plus one further
/// call that finds the frontier empty and mints the certificate — `N + 1`,
/// not `N`. ADR-0012 §5/⟨D-LIM⟩: the budget is excluded from every canonical
/// identity, so choosing it this way (rather than one fixed constant) cannot
/// change any run's identity, only whether it reaches a decided stop.
fn sufficient_budget(plan: &L3PlanV1) -> SaturationBudget {
    SaturationBudget::uniform(rule_count(plan) + 1)
}

/// Drive one L3 run of `plan` over a fresh, caller-retained [`Interner`].
/// The interner must outlive the run: independently re-checking the returned
/// certificate (ADR-0012 §9 Stage C fixture 12) needs the very same interner
/// that minted its handles, which [`L3RunReport`] does not carry back out on
/// its own (`brix_lower::run_l3_plan`'s convenience wrapper drops it).
fn run_with(
    plan: &L3PlanV1,
    adm_choice: L3AdmChoice<'_>,
    budget: SaturationBudget,
) -> (Interner, L3RunReport) {
    let mut interner = Interner::new();
    let report = run_l3_plan_with_interner(&mut interner, plan, adm_choice, budget);
    (interner, report)
}

/// Independently re-derive a `Quiescent` stop's certificate (ADR-0012 §9
/// Stage C, final paragraph: "MAY print 'quiescent' **only** when it holds a
/// `Quiescent` whose certificate its own checker verified" — holding a
/// `SaturatedStop::Quiescent` is not sufficient). Rebuilds the exact
/// presentation [`run_l3_plan_with_interner`] used, over the caller-supplied
/// `adm` (the production path's `adm` is a pure re-derivation from the
/// report's own transition table via [`l3_adm`]; test-only override policies
/// pass their own `adm` back in here so the check is against the policy the
/// run actually used, never a different one).
fn verify_quiescence(
    interner: &Interner,
    report: &L3RunReport,
    adm: &dyn Adm,
) -> Option<QuiescenceCertificateId> {
    let certificate = report.quiescence_certificate.as_ref()?;
    let regime = L3Regime::new(Rc::clone(&report.table));
    let regimes: [&dyn SettlementRegime; 1] = [&regime];
    let presentation = PresentationV1 {
        id: report.run.presentation,
        regimes: &regimes,
        regime_set: report.regime_set,
        adm,
        adm_id: report.adm_id,
        profile: &report.observation_profile,
        interner,
        context: report.context,
        assumptions: DeclaredAssumptions::all(),
    };
    // This profile's administrative prefix is always empty (𝒢_τ = ∅,
    // ADR-0012 §4.1) — there is never a hidden step to supply.
    check_quiescence_certificate(certificate, &presentation, &report.final_config, &[])
        .verified_id()
}

/// Stable, CLI-owned slugs for [`L3UnknownReasonV1`] (ADR-0012 §5: "`Unknown`
/// additionally carries a stable versioned reason code, while human
/// diagnostic text remains outside the semantic identity" — this mapping is
/// exactly that diagnostic text, owned here rather than reusing any internal
/// `Debug` rendering).
fn unknown_reason_slug(reason: L3UnknownReasonV1) -> &'static str {
    match reason {
        L3UnknownReasonV1::AdministrativeBudgetExhausted => "administrative-budget-exhausted",
        L3UnknownReasonV1::AdministrativeStateBudgetExhausted => {
            "administrative-state-budget-exhausted"
        }
        L3UnknownReasonV1::VisibleBudgetExhausted => "visible-budget-exhausted",
        L3UnknownReasonV1::ProfileError => "profile-error",
        L3UnknownReasonV1::CommitFailed => "commit-failed",
        L3UnknownReasonV1::UndeclaredAssumption => "undeclared-assumption",
        L3UnknownReasonV1::AssumptionViolated => "assumption-violated",
        L3UnknownReasonV1::KeyConflict => "key-conflict",
        L3UnknownReasonV1::AdapterIntegrityFailure => "adapter-integrity-failure",
        L3UnknownReasonV1::DivergenceObserved => "divergence-observed",
    }
}

/// Build the normative post-plan output lines (ADR-0012 §9 Stage C, final
/// paragraph) for one `L3RunReport`: program, context, presentation,
/// observation profile, semantic run, journal-chain, ordered `Derived` step
/// identities, and exactly one status line. Returns `(lines, had_error)`.
fn format_settlement(
    interner: &Interner,
    report: &L3RunReport,
    adm: &dyn Adm,
) -> (Vec<String>, bool) {
    let mut lines = vec![
        format!("program: {}", report.run.program.to_hex()),
        format!("context: {}", report.run.context.to_hex()),
        format!(
            "presentation: {}",
            report.run.presentation.digest().to_hex()
        ),
        format!(
            "observation-profile: {}",
            report.run.observation_profile.to_hex()
        ),
        format!("run: {}", settlement_run_id(&report.run).digest().to_hex()),
        format!("journal-chain: {}", report.run.chain_digest.to_hex()),
    ];
    for (i, step) in report.journal.steps().iter().enumerate() {
        lines.push(format!(
            "step[{i}]: {}",
            step.observation.judgement_digest.to_hex()
        ));
    }

    let (status, certificate_line, had_error) = match &report.run.stop {
        SettlementStopV1::Quiescent { .. } => match verify_quiescence(interner, report, adm) {
            Some(certificate_id) => {
                let status = if report.agenda_residue > 0 {
                    format!(
                        "quiescent under this policy, with {} rule{} never admitted",
                        report.agenda_residue,
                        if report.agenda_residue == 1 { "" } else { "s" }
                    )
                } else {
                    "quiescent".to_string()
                };
                (
                    status,
                    Some(format!("certificate: {}", certificate_id.digest().to_hex())),
                    false,
                )
            }
            // The runner claimed Quiescent, but this CLI's own independent
            // check could not re-derive it. Per ADR-0012 §6 invariant 6, a
            // certificate that fails its checker is Unknown, never a
            // downgraded pass — never printed as quiescent.
            None => ("unknown (certificate-unverified)".to_string(), None, true),
        },
        SettlementStopV1::Unknown { reason } => (
            format!("unknown ({})", unknown_reason_slug(*reason)),
            None,
            true,
        ),
    };
    if let Some(certificate_line) = certificate_line {
        lines.push(certificate_line);
    }
    lines.push(format!("status: {status}"));

    (lines, had_error)
}

fn lines_to_string(lines: Vec<String>) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `brix run <file.brix>`: lower and drive one L3 settlement run, printing
/// ADR-0012 §9 Stage C's normative fields. This is the CLI's only production
/// admissibility path: [`L3AdmChoice::Compiled`] — ADR-0012 §3.4, "there is
/// no surface policy language in v1."
fn run_report(source: &str) -> (String, bool) {
    match lower_or_reject(source) {
        Err(diagnostic) => (diagnostic, true),
        Ok(plan) => {
            let budget = sufficient_budget(&plan);
            let (interner, report) = run_with(&plan, L3AdmChoice::Compiled, budget);
            let adm = l3_adm(&report.table);
            let (lines, had_error) = format_settlement(&interner, &report, &adm);
            (lines_to_string(lines), had_error)
        }
    }
}

/// `brix audit <file.brix>`: run the plan, then audit the resulting journal
/// (ADR-0012 §5, §9 Stage D). Per ADR-0012 §7/ADR-0002 §4.1, auditing never
/// upgrades the settlement outcome or the quiescence certificate — the
/// "status:" line above is produced by exactly [`format_settlement`], and the
/// audit result is reported as its own, separately labelled judgement.
fn audit_report(source: &str) -> (String, bool) {
    match lower_or_reject(source) {
        Err(diagnostic) => (diagnostic, true),
        Ok(plan) => {
            let budget = sufficient_budget(&plan);
            let (interner, report) = run_with(&plan, L3AdmChoice::Compiled, budget);
            let adm = l3_adm(&report.table);
            let (mut lines, mut had_error) = format_settlement(&interner, &report, &adm);

            // Only `AuditResult::Audited` is an `Audited` judgement;
            // `AuditResult::Unknown` is reported as unknown audit status,
            // never as a pass or a failure of the underlying run (ADR-0012
            // §5).
            let audit_results = audit_l3_run(&report);
            let mut unknown_count = 0usize;
            for (i, result) in audit_results.iter().enumerate() {
                match result {
                    AuditResult::Audited(audited) => {
                        lines.push(format!(
                            "audit[{i}]: audited {}",
                            audited.audited_id.to_hex()
                        ));
                    }
                    AuditResult::Unknown(reason) => {
                        unknown_count += 1;
                        lines.push(format!("audit[{i}]: unknown ({reason})"));
                    }
                }
            }
            if unknown_count > 0 {
                had_error = true;
                lines.push(format!(
                    "audit: unknown ({unknown_count} of {} steps unknown)",
                    audit_results.len()
                ));
            } else {
                lines.push("audit: audited".to_string());
            }

            (lines_to_string(lines), had_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use brix_canon::{Digest, Domain};
    use brix_lower::L3LowerError;
    use soc_core::AdmNone;

    #[test]
    fn checks_identity_application_to_proven_int() {
        // The pure λ-calculus core (var/λ/app) is discharged, so `id(42)` earns Proven.
        let src = "fn id(n) = n\nlet r = id(42)\n";
        let (report, had_error) = check_report(src);
        assert!(!had_error, "report: {report}");
        assert!(
            report.contains("r : Int @Proven"),
            "expected `r : Int @Proven`, got:\n{report}"
        );
    }

    #[test]
    fn checks_literal_to_proven_int() {
        // A pure literal rests only on the discharged (tight) `g_lit`, so it
        // honestly earns Proven.
        let (report, had_error) = check_report("let x = 42\n");
        assert!(!had_error);
        assert!(report.contains("x : Int @Proven"), "{report}");
    }

    #[test]
    fn checks_record_and_field_access_to_proven() {
        let src = "let p = Item { x: 1, y: 2 }\nlet a = p.x\n";
        let (report, had_error) = check_report(src);
        assert!(!had_error, "report: {report}");
        assert!(
            report.contains("p : {x: Int, y: Int} @Proven"),
            "expected `p : {{x: Int, y: Int}} @Proven`, got:\n{report}"
        );
        assert!(
            report.contains("a : Int @Proven"),
            "expected `a : Int @Proven`, got:\n{report}"
        );
    }

    #[test]
    fn unsupported_binding_is_reported_not_crashed() {
        let (report, had_error) = check_report("let y = 1 then 2\n");
        assert!(had_error);
        assert!(report.contains("y : — (not checked"), "{report}");
    }

    #[test]
    fn parse_error_is_reported() {
        let (report, had_error) = check_report("let = 42\n");
        assert!(had_error);
        assert!(report.contains("parse error"), "{report}");
    }

    // -----------------------------------------------------------------
    // `brix run` — ADR-0012 §9 Stage C's normative output discipline.
    // -----------------------------------------------------------------

    #[test]
    fn run_two_rules_reaches_verified_quiescence() {
        let (report, had_error) = run_report("rule a() = 1\nrule b() = 2\n");
        assert!(!had_error, "report: {report}");
        assert!(report.contains("program: "), "{report}");
        assert!(report.contains("context: "), "{report}");
        assert!(report.contains("presentation: "), "{report}");
        assert!(report.contains("observation-profile: "), "{report}");
        assert!(report.contains("run: "), "{report}");
        assert!(report.contains("journal-chain: "), "{report}");
        assert!(report.contains("step[0]: "), "{report}");
        assert!(report.contains("step[1]: "), "{report}");
        assert!(!report.contains("step[2]: "), "exactly two steps: {report}");
        assert!(report.contains("certificate: "), "{report}");
        assert!(report.contains("status: quiescent\n"), "{report}");
    }

    #[test]
    fn run_empty_module_reaches_verified_quiescence_with_empty_journal() {
        let (report, had_error) = run_report("");
        assert!(!had_error, "report: {report}");
        assert!(!report.contains("step["), "no rules, no steps: {report}");
        assert!(report.contains("status: quiescent\n"), "{report}");
    }

    #[test]
    fn run_is_byte_identical_across_repeated_invocations() {
        let src = "rule a() = 1\nrule b() = 2\nrule c() = 3\n";
        let (first, had_error_1) = run_report(src);
        let (second, had_error_2) = run_report(src);
        assert_eq!(had_error_1, had_error_2);
        assert_eq!(first, second, "repeated runs must be byte-identical");
    }

    #[test]
    fn run_denying_policy_is_quiescent_but_qualified_and_never_says_complete() {
        // ADR-0012 §3.4: "There is no surface policy language in v1" — a
        // denying policy can only be injected by a test, exactly as the
        // Stage C fixture 5 test harness in `brix_lower::l3_run` does. This
        // exercises `format_settlement`'s residue-qualification path
        // directly, over an `L3AdmChoice::Override`, bypassing the
        // Compiled-only production entry point `run_report`.
        let module = brix_syntax::parse("rule a() = 1\nrule b() = 2\n").unwrap();
        let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .expect("lowers");
        let adm_id = Digest::of(Domain::Value, b"brix-cli-test.deny-all@1");
        let budget = sufficient_budget(&plan);
        let (interner, report) = run_with(
            &plan,
            L3AdmChoice::Override {
                adm: &AdmNone,
                adm_id,
            },
            budget,
        );
        assert_eq!(report.agenda_residue, 2, "both rules denied");
        let (lines, had_error) = format_settlement(&interner, &report, &AdmNone);
        assert!(!had_error, "a denied agenda is still genuine quiescence");
        let report_text = lines_to_string(lines);
        assert!(
            report_text
                .contains("status: quiescent under this policy, with 2 rules never admitted"),
            "{report_text}"
        );
        assert!(
            !report_text.contains("complete"),
            "must never say complete: {report_text}"
        );
        assert!(report_text.contains("certificate: "), "{report_text}");
    }

    #[test]
    fn run_budget_exhausted_is_unknown_with_no_certificate() {
        let module = brix_syntax::parse("rule a() = 1\nrule b() = 2\n").unwrap();
        let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .expect("lowers");
        // One less than `sufficient_budget` would supply: enough for one
        // commit, not enough to ever observe the empty frontier.
        let budget = SaturationBudget::uniform(1);
        let (interner, report) = run_with(&plan, L3AdmChoice::Compiled, budget);
        let adm = l3_adm(&report.table);
        let (lines, had_error) = format_settlement(&interner, &report, &adm);
        assert!(had_error, "budget exhaustion must be an error exit");
        let report_text = lines_to_string(lines);
        assert!(
            report_text.contains("status: unknown (visible-budget-exhausted)"),
            "{report_text}"
        );
        assert!(
            !report_text.contains("certificate:"),
            "no certificate of any kind on budget exhaustion: {report_text}"
        );
    }

    #[test]
    fn run_rejects_unsupported_module_with_no_semantic_identifier() {
        let (report, had_error) = run_report("fn id(n) = n\n");
        assert!(had_error);
        assert!(report.starts_with("rejected:"), "{report}");
        for forbidden in [
            "program:",
            "context:",
            "run:",
            "presentation:",
            "journal-chain:",
        ] {
            assert!(
                !report.contains(forbidden),
                "rejected output must mint no semantic identifier, found {forbidden:?} in: {report}"
            );
        }
    }

    #[test]
    fn run_never_prints_forbidden_settlement_words() {
        let (quiescent, _) = run_report("rule a() = 1\n");
        let (rejected, _) = run_report("fn id(n) = n\n");
        let module = brix_syntax::parse("rule a() = 1\nrule b() = 2\n").unwrap();
        let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .expect("lowers");
        let (interner, report) =
            run_with(&plan, L3AdmChoice::Compiled, SaturationBudget::uniform(1));
        let adm = l3_adm(&report.table);
        let (unknown_lines, _) = format_settlement(&interner, &report, &adm);
        let unknown_text = lines_to_string(unknown_lines);

        for text in [&quiescent, &rejected, &unknown_text] {
            for forbidden in ["fixpoint", "settled", "Audited", "Proven", "complete"] {
                assert!(
                    !text.contains(forbidden),
                    "settlement output must never contain {forbidden:?}: {text}"
                );
            }
        }
    }

    // -----------------------------------------------------------------
    // `brix audit` — ADR-0012 §5, §9 Stage D.
    // -----------------------------------------------------------------

    #[test]
    fn audit_two_rules_is_audited_and_does_not_change_settlement_status() {
        let (report, had_error) = audit_report("rule a() = 1\nrule b() = 2\n");
        assert!(!had_error, "report: {report}");
        assert!(report.contains("status: quiescent\n"), "{report}");
        assert!(report.contains("audit[0]: audited "), "{report}");
        assert!(report.contains("audit[1]: audited "), "{report}");
        assert!(report.contains("audit: audited\n"), "{report}");
    }

    #[test]
    fn audit_word_appears_only_in_audit_output_never_in_run_output() {
        let (run, _) = run_report("rule a() = 1\nrule b() = 2\n");
        assert!(
            !run.to_lowercase().contains("audit"),
            "brix run must never mention audit: {run}"
        );
        let (audit, _) = audit_report("rule a() = 1\nrule b() = 2\n");
        assert!(audit.contains("audited"), "{audit}");
        assert!(
            !audit.contains("Audited"),
            "capital-A Audited is a judgement outcome name, not this CLI's own text: {audit}"
        );
    }

    #[test]
    fn audit_rejects_unsupported_module_with_no_semantic_identifier() {
        let (report, had_error) = audit_report("fn id(n) = n\n");
        assert!(had_error);
        assert!(report.starts_with("rejected:"), "{report}");
    }

    #[test]
    fn audit_is_byte_identical_across_repeated_invocations() {
        let src = "rule a() = 1\nrule b() = 2\n";
        let (first, _) = audit_report(src);
        let (second, _) = audit_report(src);
        assert_eq!(first, second, "repeated audits must be byte-identical");
    }

    #[test]
    fn unknown_reason_slug_is_stable_and_total() {
        // Every ordinal round-trips to a distinct, non-empty slug — a totality
        // sanity check for the CLI-owned diagnostic vocabulary.
        let reasons = [
            L3UnknownReasonV1::AdministrativeBudgetExhausted,
            L3UnknownReasonV1::AdministrativeStateBudgetExhausted,
            L3UnknownReasonV1::VisibleBudgetExhausted,
            L3UnknownReasonV1::ProfileError,
            L3UnknownReasonV1::CommitFailed,
            L3UnknownReasonV1::UndeclaredAssumption,
            L3UnknownReasonV1::AssumptionViolated,
            L3UnknownReasonV1::KeyConflict,
            L3UnknownReasonV1::AdapterIntegrityFailure,
            L3UnknownReasonV1::DivergenceObserved,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for reason in reasons {
            let slug = unknown_reason_slug(reason);
            assert!(!slug.is_empty());
            assert!(seen.insert(slug), "duplicate slug: {slug}");
        }
    }

    // Sanity: `L3LowerError` stays `Debug`-only (no `Display`), which is what
    // `lower_or_reject` relies on — kept as a compile-time-ish reminder if
    // that ever changes.
    #[test]
    fn lower_error_debug_is_deterministic() {
        let module = brix_syntax::parse("fn id(n) = n\n").unwrap();
        let err =
            lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous()).unwrap_err();
        assert_eq!(
            format!("{err:?}"),
            format!("{:?}", L3LowerError::FnItemNotAllowed("id".to_string()))
        );
    }
}
