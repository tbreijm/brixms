//! Adversarial test suite for the elaboration boundary's source verification
//! (ADR-0016 §6, audit finding A-2).
//!
//! `elaborate_decomposition` and `elaborate_tree` verify their source
//! internally — via `AuditedSource::verify` — before ever touching the
//! kernel, and report a failed verification as `ElaborationResult::Refused`,
//! never `NotElaborated` and never `Proven`. Every negative test here proves
//! a specific illegal caller is turned away at that boundary; the honest-path
//! test proves the fence refuses only illegal callers, not legal ones.

use brix_elaborate::{
    elaborate_decomposition, elaborate_tree, ElaborationResult, RealizesTree, TreeObj,
};
use brix_kernel::Budget;
use brix_semantic::{
    Authority, ConfigId, ContextId, DecompVerification, Decomposition, GeneratorId,
    GeneratorRegistry, GeneratorSemantics, Judgement, Outcome, PropositionId, PublicationError,
    Support, TreeDerivation,
};

fn context() -> ContextId {
    ContextId::root()
}

fn proposition(tag: &str) -> PropositionId {
    PropositionId::from_canon(tag.as_bytes())
}

fn recorded_decomposition(tag: &str) -> Decomposition {
    let g = GeneratorId::named(&format!("elab-fence.{tag}.g@1"));
    let x0 = ConfigId::from_canon(format!("elab-fence.{tag}.x0").as_bytes());
    let x1 = ConfigId::from_canon(format!("elab-fence.{tag}.x1").as_bytes());
    Decomposition::recorded(vec![g], vec![x0, x1]).expect("well-formed chain")
}

fn verified_decomposition(tag: &str) -> Decomposition {
    let g = GeneratorId::named(&format!("elab-fence.{tag}.g@1"));
    let x0 = ConfigId::from_canon(format!("elab-fence.{tag}.x0").as_bytes());
    let x1 = ConfigId::from_canon(format!("elab-fence.{tag}.x1").as_bytes());
    verified_chain(vec![g], vec![x0, x1])
}

fn budget() -> Budget {
    Budget::new(100, 100)
}

// --- elaborate_decomposition --------------------------------------------

#[test]
fn derived_source_is_refused_before_the_kernel_is_ever_reached() {
    let ctx = context();
    let prop = proposition("decomp-derived-source");
    let recorded = recorded_decomposition("decomp-derived-source");

    // A Derived judgement is legally published — the settlement kernel really
    // did record this chain — but Derived may never cross an elaboration
    // boundary (ADR-0002 §5 ¶2). If the kernel were reached, a malformed or
    // mismatched decomposition would surface as `NotElaborated`; the fence
    // must intercept before that, as `Refused`.
    let source = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&recorded),
    )
    .expect("a Recorded chain legally publishes Derived");

    let result = elaborate_decomposition(&source, &recorded, budget());
    match result {
        ElaborationResult::Refused(err) => {
            assert_eq!(
                err,
                PublicationError::NotAudited {
                    found: Outcome::Derived
                }
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("a Derived source must never cross the elaboration boundary")
        }
        other => panic!("expected Refused(NotAudited), got {other:?}"),
    }
}

#[test]
fn audited_source_presented_with_a_recorded_chain_is_refused_never_proven() {
    let ctx = context();
    let prop = proposition("decomp-audited-with-recorded-support");
    let verified = verified_decomposition("decomp-audited-with-recorded-support");
    let recorded = recorded_decomposition("decomp-audited-with-recorded-support");

    let source = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&verified),
    )
    .expect("a ReplayVerified chain legally publishes Audited");

    // The judgement really is Audited, but the artifact presented here is
    // the merely-recorded form, not the replay-verified chain its evidence
    // names.
    let result = elaborate_decomposition(&source, &recorded, budget());
    match result {
        ElaborationResult::Refused(err) => {
            assert_eq!(
                err,
                PublicationError::DecompositionVerificationMismatch {
                    outcome: Outcome::Audited,
                    expected: DecompVerification::ReplayVerified,
                    found: DecompVerification::Recorded,
                }
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("a recorded chain must never cross the elaboration boundary, even under a genuinely Audited judgement")
        }
        other => panic!("expected Refused(DecompositionVerificationMismatch), got {other:?}"),
    }
}

#[test]
fn audited_source_whose_evidence_names_a_different_decomposition_is_refused() {
    let ctx = context();
    let prop = proposition("decomp-evidence-binding-mismatch");
    let bound = verified_decomposition("decomp-evidence-binding-mismatch-bound");
    let other = verified_decomposition("decomp-evidence-binding-mismatch-other");

    let source = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&bound),
    )
    .expect("a ReplayVerified chain legally publishes Audited");

    // A genuinely Audited judgement, but the decomposition passed here is
    // not the one its own evidence id names.
    let result = elaborate_decomposition(&source, &other, budget());
    match result {
        ElaborationResult::Refused(err) => {
            assert_eq!(
                err,
                PublicationError::EvidenceBindingMismatch {
                    expected: source.evidence,
                    found: Support::Settlement(&other).evidence_id(),
                }
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("a decomposition other than the one an Audited judgement's evidence names must never cross the boundary")
        }
        other => panic!("expected Refused(EvidenceBindingMismatch), got {other:?}"),
    }
}

#[test]
fn honest_audited_source_still_reaches_proven() {
    let ctx = context();
    let prop = proposition("decomp-honest-path");
    let verified = verified_decomposition("decomp-honest-path");

    let source = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&verified),
    )
    .expect("a ReplayVerified chain legally publishes Audited");

    let result = elaborate_decomposition(&source, &verified, budget());
    match result {
        ElaborationResult::Proven { judgement, edge } => {
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(edge.target, source.id().digest());
        }
        ElaborationResult::Refused(err) => {
            panic!("the fence must refuse only illegal callers; a legal AuditChecker-published Audited source was refused: {err:?}")
        }
        other => panic!("expected Proven, got {other:?}"),
    }
}

// --- elaborate_tree ------------------------------------------------------
//
// `elaborate_tree` runs the same `AuditedSource::verify` boundary check, but
// against the tree lane's route (ADR-0017): the support is
// `Support::Tree(&derivation)`, a checked `TreeDerivation` artifact, not a
// `Decomposition`.

fn well_formed_tree() -> RealizesTree {
    let g = GeneratorId::named("elab-fence-tree.g@1");
    RealizesTree::Leaf {
        generator: g,
        src: TreeObj::Atom(ConfigId::from_canon(b"elab-fence-tree.x0")),
        dst: TreeObj::Atom(ConfigId::from_canon(b"elab-fence-tree.x1")),
    }
}

/// A `StructureVerified` artifact over [`well_formed_tree`]. `brix-elaborate`
/// cannot reach `soc-regimes`'s tree-audit checker, so — as in
/// `brix-semantic`'s own tests — the verified tag is constructed directly;
/// the boundary check under test is `AuditedSource::verify`'s binding, not
/// the checker itself.
fn well_formed_tree_derivation() -> TreeDerivation {
    verified_tree_derivation(well_formed_tree())
}

#[test]
fn derived_source_is_refused_by_elaborate_tree_before_the_kernel_is_ever_reached() {
    let ctx = context();
    let prop = proposition("tree-derived-source");
    let recorded = recorded_decomposition("tree-derived-source");
    let derivation = well_formed_tree_derivation();

    let source = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&recorded),
    )
    .expect("a Recorded chain legally publishes Derived");

    let result = elaborate_tree(&source, &derivation, budget());
    match result {
        ElaborationResult::Refused(err) => {
            assert_eq!(
                err,
                PublicationError::NotAudited {
                    found: Outcome::Derived
                }
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("a Derived source must never cross the elaboration boundary via elaborate_tree either")
        }
        other => panic!("expected Refused(NotAudited), got {other:?}"),
    }
}

#[test]
fn honest_audited_source_via_tree_realization_still_reaches_proven() {
    let ctx = context();
    let prop = proposition("tree-honest-path");
    // The tree lane's support is a checked `TreeDerivation` artifact
    // (ADR-0017 §5): `Judgement::publish` derives the matching evidence from
    // the artifact's own content-addressed id, not from `prop`.
    let derivation = well_formed_tree_derivation();
    let support = Support::Tree(&derivation);

    let source = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        support,
    )
    .expect("a StructureVerified tree derivation legally publishes Audited");

    let result = elaborate_tree(&source, &derivation, budget());
    match result {
        ElaborationResult::Proven { judgement, edge } => {
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(edge.target, source.id().digest());
        }
        ElaborationResult::Refused(err) => {
            panic!("the fence must refuse only illegal callers; a legal AuditChecker-published Audited source was refused: {err:?}")
        }
        other => panic!("expected Proven, got {other:?}"),
    }
}

/// Earn a `ReplayVerified` chain the honest way (ADR-0019 D7): build the
/// registry, supply a semantics that realizes exactly this chain's links, and
/// run the **real** checked transition. No test constructs a verified
/// artifact by assertion any more — the stamp constructor is gone.
///
/// This fixture semantics accepts precisely the chain it is handed, which is
/// what a fixture should do: the independent negatives (a padded chain, a
/// corrupted intermediate config, an unregistered generator) live beside
/// `verify_replay` itself in `brix-semantic`.
fn verified_chain(generators: Vec<GeneratorId>, configs: Vec<ConfigId>) -> Decomposition {
    struct ExactChain {
        links: Vec<(GeneratorId, ConfigId, ConfigId)>,
    }
    impl GeneratorSemantics for ExactChain {
        fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
            self.links
                .iter()
                .any(|(a, b, c)| a == g && b == src && c == dst)
        }
    }

    let mut registry = GeneratorRegistry::new();
    for g in &generators {
        registry.insert(*g);
    }
    let links = generators
        .iter()
        .enumerate()
        .map(|(i, g)| (*g, configs[i], configs[i + 1]))
        .collect();

    Decomposition::recorded(generators, configs)
        .expect("well-formed fixture chain")
        .verify_replay(&registry, &ExactChain { links })
        .expect("an honest fixture chain earns the tag")
}

/// Earn a `StructureVerified` derivation the honest way (ADR-0019 D5/D7):
/// build the registry from the generators the tree actually cites, then run
/// the real `verify_structure` transition against the tree's own endpoints.
///
/// A *checker* must never build its registry this way — that reduces
/// membership to "every cited generator is among the cited generators". Here
/// the job is to produce a legitimately verified fixture so the downstream
/// fence can be exercised; the real membership negative lives in
/// `soc-regimes`' `tree_audit`, against the regime's declared registry.
fn verified_tree_derivation(tree: RealizesTree) -> TreeDerivation {
    let mut registry = GeneratorRegistry::new();
    for leaf in tree.leaves() {
        if let RealizesTree::Leaf { generator, .. } = leaf {
            registry.insert(*generator);
        }
    }
    TreeDerivation::recorded(tree.clone())
        .verify_structure(&tree.src(), &tree.dst(), &registry)
        .expect("a well-formed fixture tree over its own generators earns the tag")
}
