# `spec/` — BrixMS specification, decisions, and plan

This directory holds the **governing decisions** (ADRs), the **master plan**,
the **normative contracts** (`SOC_Semantic_Laws.md`,
`Type_Realization_Contract.md`), and the **archive** of superseded plans and
specifications. There is no single current language-specification document —
the language surface is defined by the accepted ADRs (see "Reading order"
below and ADR-0030 through ADR-0041).

## Reading order (start here)

1. **[`adr/ADR-0002_SOC_Constitution.md`](./adr/ADR-0002_SOC_Constitution.md)** —
   the current constitution. One category of configurations and witnesses;
   realization as a lax/tight functor; dynamics as a settlement coalgebra
   determinized by a keyed calendar; the epistemic outcome lattice; the anti-v1
   engineering doctrine (the O(Δ) invariant). **Read this first.**
2. **[`SOC_Semantic_Laws.md`](./SOC_Semantic_Laws.md)** — the normative law
   registry and executable conformance map. Every law names its exact domain,
   context, authority, evidence, failure form, current gate, and open obligation.
3. **[`Type_Realization_Contract.md`](./Type_Realization_Contract.md)** — the
   normative contract for the native typing regime (issue #53): canonical
   inputs, contexts, every primitive generator's exact realization relation,
   derivation well-formedness, the negative-outcome taxonomy, grades, and the
   discharge artifact. Every clause carries its evidence status — pinned by a
   named test, partly pinned, or specified with no implementation yet.
4. **[`Build_Plan_v3_SOC.md`](./Build_Plan_v3_SOC.md)** — the master plan: SOC's
   semantic stages interleaved with the engineering order into one
   dependency-ordered sequence with per-step gates.
5. **[`Next_Steps.md`](./Next_Steps.md)** — the immediate 3–5 actions.
6. **[`Issue_Disposition_2026-07.md`](./Issue_Disposition_2026-07.md)** — every
   open issue re-classified (keep/reframe/park/close) + the new issues to open.
7. **[`adr/ADR-0001_Proof_Substrate.md`](./adr/ADR-0001_Proof_Substrate.md)** —
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
    ADR-0030 .. ADR-0041        ← the current language-surface ADRs (finite-
                                   decision alpha, external inputs, functions,
                                   structured inputs, Boolean ops, integer
                                   division, unary minus, bounded lists,
                                   finite relations, persistent knowledge base —
                                   check each one's own Status line)
  Build_Plan_v3_SOC.md          ← CURRENT master plan
  Next_Steps.md                 ← immediate actions
  Issue_Disposition_2026-07.md  ← issue re-classification + new issues
  errata/                       ← spec errata (append-only rulings)
  archive/                      ← superseded specs and build plans (see below)
```

### Archived (superseded) documents

Each carries a one-line header pointing at its successor. Retained for historical
reference and for design content that survives (the `brix-oracle` reference
design, the determinism discipline, the ring-model orchestration and gate
vocabulary).

- `archive/BrixMS_v9_0.md` — the pre-SOC "Living Model" v9 language and
  platform specification. Superseded by the SOC constitution
  ([`ADR-0002`](./adr/ADR-0002_SOC_Constitution.md)),
  [`SOC_Semantic_Laws.md`](./SOC_Semantic_Laws.md), and
  [`Type_Realization_Contract.md`](./Type_Realization_Contract.md). Retained
  as the finite-presentation frontend's source material, the `brix.type`
  structural-regime corpus, and (Appendix G) the normative sketch for
  canonical encoding. `archive/BrixMS_Language_Specification_v9_0.md` is a
  near-duplicate, pre-erratum fork of this file (moved here from `docs/`; see
  its own header for the diff).
- `archive/Ring0_Build_Plan.md` and its byte-identical duplicate
  `archive/BrixMS_Toolchain_Build_Plan_Ring0.md`
- `archive/Build_Plan_v2.md` and its byte-identical duplicate
  `archive/BrixMS_Build_Plan_v2_Toolchain_First.md`

The build-plan quartet is **superseded by `Build_Plan_v3_SOC.md`.** Under
ADR-0002 the toolchain is demoted from the semantic center to a *finite
`F_O`-presentation frontend*.

### A note on ADR-0028

There are two documents numbered ADR-0028 — a drafting-order collision, not a
correction of one by the other. Both are accepted and both stay under their
current filenames because too much else already links to them by full name:

- [`ADR-0028_Typing_Over_Context_Expression_Pairs.md`](./adr/ADR-0028_Typing_Over_Context_Expression_Pairs.md)
  — the native typing regime judges `(Γ, e)` pairs, not bare expressions
  (`Type_Realization_Contract.md`'s ⟨D-CTXENDPOINT⟩/⟨D-CTXPUBLISH⟩).
- [`ADR-0028_Witness_Provider_Ontology.md`](./adr/ADR-0028_Witness_Provider_Ontology.md)
  — witness providers present candidates without becoming a separate semantic
  entity (referenced by `ADR-0029`, `ADR-0030`, and SOC-LAW-07's governance
  monotonicity).

A bare "ADR-0028" elsewhere in the repo, with no filename or link, almost
always means whichever of the two its surrounding sentence is actually about
— check the topic, not just the number.

## Related, elsewhere in the repo

- **`docs/`** — the SOC foundation `.tex`
  ([`SOC_core_foundations_revised.tex`](../docs/SOC_core_foundations_revised.tex)),
  the scientific-article outline
  ([`BrixMS_Scientific_Article_Outline.md`](../docs/BrixMS_Scientific_Article_Outline.md)),
  the conceptual language overview
  ([`brix-language.md`](../docs/brix-language.md)), the trusted-boundary audit
  record (`audit/`), and forward-looking design notes (`planning/`, which
  documents proposed syntax on purpose — it is not a description of shipped
  behavior). There is no separate article `.tex`/`.pdf` draft beyond the
  outline and the foundation document itself.
- **`crates/brix-semantic/`** — the substrate implementation (ADR-0002 §6): the
  outcome lattice, `ContextId` root anchor, and the artifact identities, being
  extended with the SOC artifacts (`Witness`, `RegimeId`, `𝒢`, `Decomposition`,
  `Realizes`).
