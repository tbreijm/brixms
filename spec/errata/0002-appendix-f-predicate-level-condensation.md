# 0002 — Appendix F condensation is predicate-granular for tuple production

**Lane:** oracle (brix-oracle)
**Status:** proposed, awaiting ruling
**Affected sections:** Appendix F (Phase inference, steps 1 & 4), Part III §5
(rule dependency graph / SCC condensation), Part III §6 (`Masked.atPhase`)
**Affected conformance IDs:** Appendix I.1 (Incremental = full recompute —
"bit-identical … including … masks"), Appendix I.4 (Mask dynamics —
"updates live views at exactly the phases Appendix F dictates")

## The ambiguity

Appendix F step 1 builds the dependency graph D over **rules**: "positive edge
r₁ → r₂ when r₂ reads a relation r₁ derives," and step 4 condenses "SCCs of
positive edges." Part III §5 echoes this ("builds the rule dependency graph,
condenses strongly connected components"). Taken literally over rule nodes, two
rules that jointly derive one *recursive* relation land in **different** SCCs
whenever the recursion is not individually mutual. Transitive closure is the
minimal witness:

```
derive Base:  Reach(src: x, dst: y) from { Link(src: x, dst: y) }
derive Trans: Reach(src: x, dst: z) from { Reach(src: x, dst: y), Link(src: y, dst: z) }
```

Positive edges are `Base → Trans` (Trans reads Reach, which Base derives) and
`Trans → Trans` (Trans reads Reach, which Trans derives). There is **no**
`Trans → Base` edge (Base reads only `Link`), so Tarjan yields two SCCs
`{Base}` and `{Trans}` → **two phases** for a single relation.

## Why this is wrong (and why it matters even though the extent is right)

For purely monotone rules the least fixpoint is split-invariant, so the two-phase
assignment still computes the correct `Reach` extent. But the phase assignment
is itself **normative, observable output**, not just an evaluation schedule:

1. A phase boundary asserts that a relation's extent is **complete** and
   therefore safe for a non-monotone (`without`/aggregate) read. Completeness of
   `Reach` is a joint property of *all* rules deriving it; a boundary between
   `Base` and `Trans` asserts a meaningless intermediate state ("the Base part
   of Reach is finished"), stabilized here only by the topological-sort
   tie-break — an implementation artifact, not a property of the model.
2. `Masked` edges carry `atPhase` (Part III §6), and conformance category 4
   requires live views to update "at exactly the phases Appendix F dictates." A
   frozen reference implementation must emit a **canonical** phase assignment.
   Rule-level condensation is non-canonical (the `{Base}`-before-`{Trans}` split
   is one of several valid topological orders); predicate-level condensation is
   the standard, canonical object.
3. Step 4's in-SCC error check ("a strict or mask edge inside one SCC is a
   compile-time error") is meant to express Part III §5's "cycles through a
   non-monotone edge are errors" over a relation's recursive component. With
   co-producers split, an illegal non-monotone read of the shared relation is
   only caught by the weaker step-5 cross-component-cycle path (no minimal-path
   guarantee), not the direct step-4 diagnostic the spec intends.

Standard stratified-Datalog theory (Apt–Blair–Walker) defines strata as a map on
**predicates**, with every rule whose head is `p` living in `stratum(p)`. The
oracle's two failing phase counts are the literal-transcription artifact, not a
disagreement with that theory.

## Proposed ruling

> **Erratum (Appendix F, steps 1/4; Part III §5).** The dependency graph D is
> rule-granular, but condensation is predicate-granular for tuple production: in
> addition to the positive edges of step 1, all rules deriving the same relation
> R (Tuple heads into R; **mask heads excluded**) form a single strongly
> connected unit, so that every producer of R is assigned to one SCC and R's
> extent is complete at exactly one phase boundary. Equivalently, step 4
> condenses the graph obtained by adding a positive cycle over the producers of
> each relation in canonical rule-id order. Step 4's in-SCC check therefore
> reads: a strict or mask edge between two rules in one such component — i.e., a
> non-monotone read of a relation from inside its own production component — is a
> compile-time error. Mask-producing rules and constraint/query read-sites
> remain individual nodes; steps 3 and 5 are unchanged.

Mask heads are deliberately excluded from the co-production merge: the Part III
§6 mask phase rule depends on mask rules being separable from the producers they
mask (`producers(R) ⇒ M(R)`), so merging them would create spurious in-SCC
cycles on legal pricing-override programs.

## What the oracle does until ruled

`crates/brix-oracle/src/phase.rs::infer_phases` implements the ruling: before
Tarjan, for each relation with ≥2 Tuple-head producer rules it adds positive
edges forming a cycle over those producers (in sorted `RuleId` order), unioning
them into one SCC; mask-head producers are excluded. Steps 4 (strict/mask
in-SCC error) and 5 (condensation + topo-sort) are unchanged and operate on the
enlarged components. If the ruling comes back rule-granular after all, only the
pre-Tarjan edge-augmentation block is removed — no change to the SCC, error, or
ordering machinery.
