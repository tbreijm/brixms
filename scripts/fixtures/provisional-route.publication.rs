// Negative fixture for scripts/test_law_map_provisional_gate.sh.
//
// Not compiled and not part of the workspace: it exists only so the
// provisional-route coupling in scripts/check_soc_law_map.py can be shown to
// FAIL on a table that still carries a RouteStatus::Provisional row, without
// putting one back into the real publication route table (ADR-0017 §8, and the
// same discipline scripts/fixtures/forbidden-kernel-dependency.metadata.json
// applies to the TCB dependency gate).
//
// This mirrors the shape of the row ADR-0016 §7 carried and ADR-0017 retired.

pub const ROUTES: &[Route] = &[
    Route {
        authority: Authority::ProofKernel,
        outcome: Outcome::Proven,
        support: SupportKind::KernelCertificate,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AuditChecker,
        outcome: Outcome::Audited,
        support: SupportKind::Tree,
        condition: RouteCondition::None,
        status: RouteStatus::Provisional,
    },
];
