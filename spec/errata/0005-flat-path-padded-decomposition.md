# 0005 — `audited_type_check` pads its configuration chain, so its `Audited` binds to a `Decomposition` that fails audit

**Lane:** semantics (soc-regimes)
**Status:** **ruled 2026-08-08** by [ADR-0018](../adr/ADR-0018_Retire_The_Flat_Typing_Lane.md) (filed 2026-08-08; tracked by [#262](https://github.com/tbreijm/brixms/issues/262))
**Affected conformance:** SOC-LAW-05 (authority non-escalation); ADR-0002 §5.1/§5.2 (recorded vs replay-verified); ADR-0007 §1 (the padding this motivated the tree encoding to remove)

## Provenance

Found while ruling on [#259](https://github.com/tbreijm/brixms/issues/259) /
[ADR-0017](../adr/ADR-0017_Tree_Realization_Support.md). Auditing the *tree* path's
support meant reading its flat sibling sixty lines up, which turned out to have a
different defect of the same family.

Filed separately rather than folded into ADR-0017, per #259's instruction to say so
rather than expand scope quietly.

## The defect

`crates/soc-regimes/src/type_realization.rs`, `audited_type_check`:

```rust
let mut configs = vec![src];
configs.resize(derivation.len() + 1, dst);

let verified_decomp =
    Decomposition::replay_verified(derivation, configs).expect("replay verified decomposition");
```

The configuration chain is **padded** — `[src, dst, dst, …, dst]` — to satisfy
`Decomposition`'s `configs.len() == generators.len() + 1` invariant without materialising
real intermediate configurations. It is then stamped `replay_verified` and used as the
evidence for an `Audited` judgement.

## Why it is distinct from erratum 0004

Erratum 0004 (the tree path) was **circular**: the evidence was a digest of the
proposition being claimed, so no artifact existed at all.

Here the evidence is *not* circular. It binds to a real `Decomposition` with its own
content-addressed identity. The problem is that the artifact it binds to is one an actual
audit **rejects**.

That is already demonstrated in-tree: `test_multi_step_elaboration_tree_vs_linear_tension`
builds exactly this padded chain and shows it passes `elaborate_decomposition`
syntactically but **fails `soc_core::audit_step` under sound (non-identity) generator
semantics** — the intermediate configurations are not realized by their generators,
because they were never real.

ADR-0007 §1 names this as the reason the tree encoding exists:

> passes syntactic `RealizesComp`… but fails semantic audit under sound generator semantics

The tree encoding was introduced to fix it. The flat path was retained unchanged (ADR-0007
§7, so that no existing test regressed) and still does it.

## Where this sits among the neighbouring findings

| | Defect | Disposition |
|---|---|---|
| Erratum 0004 / #259 | The *tree* path's `Audited` had no artifact | **Ruled** — ADR-0017 gives it one, earned by a checker |
| **This erratum / #262** | The *flat* path's `Audited` has an artifact, but a fabricated one | Open |
| ADR-0016 §7.1 / A-3 / #178 | `Decomposition::replay_verified` is an unchecked stamp, so nothing structurally prevents either | Open, general |

This one is the middle case: not the general residual, and not the tree path, but a
specific known-bogus artifact with a test already proving it bogus.

## Why it cannot be guessed

Three resolutions are available and they differ in cost and in what they say:

1. **Materialise real intermediate configurations** during flat inference, so the chain is
   genuine. Most faithful; touches `infer`.
2. **Route the flat path through a real audit** — produce a `Recorded` decomposition and
   let `soc_core::audit_step` under sound generator semantics be what upgrades it, exactly
   as the settlement lane works. This is the option that would make the flat path's
   `Audited` mean the same thing the settlement lane's does.
3. **Retire the flat `Audited` path.** ADR-0007 §6 introduced the tree encoding to
   supersede it; `audited_type_check` may no longer deserve an `Audited` outcome at all.

Option 3 needs an inventory of what still calls `audited_type_check` on the product path —
ADR-0007 §7 retained it specifically so nothing regressed, which suggests it may now be
vestigial.

## Ruling (adopted 2026-08-08, ADR-0018)

**Option 3: retire the flat typing lane.** Not a repair — a removal.

Three facts decided it:

1. **ADR-0007 §1 already named this padding as the reason the tree encoding
   exists**, and §7 kept the flat path only so that no test regressed. That was a
   migration courtesy, not a design commitment.
2. **Nothing calls it.** Verified across the workspace: `type_check` and
   `audited_type_check` have zero callers outside `soc-regimes`' own test module.
   `brix-lower`'s `check_module` and every `brix-cli` command go through
   `audited_type_check_tree`. The `infer` engine they wrap is reachable from
   nothing else. No Brix program can reach this lane.
3. **The padding survives a downgrade.** Publishing `Derived` instead of
   `Audited` (the smaller fix) removes the false *verification* claim but keeps
   the false *record*: a `Recorded` chain that misstates its own intermediate
   configurations is not made honest by declining to verify it.

Removed: `type_check`, `audited_type_check`, the `infer` engine, their tests, and
the `soc-regimes` re-export. Preserved: `infer_tree`/`audited_type_check_tree`,
every helper they share, and **the entire settlement lane** — `Decomposition`,
`commit_tick`, `audit_step`, and `elaborate_decomposition` build real chains and
were never implicated. The defect was in one typing-lane *caller* of
`Decomposition`, never in `Decomposition` itself.

Options 1 (materialise real configs) and 2 (route through a real audit) were
rejected as disproportionate: the first reworks a ~250-line inference engine to
repair a lane with no caller, duplicating what `infer_tree` already does; the
second needs ADR-0015 ⟨D-PRIM⟩'s registry, which would invert the dependency.

**No grade moves** — nothing reachable changed. `crates/brix-lower/tests/lower_proven.rs`
passes unchanged.

### The counterexample is preserved

`test_multi_step_elaboration_tree_vs_linear_tension` was the evidence for this
erratum, and it went with the code it tested. Its property is restated on the
surviving path as `tree_derivation_carries_no_padded_step`: every leaf of a tree
derivation satisfies `src != dst`, which is exactly the predicate
`NonPaddedSemantics` used to reject the flat chain. Four inference-property tests
that happened to live on the flat path (unbound variable, non-function
application, occurs check, determinism) were re-expressed against `infer_tree`,
so the deletion cost no coverage.

## Superseded interim disposition

**None. Behaviour was unchanged and nothing was fixed** when this was filed. Unlike erratum 0004, this path
needs no interim scaffolding: its evidence already binds to an artifact, so the ADR-0016
fence accepts it and no route is `Provisional` on its account. The defect is in what the
artifact says, not in whether one exists — which is precisely why it needs a ruling rather
than a patch.

## Conformance IDs affected

- **SOC-LAW-05** (authority non-escalation) — **resolved**: the publication is removed, so
  no `Audited` judgement rests on an artifact that fails its own audit.
- ADR-0002 §5.1/§5.2 — **resolved**: no `replay_verified` is asserted over a padded chain
  anywhere. After this ruling `soc_core::audit::audit_step` is the **only** caller of
  `Decomposition::replay_verified`, which is what the ADR-0016 §7.1 residual assumed.
- **ADR-0005** — its Stage 2 flat depth slice is retired; the architecture it describes now
  runs through the ADR-0007 tree encoding with the ADR-0017 artifact.

## Implementation alignment

- `crates/soc-regimes/src/type_realization.rs` — `audited_type_check`; unchanged by this
  erratum.
- `crates/soc-regimes/src/type_realization.rs` —
  `test_multi_step_elaboration_tree_vs_linear_tension` is the existing demonstration; any
  ruling should keep it as the regression gate.
