//! ADR-0012 Stage D — the plan-specific audit boundary (§2 item 7, §5, §9
//! Stage D).
//!
//! This is deliberately the smallest of the four L3 stages. It supplies
//! exactly the two ingredients `soc_core::audit::audit_journal` needs to be
//! callable over an L3 run — a [`brix_semantic::GeneratorRegistry`]
//! containing precisely the plan's `N` generators, and a
//! [`soc_core::audit::GeneratorSemantics`] impl that re-derives, from the
//! plan's own precomputed transition table (Stage B,
//! [`crate::l3_regime::L3TransitionTable`]), the exact pre/post world pair
//! each generator witnesses (§3.3's unique transition) — plus one thin,
//! non-authoritative entry point ([`audit_l3_journal`] /
//! [`audit_l3_run`]) that calls `audit_journal` with them.
//!
//! **This module mints nothing.** `soc_core::audit::audit_step`/
//! `audit_journal` remain the *sole* authority for the `Derived → Audited`
//! upgrade (ADR-0002 §4.1: only `Authority::AuditChecker` may publish
//! `Audited`); this module never constructs an `AuditResult::Audited` itself,
//! and the L3 settlement loop ([`crate::l3_run::run_l3_plan_with_interner`])
//! never calls anything here — auditing is an explicit, separate action
//! (ADR-0012 §5: "An optional explicit audit action calls `audit_journal`").
//!
//! **Re-derivation, not trust.** ADR-0012 §2 item 7 requires the semantics to
//! check "the exact plan, rule, source world, destination world, and fact
//! identity." [`L3TransitionTable::expected_endpoints`] is built once from
//! the validated, normalized plan and never reads a committed step's
//! recorded `src`/`dst` — a semantics that instead trusted the journal's own
//! claim about its endpoints would not be auditing anything.
//!
//! **Orthogonal to certification (ADR-0012 §7).** A [`crate::l3_run::L3RunReport`]'s
//! quiescence certificate is `Derived`-graded and carries its own
//! independent checker (`soc_core::saturate::check_quiescence_certificate`);
//! nothing in this module reads, constructs, or otherwise touches a
//! certificate. Auditing the journal a `Quiescent` run produced upgrades
//! that journal's settlement steps; it does not — and structurally cannot,
//! since this module never sees a certificate — upgrade the quiescence claim
//! itself. "An audited journal supporting a quiescence claim is not an
//! audited quiescence claim" (ADR-0012 §9 Stage D).

use std::rc::Rc;

use brix_semantic::{ContextId, GeneratorRegistry};
use soc_core::audit_receipt::ReceiptError;

use crate::l3_canon::ProgramIdV1;

use soc_core::audit::{audit_journal, AuditResult, GeneratorSemanticsV1};
use soc_core::journal::Journal;

use crate::l3_regime::L3TransitionTable;
use crate::l3_run::L3RunReport;

/// The plan-specific `ρ_g` relation ADR-0012 §2 item 7 requires, **as declared
/// data** (ADR-0020 D8).
///
/// This was an executable `GeneratorSemantics` implementation until ADR-0020.
/// Its answer was always a lookup — `table.expected_endpoints(g) == Some((src,
/// dst))` — so it loses nothing by becoming the rows it was looking up: one
/// `ExactRows` entry per generator, holding exactly the pre/post canonical
/// world identities [`L3TransitionTable`] associates with that generator's
/// rule (§3.3's unique transition `World(program, Cons(r, tail), h, n)
/// --g(program, r)--> World(program, tail, Append(h, Fact(r, value(r))), n + 1)`).
///
/// What it *gains* is an identity. The manifest is a deterministic function of
/// the immutable transition table, so
/// [`GeneratorSemanticsIdV1`](brix_semantic::GeneratorSemanticsIdV1) names the
/// exact oracle an audit ran under, and two audits over the same chain under
/// different plans are distinguishable (ADR-0020 §4).
///
/// The re-derivation discipline is unchanged and still load-bearing: the rows
/// come from the plan's own table, never from the journal being audited. A
/// fabricated fact yields a different world digest, so a forged destination can
/// never coincide with the one this plan's rule actually produces.
///
/// ⚠ **The expected manifest must be derived from the validated plan**, not
/// read out of a receipt being checked (ADR-0020 §2). That is what makes this
/// lane's anchor independent.
pub fn l3_generator_semantics(table: &L3TransitionTable) -> GeneratorSemanticsV1 {
    let mut manifest = GeneratorSemanticsV1::new();
    for g in table.generators() {
        // Total by construction: `generators()` yields exactly the keys
        // `expected_endpoints` resolves. A `None` here would be an internal
        // inconsistency, so declare no row rather than inventing one — the
        // audit then fails closed on an undeclared generator.
        if let Some((src, dst)) = table.expected_endpoints(g) {
            manifest.declare_rows(g, [(src, dst)]);
        }
    }
    manifest
}

/// Build the `GeneratorRegistry` 𝒢 for `table`: exactly the plan's `N`
/// generators — the same set [`crate::l3_regime::build_l3_observation_profile`]
/// declares as the realizing partition (ADR-0012 §4.1), no more, no fewer.
pub fn l3_generator_registry(table: &L3TransitionTable) -> GeneratorRegistry {
    let mut registry = GeneratorRegistry::new();
    for g in table.generators() {
        registry.insert(g);
    }
    registry
}

/// Audit every step of `journal`, in commit order, against `table`'s
/// plan-specific registry and semantics (ADR-0012 §2 item 7, §5).
///
/// This is a thin, **non-authoritative** wrapper over
/// [`soc_core::audit::audit_journal`] — the sole authority for the
/// `Derived → Audited` upgrade remains that function (ADR-0002 §4.1); this
/// function constructs no judgement and mints nothing. "Only
/// `AuditResult::Audited` produces a separate `Audited` judgement and
/// dependency; `AuditResult::Unknown` is returned as unknown audit status"
/// (ADR-0012 §5) — callers get exactly what `audit_journal` returned, per
/// step, unmodified.
pub fn audit_l3_journal(
    journal: &Journal,
    context: ContextId,
    table: &Rc<L3TransitionTable>,
) -> Vec<AuditResult> {
    let registry = l3_generator_registry(table);
    let semantics = l3_generator_semantics(table);
    audit_journal(journal, context, &registry, &semantics)
}

/// Convenience: audit an [`L3RunReport`]'s own journal, under its own run
/// context and transition table (ADR-0012 §9 Stage D). Equivalent to calling
/// [`audit_l3_journal`] with the report's `journal`/`context`/`table` — kept
/// as its own function only because a caller holding a full report otherwise
/// has to destructure it every time.
///
/// The settlement loop itself never calls this: auditing is an explicit,
/// separate action a caller opts into after a run completes (ADR-0012 §5),
/// and doing so never reads, mutates, or otherwise touches
/// `report.quiescence_certificate` — certification and grading are
/// orthogonal axes (ADR-0012 §7; §9 Stage D's orthogonality fixture).
pub fn audit_l3_run(report: &L3RunReport) -> Vec<AuditResult> {
    audit_l3_journal(&report.journal, report.context, &report.table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_semantic::{ConfigId, GeneratorId};

    use brix_canon::{Digest, Domain};
    use brix_semantic::{
        Decomposition, EdgeKind, Evidence, JudgementId, Outcome, Realizes, WitnessId,
    };
    use soc_core::calendar::Key;
    use soc_core::commit::Observation;
    use soc_core::journal::CommittedStep;
    use soc_core::saturate::{quiescence_certificate_id, SaturationBudget};

    use crate::l3::{lower_l3_plan, L3PlanV1, L3ValueV1, PlanLimitsV1, L3_PROFILE_MARKER_V1};
    use crate::l3_canon::{
        build_pending, fact_id, l3_generator_id, l3_value_id, l3_witness_id, rule_id, world_id,
        FactChainIdV1, FactV1, L3WorldV1, ProgramIdV1,
    };
    use crate::l3_run::{run_l3_plan, L3AdmChoice, SettlementStopV1};

    fn plan(src: &str) -> L3PlanV1 {
        let module = brix_syntax::parse(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .unwrap_or_else(|e| panic!("lowering failed: {e:?}"))
    }

    fn generous_budget() -> SaturationBudget {
        SaturationBudget::uniform(1_000)
    }

    /// A two-rule report plus the identities needed to hand-build tampered
    /// [`CommittedStep`]s against the very same table the report ran over.
    struct TwoRuleFixture {
        report: L3RunReport,
        program: ProgramIdV1,
        rule_a: crate::l3_canon::RuleId,
        rule_b: crate::l3_canon::RuleId,
        gen_a: GeneratorId,
        gen_b: GeneratorId,
        w0: ConfigId,
        w1: ConfigId,
        w2: ConfigId,
    }

    fn two_rule_fixture() -> TwoRuleFixture {
        let p = plan("rule a() = 1\nrule b() = 2\n");
        let report = run_l3_plan(&p, L3AdmChoice::Compiled, generous_budget());
        let program = report.run.program;
        let rule_a = rule_id(program, 0, "a");
        let rule_b = rule_id(program, 1, "b");
        let w0 = report.table.world_configs()[0];
        let w1 = report.table.world_configs()[1];
        let w2 = report.table.world_configs()[2];
        let gen_a = l3_generator_id(program, rule_a, w0, w1);
        let gen_b = l3_generator_id(program, rule_b, w1, w2);
        // Sanity: these are exactly the table's own two generators — this
        // fixture re-derives them the same way `build_l3_transition_table`
        // does, it does not invent parallel identities.
        assert_eq!(
            report.table.generators(),
            std::collections::BTreeSet::from([gen_a, gen_b])
        );
        TwoRuleFixture {
            report,
            program,
            rule_a,
            rule_b,
            gen_a,
            gen_b,
            w0,
            w1,
            w2,
        }
    }

    /// Mirror `soc_core::audit::audit_step`'s own derivation of the `Derived`
    /// judgement/observation exactly, so a hand-built tampered step is
    /// internally self-consistent — i.e. its recorded `observation` agrees
    /// with its own `witness`/`src`/`dst`/`decomposition`. This isolates each
    /// tamper fixture to the *one* condition it targets (source world,
    /// destination/fact world, or cited generator) rather than incidentally
    /// tripping `audit_step`'s log-integrity cross-check (step 1) first.
    fn consistent_observation(
        context: ContextId,
        witness: WitnessId,
        src: ConfigId,
        dst: ConfigId,
        decomposition: &Decomposition,
    ) -> Observation {
        let proposition = Realizes::new(witness, src, dst).proposition_id();
        let evidence = Evidence::SettlementReplay {
            body: decomposition.id().digest(),
        }
        .id();
        // Identity only — this helper reproduces the observation a committed
        // step would carry; it publishes nothing (ADR-0016 §3).
        let derived_id = JudgementId::recompute(context, proposition, Outcome::Derived, evidence);
        Observation {
            outcome_class: Outcome::Derived,
            judgement_digest: derived_id.digest(),
        }
    }

    fn make_step(
        context: ContextId,
        witness: WitnessId,
        src: ConfigId,
        dst: ConfigId,
        decomposition: Decomposition,
    ) -> CommittedStep {
        let observation = consistent_observation(context, witness, src, dst, &decomposition);
        CommittedStep {
            key: Key::new(0, 0, Digest::of(Domain::Value, b"l3-audit-fixture-key")),
            observation,
            decomposition,
            src,
            dst,
            witness,
        }
    }

    fn one_step_journal(step: CommittedStep) -> Journal {
        let mut journal = Journal::new();
        journal.append(step);
        journal
    }

    // -----------------------------------------------------------------
    // §9 Stage D fixture: the unchanged journal yields distinct linked
    // Derived/Audited judgements.
    // -----------------------------------------------------------------

    #[test]
    fn unchanged_journal_yields_distinct_linked_derived_and_audited_judgements() {
        let f = two_rule_fixture();
        assert_eq!(f.report.journal.len(), 2, "two committed rules");

        let results = audit_l3_run(&f.report);
        assert_eq!(results.len(), 2);

        for (step, result) in f.report.journal.steps().iter().zip(results.iter()) {
            match result {
                AuditResult::Audited(audited) => {
                    // Distinctness: the Audited judgement is a NEW
                    // JudgementId, never the pre-existing Derived one.
                    assert_ne!(
                        audited.audited_id, audited.derived_id,
                        "Audited must be a distinct judgement from Derived"
                    );
                    // The link: Audited -> Derived via EdgeKind::Premise,
                    // targeting exactly the Derived judgement's digest.
                    assert_eq!(audited.link.kind, EdgeKind::Premise);
                    assert_eq!(audited.link.target, audited.derived_id.digest());
                    // The pre-existing Derived judgement this links to is
                    // exactly the one the committed step itself published.
                    assert_eq!(
                        audited.derived_id.digest(),
                        step.observation.judgement_digest
                    );
                    assert_eq!(audited.audited.outcome, Outcome::Audited);
                }
                AuditResult::Unknown(reason) => {
                    panic!("unchanged journal step must audit clean, got Unknown({reason})")
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // §9 Stage D fixture: four separate tamper targets, each Unknown.
    // -----------------------------------------------------------------

    #[test]
    fn tampered_rule_yields_unknown() {
        // Cite the WRONG rule's generator (b's) for a's true src/dst pair.
        // `b`'s generator is a real, registered member of 𝒢 — this is not
        // caught by "generator outside 𝒢", only by re-deriving that `g_b`
        // does not witness `w0 -> w1`.
        let f = two_rule_fixture();
        let witness = l3_witness_id(f.gen_a);
        let bad_decomposition = Decomposition::recorded(vec![f.gen_b], vec![f.w0, f.w1]).unwrap();
        let step = make_step(f.report.context, witness, f.w0, f.w1, bad_decomposition);
        let results = audit_l3_journal(&one_step_journal(step), f.report.context, &f.report.table);
        assert_eq!(results.len(), 1);
        assert!(
            matches!(results[0], AuditResult::Unknown(_)),
            "a decomposition citing the wrong rule's generator must never audit clean"
        );
    }

    #[test]
    fn tampered_world_yields_unknown() {
        // Wrong SOURCE world: cite a's real generator/destination, but claim
        // it started from w2 (the terminal world) rather than w0.
        let f = two_rule_fixture();
        let witness = l3_witness_id(f.gen_a);
        let bad_src = f.w2;
        let bad_decomposition =
            Decomposition::recorded(vec![f.gen_a], vec![bad_src, f.w1]).unwrap();
        let step = make_step(f.report.context, witness, bad_src, f.w1, bad_decomposition);
        let results = audit_l3_journal(&one_step_journal(step), f.report.context, &f.report.table);
        assert_eq!(results.len(), 1);
        assert!(
            matches!(results[0], AuditResult::Unknown(_)),
            "a decomposition citing the wrong source world must never audit clean"
        );
    }

    #[test]
    fn tampered_fact_yields_unknown() {
        // Wrong destination world, built from a genuinely FORGED fact (a's
        // rule publishing payload 999 instead of the real 1) rather than an
        // arbitrary foreign ConfigId — this is specifically a fact tamper,
        // not just "some other world."
        let f = two_rule_fixture();
        let forged_fact = FactV1 {
            rule: f.rule_a,
            payload: l3_value_id(&L3ValueV1::Int(999)),
        };
        let forged_facts = FactChainIdV1::append(FactChainIdV1::genesis(), fact_id(&forged_fact));
        let forged_pending = build_pending(&[f.rule_b]);
        let forged_world = L3WorldV1 {
            program: f.program,
            pending: forged_pending,
            facts: forged_facts,
            fact_count: 1,
        };
        let forged_dst = world_id(&forged_world);
        assert_ne!(
            forged_dst, f.w1,
            "a forged fact payload must not collide with the real destination world"
        );

        let witness = l3_witness_id(f.gen_a);
        let bad_decomposition =
            Decomposition::recorded(vec![f.gen_a], vec![f.w0, forged_dst]).unwrap();
        let step = make_step(
            f.report.context,
            witness,
            f.w0,
            forged_dst,
            bad_decomposition,
        );
        let results = audit_l3_journal(&one_step_journal(step), f.report.context, &f.report.table);
        assert_eq!(results.len(), 1);
        assert!(
            matches!(results[0], AuditResult::Unknown(_)),
            "a decomposition resting on a forged fact must never audit clean"
        );
    }

    #[test]
    fn tampered_decomposition_yields_unknown() {
        // Structurally tampered decomposition: correct generator and
        // endpoints, but in the WRONG verification form
        // (`ReplayVerified` instead of `Recorded`) — `audit_step`'s own
        // log-integrity check (step 1) fails closed on this before the
        // relational-composition check ever runs, which is itself part of
        // the fail-closed discipline this fixture is checking.
        let f = two_rule_fixture();
        let witness = l3_witness_id(f.gen_a);
        // ADR-0019 D7: the verified form is earned, not stamped, even here.
        // The chain itself is honest — the tampering under test is that it is
        // presented to the auditor in the verified form when a *recorded* one
        // is required, not that its links are false.
        let mut registry = brix_semantic::GeneratorRegistry::new();
        registry.insert(f.gen_a);
        let mut semantics = soc_core::audit::GeneratorSemanticsV1::new();
        semantics.declare_rows(f.gen_a, [(f.w0, f.w1)]);
        let bad_decomposition = Decomposition::recorded(vec![f.gen_a], vec![f.w0, f.w1])
            .unwrap()
            .verify_replay(&registry, &semantics)
            .expect("the fixture chain earns the tag");
        let step = make_step(f.report.context, witness, f.w0, f.w1, bad_decomposition);
        let results = audit_l3_journal(&one_step_journal(step), f.report.context, &f.report.table);
        assert_eq!(results.len(), 1);
        assert!(
            matches!(results[0], AuditResult::Unknown(_)),
            "a decomposition in the wrong verification form must never audit clean"
        );
    }

    // -----------------------------------------------------------------
    // §9 Stage D fixture: the orthogonality fixture. Auditing the journal
    // of a Quiescent run MUST NOT change the quiescence certificate, its
    // identity, or its Derived grade.
    // -----------------------------------------------------------------

    #[test]
    fn auditing_a_quiescent_runs_journal_does_not_touch_the_quiescence_certificate() {
        let f = two_rule_fixture();
        let SettlementStopV1::Quiescent {
            certificate: cert_id_before,
        } = f.report.run.stop.clone()
        else {
            panic!("expected Quiescent, got {:?}", f.report.run.stop);
        };
        let cert_before = f
            .report
            .quiescence_certificate
            .clone()
            .expect("Quiescent carries a certificate");
        assert_eq!(cert_before.grade, Outcome::Derived);
        assert_eq!(quiescence_certificate_id(&cert_before), cert_id_before);

        // The explicit audit action: an entirely separate call, over the
        // same journal/context/table the report already carries.
        let results = audit_l3_run(&f.report);
        assert_eq!(results.len(), 2);
        for result in &results {
            assert!(
                matches!(result, AuditResult::Audited(_)),
                "sanity: this run's own journal is untampered"
            );
        }

        // The certificate, read straight off the SAME report after auditing,
        // is byte-identical, its identity unchanged, and its grade still
        // Derived. Nothing about calling `audit_l3_run` above could have
        // mutated `f.report` (it only borrows it), but this is the fixture
        // ADR-0012 §9 Stage D asks for, made explicit rather than left to
        // Rust's borrow checker to imply silently.
        let SettlementStopV1::Quiescent {
            certificate: cert_id_after,
        } = f.report.run.stop.clone()
        else {
            panic!("expected Quiescent, got {:?}", f.report.run.stop);
        };
        assert_eq!(cert_id_before, cert_id_after);
        let cert_after = f
            .report
            .quiescence_certificate
            .clone()
            .expect("still Quiescent, still carries a certificate");
        assert_eq!(cert_before, cert_after, "byte-identical certificate");
        assert_eq!(cert_after.grade, Outcome::Derived, "grade still Derived");
        assert_eq!(quiescence_certificate_id(&cert_after), cert_id_before);

        // Stronger than "unchanged": the two Audited judgements produced
        // are for the settlement steps' own Realizes propositions, and are
        // — by construction, since this module never reads the certificate
        // at all — distinct from the certificate's own Quiescent judgement.
        // An audited journal supporting a quiescence claim is not an
        // audited quiescence claim (ADR-0012 §7, §9 Stage D).
        for result in &results {
            if let AuditResult::Audited(audited) = result {
                assert_ne!(audited.audited_id.digest(), cert_before.judgement.digest());
                assert_ne!(audited.derived_id.digest(), cert_before.judgement.digest());
            }
        }
    }

    // -----------------------------------------------------------------
    // The registry contains exactly the plan's N generators, and no
    // adapter/decomposition or witness type is required to build it.
    // -----------------------------------------------------------------

    #[test]
    fn generator_registry_is_exactly_the_plans_generators() {
        let f = two_rule_fixture();
        let registry = l3_generator_registry(&f.report.table);
        assert_eq!(registry.len(), 2);
        assert!(registry.contains(&f.gen_a));
        assert!(registry.contains(&f.gen_b));
        assert!(!registry.contains(&GeneratorId::named("not-in-this-plan@1")));
    }
}

// ---------------------------------------------------------------------------
// ADR-0022 — source re-derivation.
// ---------------------------------------------------------------------------

/// Why a source-derived receipt check was refused (ADR-0022 D8).
///
/// Every variant is a refusal that produces no `Audited`, no receipt, and
/// never `Refuted`. The distinctions exist because they are different facts a
/// verifier operator needs to act on: bad bytes, a declined workload, a
/// program that is not the one expected, and a receipt that does not validate
/// are four different problems.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceReceiptError {
    /// The supplied bytes are not UTF-8.
    InvalidUtf8,
    /// Parsing refused — malformed source, or a `ParseLimits` bound exceeded.
    Parse(String),
    /// Lowering to the L3 fragment refused.
    Lower(String),
    /// The source lowers to a valid plan, but **not the expected one**
    /// (ADR-0022 D5). This is frontend drift or a substituted program being
    /// detected, and it must never fall back to the receipt's own manifest.
    ProgramMismatch {
        expected: ProgramIdV1,
        derived: ProgramIdV1,
    },
    /// The receipt did not validate against the locally derived environment
    /// (ADR-0020 D7).
    Receipt(ReceiptError),
}

/// **Re-derive the expected audit environment from source and check a receipt
/// against it** (ADR-0022 D1–D4) — the non-cryptographic answer to "which
/// oracle was authorized".
///
/// The verifier is given the `.brix` source and an **independently expected**
/// [`ProgramIdV1`]. It parses and lowers under its own resource policy,
/// refuses if the result is not the expected program, builds the transition
/// table, derives the registry and semantics manifest itself, and only then
/// hands them to [`check_audit_receipt_v1`].
///
/// **It deliberately takes no expected registry or semantics parameter.** Both
/// are derived internally. A caller that could supply them would reintroduce
/// exactly the hole ADR-0020 §2 describes — a consumer that adopts the
/// receipt's own expectation has authenticated nothing.
///
/// This is stronger than a signed authorization (ADR-0021): it checks that the
/// rows *follow from* the source under the local implementation, rather than
/// trusting an authorized signer not to lie (ADR-0022 D9). It closes ADR-0020
/// residuals 2 and 3 **for this path** — not for generic
/// `GeneratorSemanticsV1` callers, another regime, a deployment that withholds
/// source, or target selection when no expected program id is held.
///
/// Fails closed at every stage; no failure yields a weaker pass.
#[allow(clippy::too_many_arguments)]
pub fn check_l3_audit_receipt_from_source_v1(
    source: &[u8],
    expected_program: ProgramIdV1,
    parse_limits: brix_syntax::ParseLimits,
    plan_limits: &crate::l3::PlanLimitsV1,
    receipt: &soc_core::audit_receipt::SettlementAuditReceiptV1,
    step: &soc_core::journal::CommittedStep,
    context: ContextId,
) -> Result<soc_core::audit_receipt::SettlementAuditReceiptIdV1, SourceReceiptError> {
    // 1. Bytes → text, bounded before anything is decoded or allocated.
    let text = std::str::from_utf8(source).map_err(|_| SourceReceiptError::InvalidUtf8)?;

    // 2. Parse under this verifier's own resource policy. A refusal here is
    //    local policy, not a claim about the program (ADR-0022 D6).
    let module = brix_syntax::parse_bounded(text, parse_limits)
        .map_err(|e| SourceReceiptError::Parse(e.to_string()))?;

    // 3. Lower to the L3 fragment.
    let plan = crate::l3::lower_l3_plan(&module, crate::l3::L3_PROFILE_MARKER_V1, plan_limits)
        .map_err(|e| SourceReceiptError::Lower(format!("{e:?}")))?;

    // 4. Target selection: the program must be the one independently expected.
    //    Drift is detected here and never normalized away (ADR-0022 D5).
    let derived_program = crate::l3_canon::program_id(&plan);
    if derived_program != expected_program {
        return Err(SourceReceiptError::ProgramMismatch {
            expected: expected_program,
            derived: derived_program,
        });
    }

    // 5. Derive the audit environment locally. `build_l3_transition_table`
    //    computes the program id itself (ADR-0020 D8), so the table is
    //    plan-bound by construction.
    let mut interner = soc_core::intern::Interner::new();
    let table = crate::l3_regime::build_l3_transition_table(&mut interner, &plan);
    let registry = l3_generator_registry(&table);
    let semantics = l3_generator_semantics(&table);

    // 6. Hand the *derived* environment to the ADR-0020 checker, which
    //    replays rather than reads.
    soc_core::audit_receipt::check_audit_receipt_v1(receipt, step, context, &registry, &semantics)
        .map_err(SourceReceiptError::Receipt)
}
