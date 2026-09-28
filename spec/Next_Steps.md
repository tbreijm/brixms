# Next Steps

The July 2026 list this file used to hold is done: `Audited` landed in the
outcome lattice (`crates/brix-semantic/src/outcome.rs`), `soc-core` exists
with the naive oracle and the incremental engine, and the O(Δ) gate
(`crates/soc-core/tests/o_delta_gate.rs`) runs in CI. That work is history,
not a plan — see `git log` and the ADRs it names for how it happened.

**The forward-looking plan now lives in
[`docs/planning/beta-roadmap.md`](../docs/planning/beta-roadmap.md)**,
organized as milestones (one shared expression language across profiles;
relations and per-entity decisions; a persistent, revisable knowledge base;
tooling and integration) with open design questions and acceptance criteria
for each. Read it for the full picture.

## The immediate actions

In order, ahead of the milestones above:

1. **Ratify or fold in ADR-0032 through ADR-0036.** Each is marked "Proposed
   implementation" even though its code is landed, tested, and part of the
   0.1.0-alpha.3 source tree (pure finite-decision helper functions,
   structured `brix.input@2` inputs, short-circuiting Boolean operators,
   exact integer division, unary minus). Either ratify them as-is or fold
   their content into a successor ADR; leaving landed behavior permanently
   "Proposed" understates what a reader can already rely on.
   **Acceptance:** each ADR's Status line reads "Accepted" (or is explicitly
   superseded), matching its code's presence in `crates/brix-lower`.

2. **Decide ADR-0037 (bounded lists and folds).** It is "Proposed design"
   only — no `List<T>` input, `sum`/`count`/`all`/`any` fold, or `max`-bound
   syntax exists in the finite-decision lane yet. Review it against the
   "Relations & per-entity decisions" milestone in the beta roadmap before
   implementing, since a rule-schema design there may subsume it.
   **Acceptance:** ADR-0037 is either accepted and implemented, or explicitly
   superseded by whichever ADR the "finite relations" roadmap milestone
   produces.

3. **Reconcile the two-lane split documented in
   [`docs/brix-language.md`](../docs/brix-language.md#the-two-lanes).**
   `check_module` (the `let`-lane type checker) and
   `lower_finite_decision_plan` (the finite-decision lane) currently accept
   different expression subsets under the same grammar — `&&`/`||`/`!` and
   the `div_*`/`mod_euclid` built-ins type-check in one lane and are
   `Unresolved`/`Unsupported` in the other; `/` is Float division in one and
   refused in the other. This is the concrete instance of the roadmap's "One
   language" milestone.
   **Acceptance:** a single written decision (an ADR, since it can move
   program identity) on which lane's behavior is authoritative for each
   discrepancy, landed as either a fix or a documented, intentional
   difference — not silence.
