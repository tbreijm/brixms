# `spec/` — BrixMS specification, decisions, and plan

This directory holds the **governing decisions** (ADRs), the **master plan**, the
**language specification**, and the **archive** of superseded plans.

## Reading order (start here)

1. **[`adr/ADR-0002_SOC_Constitution.md`](./adr/ADR-0002_SOC_Constitution.md)** —
   the current constitution. One category of configurations and witnesses;
   realization as a lax/tight functor; dynamics as a settlement coalgebra
   determinized by a keyed calendar; the epistemic outcome lattice; the anti-v1
   engineering doctrine (the O(Δ) invariant). **Read this first.**
2. **[`SOC_Semantic_Laws.md`](./SOC_Semantic_Laws.md)** — the normative law
   registry and executable conformance map. Every law names its exact domain,
   context, authority, evidence, failure form, current gate, and open obligation.
3. **[`Build_Plan_v3_SOC.md`](./Build_Plan_v3_SOC.md)** — the master plan: SOC's
   semantic stages interleaved with the engineering order into one
   dependency-ordered sequence with per-step gates.
4. **[`Next_Steps.md`](./Next_Steps.md)** — the immediate 3–5 actions.
5. **[`Issue_Disposition_2026-07.md`](./Issue_Disposition_2026-07.md)** — every
   open issue re-classified (keep/reframe/park/close) + the new issues to open.
6. **[`adr/ADR-0001_Proof_Substrate.md`](./adr/ADR-0001_Proof_Substrate.md)** —
   *superseded-in-part.* Its epistemic half (outcome lattice, authority table,
   artifact identities, retraction, cost-in-propositions) **survives verbatim**
   and is carried into ADR-0002; its hypergraph-as-ontology thesis is superseded.
   Retained for that frozen content and its rationale.

## Foundation document

**SOC = Settlement-Oriented Computing.** Settlement is the organizing primitive
the way objects are in OOP: `OOP : Java :: SOC : Brix`. SOC is the *paradigm*;
**Brix** is the language, and `.brix` its files. Do not call it "the SOC
language."

The conceptual foundation is **[`../docs/SOC_core_foundations_revised.tex`](../docs/SOC_core_foundations_revised.tex)**
("SOC") — *Settlement-Oriented Computing: Foundational Skeleton for Executable
Worlds*, A. Reijm, 2026-07-25. ADR-0002 is the ratified engineering constitution
derived from it; where they differ on terminology, SOC's terms are
authoritative.

Two things a reader must know before treating the skeleton as governing:

1. **It is a skeleton, and it labels its own claim strengths.** Its §1 ledger
   marks each claim `Established` / `Conditional` / `Target` / `Open`. The
   universal-world representation theorem and behavioral adequacy are
   **conjectures** (PD-2 and CJ-1), not results.
2. **⚠ On composition, the constitution is narrower than the skeleton, and the
   constitution governs.** The skeleton's "Compositional realization" axiom
   asserts `ρ_{g∘f} = ρ_g ∘ ρ_f` **globally**. ADR-0002 does not ratify that:
   realization is a *normal lax functor*, and strict equality is claimed **only
   on the tight generated subcategory 𝒦** (ADR-0002 §1 and the PD-1 obligation,
   "…a tight subcategory 𝒦 (i.e. `ρ_{g∘f}=ρ_g∘ρ_f` on 𝒦) … lax-only" elsewhere).
   That lax/tight split is the whole reason `Audited` exists as a lattice member
   and why the audit-factorization checker is a separate authority. This is a
   substantive refinement, not a terminology difference, so ADR-0002's
   "SOC's terms are authoritative" clause does **not** reinstate the global
   axiom.

The `.tex` cites `\bibliography{references}`; `references.bib` is not in the
repository, so the document does not currently compile as-is.

## Layout

```text
spec/
  README.md                     ← this index
  SOC_Semantic_Laws.md          ← normative law registry + conformance protocol
  conformance/
    soc-semantic-laws.json      ← machine-checked law/anchor/gate map
  adr/
    ADR-0001_Proof_Substrate.md ← superseded-in-part (frozen §§4–5–7 survive)
    ADR-0002_SOC_Constitution.md← CURRENT constitution
  Build_Plan_v3_SOC.md          ← CURRENT master plan
  Next_Steps.md                 ← immediate actions
  Issue_Disposition_2026-07.md  ← issue re-classification + new issues
  BrixMS_v9_0.md                ← the language specification (reference; the
                                   finite-presentation frontend's source material
                                   and the brix.type structural-regime corpus)
  BrixMS_Complexity_Profile.md  ← complexity profile (reference)
  errata/                       ← spec errata (append-only rulings)
  archive/                      ← superseded build plans (see below)
```

### Archived (superseded) plans

Each carries a one-line header pointing at its successor. Retained for historical
reference and for design content that survives (the `brix-oracle` reference
design, the determinism discipline, the ring-model orchestration and gate
vocabulary).

- `archive/Ring0_Build_Plan.md` and its byte-identical duplicate
  `archive/BrixMS_Toolchain_Build_Plan_Ring0.md`
- `archive/Build_Plan_v2.md` and its byte-identical duplicate
  `archive/BrixMS_Build_Plan_v2_Toolchain_First.md`

All four are **superseded by `Build_Plan_v3_SOC.md`.** Under ADR-0002 the
toolchain is demoted from the semantic center to a *finite `F_O`-presentation
frontend*.

## Related, elsewhere in the repo

- **`docs/`** — the SOC foundation `.tex`, the language specification long form,
  and the scientific-article materials (draft `.md`/`.tex`/`.pdf` and outline).
  The article materials are left in place (active LaTeX build artifacts).
- **`crates/brix-semantic/`** — the substrate implementation (ADR-0002 §6): the
  outcome lattice, `ContextId` root anchor, and the artifact identities, being
  extended with the SOC artifacts (`Witness`, `RegimeId`, `𝒢`, `Decomposition`,
  `Realizes`).
