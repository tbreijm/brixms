# 0005 — `audited_type_check` pads its configuration chain, so its `Audited` binds to a `Decomposition` that fails audit

**Lane:** semantics (soc-regimes)
**Status:** filed 2026-08-08 — **open**, awaiting ruling (tracked by [#262](https://github.com/tbreijm/brixms/issues/262))
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

## Interim disposition

**None. Behaviour is unchanged and nothing was fixed.** Unlike erratum 0004, this path
needs no interim scaffolding: its evidence already binds to an artifact, so the ADR-0016
fence accepts it and no route is `Provisional` on its account. The defect is in what the
artifact says, not in whether one exists — which is precisely why it needs a ruling rather
than a patch.

## Conformance IDs affected

- **SOC-LAW-05** (authority non-escalation) — an `Audited` publication whose evidence
  artifact fails the corresponding audit.
- ADR-0002 §5.1/§5.2 — `replay_verified` asserted over a chain that was never replayed and
  would not survive one.

## Implementation alignment

- `crates/soc-regimes/src/type_realization.rs` — `audited_type_check`; unchanged by this
  erratum.
- `crates/soc-regimes/src/type_realization.rs` —
  `test_multi_step_elaboration_tree_vs_linear_tension` is the existing demonstration; any
  ruling should keep it as the regression gate.
