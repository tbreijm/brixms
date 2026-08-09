# From Possibility to Commitment — Article Outline (rev. 3)

**Status:** working outline. **Committed to the repository deliberately** — rev. 2 was
written on 2026-07-23, never committed, and is **lost**. It is not recoverable from git,
any worktree, or any scratchpad. This revision reconstructs it from the surviving record
and then makes one substantive change of its own (§6, §0.0), so it is a genuine rev. 3
rather than a recovered rev. 2.

**Primary sources.** `docs/SOC_core_foundations_revised.tex` (the formal skeleton);
`spec/adr/ADR-0002_SOC_Constitution.md` and the ADR series through ADR-0019 (the ratified
design); `spec/BrixMS_v9_0.md` (Part II design contract); the v1 EPA thesis
(`BrixMS_EPA_Thesis_Reijm.pdf`, TU Delft 2019 — Brix formalism §5.3, Cartesian
coarse-phase detection §6.3, admitted intractability §8.3, which is the paper's own P3
evidence); the v2 TypeScript hypergraph implementation.

---

## §0.0 Contribution class and venue — **decided, not open**

**Contribution class: design-theory paper**, Gregor & Hevner *improvement* quadrant. The
deliverables are the formal core (D1–D6, T1–T5, C1), an artifact existence proof, and a
flagship demonstration. It is **explicitly not an evaluated-systems paper**: E1–E7 are
pre-registered, and a subset is reported as demonstration rather than evaluation.

**Venue: SoSyM regular paper.** It is the only reviewer pool that reliably knows both DEVS
and type systems; being a journal, it imposes no deadline and no page-limit amputation of
the formal core. **Fallback:** Onward! 2027 (spring deadline) *only* if the wait buys E1/E4/E6
evidence. **Sequel:** OOPSLA/ECOOP or a TOMACS empirical paper once evaluation completes.

Writing consequences of the SoSyM choice: lead with the **modeling problem** before any AI
hook; proofs to the appendix; target ~30–35 pp.

### The change this revision makes

A prior-art triage (2026-08-09, §6) established that **the categorical framing is not
novel and must not be claimed as the contribution.** Three of the four items previously
listed as our "novelty neighbourhood" are established results, and one of them is
essentially our core object. §6 and the framing below are rewritten accordingly.

This *strengthens* the SoSyM design-theory choice and rules out any category-theory venue.
The honest claim is **known machinery, used correctly, with an accountability discipline
enforced by construction** — which is the improvement quadrant precisely as scoped.

## §0.1 Identity — **closed; resist further expansion**

Core claim, epistemological:

> A system does not need to know what is true. It needs to **govern what it accepts.**

Grounded in fundamental mathematics and CS (Tarski; Chandra–Merlin 1977 for P3; Denning's
1976 lattice model for T4 — a three-tier foundations discipline observed throughout the
formal core). Embodied as **modelling & simulation × software engineering**, following the
v1 thesis lineage. Validated by design science.

The synthesis — M&S + fundamental CS ⟹ information science and epistemic philosophy — is a
*derivation*, not a relabelling.

**M&S lineage owned explicitly:** scenarios/Drivers are Zeigler experimental frames;
validate/authorize is VV&A; "production is coupled simulation" is formal digital-twin
semantics via T5. DEVS/HLA and Petty & Weisel are home literature, not borrowed literature.

**Standing rule for §3:** no freestanding philosophy. Every philosophical concept must name
a mechanism *and* a theorem or experiment.

---

## §1 Introduction — the modelling problem first

The failure mode: systems that must act on incomplete, contested, revisable information
either (a) pretend to a single truth and silently overwrite it, or (b) accumulate
possibilities without ever committing, and so cannot act. Neither is auditable afterwards.

Thesis: **make settlement — the act of accepting one possibility as committed — the
organizing primitive**, and make every commitment carry the evidence that licensed it.

## §2 Motivating example

A modelling scenario where the same fact must be simultaneously (i) proposed by several
regimes, (ii) accepted under one policy, (iii) revised later, and (iv) explained to an
auditor months afterwards. Carried through the whole paper; instantiated in §7.

## §3 Positioning: from epistemology to mechanism

Philosophy → mechanism table. Every row names executable machinery:

| Concept | Source | Mechanism in the artifact |
|---|---|---|
| belief vs **acceptance** | L. J. Cohen | the outcome lattice: `Unknown`/`Derived`/`Audited`/`Proven` |
| **fixation of belief** | Peirce | *settlement* — the paper's title concept |
| ethics of belief | Clifford | authority map: no outcome without its evidence |
| institutional facts | Searle | commitment as a logged, replayable artifact |
| testimony | epistemology of testimony | provenance/`Dependency` edges |
| semantic information | Floridi | §3 positioning anchor |
| information flow | Dretske | upgrades the Shannon disclaimer to a positive claim |

## §4 Formal core

### Definitions

- **D1 Configuration.** An admissible world description. Not intrinsically state, pattern,
  policy, history, or program — its role is supplied by the witness interpreting it.
- **D2 Witness.** A typed lawful correspondence `w : A → B`.
- **D3 Realization.** Each witness carries `ρ_w ⊆ 𝒞 × 𝒞`; `x ⇒ʷ y` iff `(x,y) ∈ ρ_w`.
  Carriers are witness-determined: `|A| = { x | x ⇒^{id_A} x }`.
- **D4 Execution configuration.** `e = ⟨x, p, h⟩` — world, policy, history — itself a
  configuration (internal packaging).
- **D5 Settlement coalgebra.** `γ : ℰ → F(ℰ)`; the functor fixes the branching shape, the
  policy populates it. An SOC *program* is a pointed coalgebra `(ℰ, γ, e₀)`.
- **D6 Finite presentation.** `P = (C₀, W₀, Real, Policy, e₀)`, effectively enumerable.

**D7 — the additions the artifact forced** (ADR-0002, not in the skeleton):
the declared class 𝒢 of **primitive logged generators**; a **certified decomposition**
`k = gₙ ∘ ⋯ ∘ g₁` with its intermediate configurations; the **tight subcategory 𝒦**
generated by 𝒢; and the **epistemic outcome lattice with a total authority map** — each
outcome has exactly one authority entitled to publish it.

### Theorems

- **T1 Category formation.** Configurations and witnesses form **SOC**. *Established* —
  bookkeeping, not the result.
- **T2 Realization is a functor into `Rel` — and this is the load-bearing correction.**
  The skeleton (`SOC_core_foundations_revised.tex`, Axiom *Compositional realization*)
  asserts `ρ_{g∘f} = ρ_g ∘ ρ_f` **globally**. The ratified design (ADR-0002, PD-1 §10) is
  **lax in general and strict only on the tight subcategory 𝒦**.

  This discrepancy is not an erratum to be quietly reconciled — **it is where the
  contribution lives.** Laxity is the compiler's licence (compressed paths remain sound);
  strictness on 𝒦 is the auditor's guarantee. The split is *why* `Audited` exists as a
  distinct outcome. The paper must state the lax version as the theorem and the strict
  version as the special case, and the skeleton must be revised to match.
- **T3 Behaviour map.** If `F` has a final coalgebra, every SOC program has a unique
  `beh : ℰ → ℬ`. *Conditional* on existence.
- **T4 Authority non-escalation.** No judgement carries an outcome exceeding what its
  authority's evidence licenses; `Unknown` never becomes truth or falsity; grades move down,
  never up. Structural, not conventional — see §5. Denning's lattice is the antecedent.
- **T5 Adequacy / universal world.** *Conjecture.* There is a finitely presented `𝕌`
  simulating every finite presentation, with soundness, completeness and progress —
  equivalently, divergence-sensitive weak bisimulation. Reflection, self-representational
  closure, and program–simulation co-location are **corollaries conditional on it**.
- **C1 Complexity.** Cost per committed step is `∝ |Δ| × index fanout`, never `∝ |world|`.
  Doubling inert configurations must not change step cost.

**Open obligations, stated as such:** (i) universality transfer via the two-counter machine
encoding; (ii) construction of `𝕌`; (iii) tightness of the generated settlement subcategory
beyond its current per-witness operational discharge.

## §5 The artifact

Nine crates, ~29k lines of source and ~13.5k of tests; a canonical-identity layer with
frozen encodings; two kernels (settlement, dependent proof); a CLI exposing
`check / run / audit / prove / why / whynot`.

**The claim worth making here is architectural, and it is the paper's strongest evidence:**
the epistemic discipline is enforced *by construction*, not by review convention.

- A `Judgement` cannot be constructed outside the artifact crate; publication consults a
  route table of legal (authority, outcome, evidence-kind) triples and fails closed
  (ADR-0016).
- A *verified* artifact is **unconstructible without the check that earns it**: the
  verification tag is the output of a checked transition, never a constructor input, and
  the alternatives are compile-time errors (ADR-0019).
- Recorded and replay-verified evidence have **different content-addressed identities** by
  design, so "I recorded a chain" and "I replayed and verified it" are not confusable.

**Three findings are reported honestly as part of the design history**, because they
demonstrate the method rather than embarrassing it: circular evidence (a digest of the very
proposition claimed), a fabricated artifact (a padded configuration chain that passes
syntactic composition but fails semantic audit), and an unchecked verification tag. Each was
found by an agent working on something adjacent, *reported rather than silently patched*,
and closed by a ruling that named the next residual instead of implying completeness.

**Currently open, and stated in the paper:** the audit checker's oracle is *supplied*, so a
verified artifact proves its predicate was **executed**, not **authenticated**. That limit
is pinned by a passing test rather than left to prose.

## §6 Related work — **rewritten after the 2026-08-09 triage**

### What we do not claim

Cited as foundations in the opening paragraphs, not defended as contributions:

- **Sobociński, *Relational Presheaves, Change of Base and Weak Simulation*** — a relational
  presheaf **is** a lax functor into `Rel`. That is our `R : SOC → Rel`, already named and
  studied.
- **Brengos, *Lax Kleisli-valued presheaves and coalgebraic weak bisimulation*** (arXiv
  1404.5267, 2014; LMCS 2019) — coalgebraic **saturation** expressed via lax functors through
  an adjunction, and weak bisimulation generalized to lax functors. This is our
  divergence-sensitive saturation, published in 2014.
- **Silva, Bonchi, Bonsangue & Rutten, *Generalizing determinization from automata to
  coalgebras*** (LMCS 2013) — the keyed-determinization construction.
- **Green, Karvounarakis & Tannen, *Provenance Semirings*** (PODS 2007) — annotation and
  derivation propagation.
- **Rewriting logic / Maude reflection; universal coalgebra; TLA; graph transformation; the
  Chemical Abstract Machine** — already positioned in the skeleton's §"Position relative to
  adjacent formalisms".

A referee who knows Brengos and Sobociński would reject a paper claiming the categorical
framing. Concede it in the first paragraph and spend the credibility elsewhere.

### What we do claim

1. **The audit-generated tight subcategory.** Provenance semirings *annotate* derivations.
   We **require a committed step to factor through** a declared class of logged generators,
   with a certified decomposition, and an authority that must *replay* it to upgrade the
   grade. The deliberate lax/tight split (T2) — compiler licence vs auditor obligation — is
   the specific claim, and we could not locate it in the neighbours.
2. **The epistemic outcome lattice with one authority per outcome, enforced structurally**
   (§5). This is a software-architecture contribution with strong artifact evidence, and it
   is the leg to lead with.
3. **The settlement interface** (quiescence certificates). *Weakest leg* — it leans on T5,
   which is conjectural. Do not lead with it.

### Terminology collision — must be differentiated explicitly

"Commitment" collides with the multi-agent-systems literature: **Singh's commitment
machines**, nonmonotonic commitment machines, commitment protocols, and spheres of
commitment. There a commitment is a *directed social obligation between a debtor and a
creditor*. Ours is an *epistemic act of acceptance by a single system, carrying replayable
evidence*. Different object, same word. Also differentiate: TypeDB, Datomic/XTDB, LogicBlox,
ATMS, Winograd & Flores, Executable UML / fUML / MDA, and digital twins.

## §7 Flagship demonstration

The §2 scenario carried end to end: proposal under several regimes → settlement under
policy → audit replay months later → explanation. Reports `why` / `whynot` output and the
grade actually earned — including where the honest answer is `Audited` rather than `Proven`.

## §8 Evaluation plan — pre-registered E1–E7

Registered in full; a subset reported as **demonstration**, per §0.0. E4 is framed as type
soundness plus a **compile-must-fail** corpus. C1 is tested by the doubling-inert-configs
benchmark, which has been a build gate from the start.

## §9 Reviewer red-team

Anticipated objections, each with its answer and its concession:

1. *"The category theory is known."* — **Conceded in §6**; not the contribution.
2. *"`Proven` overclaims."* — It asserts the compositional-validity *implication*, a
   revision-invariant theorem, not that the settlement outcome is proven. Stated in the
   artifact's own documentation and enforced by the authority map.
3. *"The universal world is a conjecture."* — Yes, and it is labelled `Target`/`Open` in the
   claim ledger. Everything depending on it is stated as conditional.
4. *"O(Δ) is just incremental view maintenance."* — The technique is not new; the claim is
   that it is an *enforced invariant* with a gate, not an optimization.
5. *"Design science with no evaluation."* — Improvement quadrant, E1–E7 pre-registered,
   sequel paper named.
6. *"The oracle is trusted."* — Conceded and pinned by a test; see §5 and §10.

## §10 Limitations and future work

The supplied-oracle limit; undischarged tightness beyond operational per-witness discharge;
T5 and the two-counter universality transfer; the unfinished surface language; the dimensional
model's absence of unit normalization (a deliberate open design choice — nominal units vs
auto-normalization vs witnessed conversions — not a defect).

---

## Immediate actions before drafting prose

1. **Revise `SOC_core_foundations_revised.tex`'s *Compositional realization* axiom** to the
   lax form with strictness on 𝒦 (T2). The skeleton and the ratified design currently
   disagree, and the paper cannot cite both.
2. **Obtain and read** the Sobociński and Brengos papers in full, not from abstracts, and
   write the differentiation paragraph against their actual theorems.
3. **Re-extract the v1 thesis** §5.3 / §6.3 / §8.3 passages — the extracted text existed only
   in a session scratchpad and is gone, the same way rev. 2 was.
4. Decide whether the three §5 findings are reported as a numbered design-history subsection
   or folded into the artifact description. *Recommendation: numbered.* They are the most
   convincing evidence that the discipline does real work.
