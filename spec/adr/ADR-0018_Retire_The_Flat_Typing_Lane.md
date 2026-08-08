# ADR-0018 — Retire the Flat Typing Lane: A Padded Chain Is a False Record Under Any Grade

Status: **Proposed** (2026-08-08) (rules on issue #262 / `spec/errata/0005-flat-path-padded-decomposition.md`; retires the flat depth slice [ADR-0005](./ADR-0005_Type_Inference_as_Realization.md) named, superseded by [ADR-0007](./ADR-0007_Tree_Structured_Typing_Elaboration.md) §6; sibling of [ADR-0017](./ADR-0017_Tree_Realization_Support.md), which ruled on the tree path).

Date: 2026-08-08.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4.1 the verifier-authority table, §5.1/§5.2 recorded vs replay-verified), [ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md) (the Stage 2 depth slice this retires), [ADR-0007: Tree-Structured Typing Elaboration](./ADR-0007_Tree_Structured_Typing_Elaboration.md) (**§1 the padding this was introduced to remove**, §6, §7 "the flat path is retained unchanged"), [ADR-0016: The Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md) (§7.1 the `replay_verified` residual), [ADR-0017: Tree-Realization Support](./ADR-0017_Tree_Realization_Support.md) (the tree path's ruling).

This ADR removes code. It adds no proof rules, no calculus, no inference behaviour, and no CLI surface, and **no grade moves on any path a program can reach**.

---

## 1. The finding

`soc-regimes::type_realization::audited_type_check` builds its configuration chain by padding:

```rust
let mut configs = vec![src];
configs.resize(derivation.len() + 1, dst);

let verified_decomp =
    Decomposition::replay_verified(derivation, configs).expect("replay verified decomposition");
```

`[src, dst, dst, …, dst]`. The intermediate configurations were never computed; the chain is padded to satisfy `Decomposition`'s `configs.len() == generators.len() + 1` invariant, then stamped `replay_verified` and used as the evidence for an `Audited` judgement.

The sibling `type_check` pads identically and publishes `Derived` on a `Recorded` chain.

This is distinct from the ADR-0017 finding. There the evidence was *circular* — a digest of the claim, with no artifact at all. Here the evidence binds to a real `Decomposition` with its own content-addressed identity. **The artifact simply says something untrue**: that `g₂` runs from `dst` to `dst`, which never happened.

It is not a suspicion. `test_multi_step_elaboration_tree_vs_linear_tension` already demonstrated both halves — the padded chain passes syntactic `RealizesComp` (because a `dst == dst` middle always matches) and **fails `soc_core::audit_step` under sound generator semantics**. The repository has been carrying its own counterexample.

## 2. Decision: retire, do not repair

**The flat typing lane is removed**: `type_check`, `audited_type_check`, the `infer` engine they alone call, their tests, and their `soc-regimes` public re-export.

Three facts make this the answer rather than a repair.

**It is already superseded, by name.** ADR-0007 §1 introduced the tree encoding *because of this padding*:

> the earlier flat encoding was faking intermediate configs (`configs = [src, dst, dst, …, dst]`) … passes syntactic `RealizesComp` … but fails semantic audit under sound generator semantics.

ADR-0007 §7 then kept the flat path "retained unchanged" so that no existing test regressed. That was a migration courtesy, not a design commitment, and it has outlived its reason.

**Nothing calls it.** Verified across the workspace: `type_check` and `audited_type_check` have **zero** callers outside `soc-regimes`' own `#[cfg(test)]` module — `brix-lower`'s `check_module` and every `brix-cli` command go through `audited_type_check_tree`. The `infer` engine they wrap is reachable from nothing else. This lane cannot be reached by any Brix program.

**The padding is the defect, and it survives a downgrade.** Publishing `Derived` on the padded chain instead of `Audited` would remove the false *verification* claim while keeping the false *record*. ADR-0002 §5.1's `Recorded` form means "the hot loop recorded this factorization"; a record that misstates its own intermediate configurations is not made honest by declining to verify it.

### 2.1 Why not the alternatives

**Materialise real intermediate configurations** (erratum 0005 option 1) would fix the root cause: thread intermediate types out of `infer` so the chain carries real `ConfigId`s. It is the right fix for a path that mattered. Here it reworks a ~250-line inference engine to repair a lane with no caller, duplicating what `infer_tree` already does correctly — ADR-0007's whole point.

**Route it through a real audit** (option 2) needs a `GeneratorSemantics` for typing rules, which is ADR-0015 ⟨D-PRIM⟩'s kernel-owned primitive-relation registry. Building that for a dead path, ahead of the ADR that owns it, inverts the dependency.

## 3. What is removed, and what is not

Removed from `crates/soc-regimes/src/type_realization.rs`:

| | |
|---|---|
| `pub fn type_check` | `Derived` on a padded `Recorded` chain |
| `pub fn audited_type_check` | `Audited` on a padded `ReplayVerified` chain |
| `fn infer` | the flat inference engine, reachable only from those two |
| their `#[cfg(test)]` call sites | including `test_multi_step_elaboration_tree_vs_linear_tension` (see §4) |
| the `soc-regimes` re-export | `pub use type_realization::{audited_type_check, type_check, …}` |

**Explicitly preserved**, because they are live and correct:

- `infer_tree` / `audited_type_check_tree` — the ADR-0007 replacement, and after ADR-0017 the only typing publisher. Untouched.
- `Expr`, `Ty`, `TyCtx`, `TypeError`, `unify`, the generator constructors, the `NUMERIC`/`GRADE` lattices — shared with the tree path.
- **The entire settlement lane.** `Decomposition`, `commit_tick`, `audit_step`, and `brix_elaborate::elaborate_decomposition` are *not* implicated. Their chains carry real intermediate configurations produced by the engine, and `crates/brix-elaborate/tests/b3_end_to_end.rs` drives the genuine `commit_tick → audit_step → elaborate_decomposition → Proven` path with a real `GeneratorSemantics` replay. **The defect was in one typing-lane caller of `Decomposition`, never in `Decomposition` or in `elaborate_decomposition` itself.**

## 4. The counterexample is preserved, not deleted

`test_multi_step_elaboration_tree_vs_linear_tension` is the evidence for this finding, and it goes with the code it tests. Deleting a demonstration along with the defect it demonstrates would leave nothing guarding the regression.

Its property is therefore restated on the surviving path, as
`tree_derivation_carries_no_padded_step`: for the same expression, every leaf of the tree derivation satisfies `src != dst` — which is exactly the predicate `NonPaddedSemantics` used to reject the flat chain. If inference ever starts padding tree endpoints, that test fires.

The negative half — that a padded chain fails a sound audit — remains recorded in `spec/errata/0005-flat-path-padded-decomposition.md` and in §1 above, where it belongs once no code produces such a chain.

## 5. Consequences

**ADR-0005's Stage 2 depth slice is retired.** ADR-0005 §6 describes replaying a tight decomposition to upgrade `Derived → Audited` on the typing lane; `audited_type_check` was that slice's implementation. The *architecture* it describes is unchanged and now runs through the tree encoding (ADR-0007) with the artifact ADR-0017 gave it. What is retired is the flat implementation, which never performed the replay it claimed.

**No grade moves.** No program can reach the removed functions, so no `brix check`, `why`, `prove`, or `whynot` output changes. `crates/brix-lower/tests/lower_proven.rs` passes unchanged, as under ADR-0017.

**A public API narrows.** `soc_regimes::{type_check, audited_type_check}` are removed from the crate's exports. The workspace has no other consumer; an external one would be depending on the padded chain, which is the thing being retired.

**Audit finding A-3 shrinks.** `docs/audit/issue-63/README.md` A-3 named three publishers outside `soc-core`. Two are removed here; the third (`audited_type_check_tree`) was resolved by ADR-0017. The general residual — that `Decomposition::replay_verified` is an unchecked stamp (ADR-0016 §7.1) — is untouched and still routes to #178, but after this ADR **`soc_core::audit::audit_step` is its only caller**, which is what that residual assumed all along.

## 6. Non-goals

- **No inference change.** `infer_tree` is untouched; the removal takes no shared helper with it.
- **No calculus, sums/matches/coverage, or ADR-0011 Track 2 change.**
- **No frozen ABI change.** No canonical encoding, ordinal, or vector is touched; `vectors/` is not re-blessed.
- **Not a narrowing of `Decomposition::replay_verified`.** That is ADR-0016 §7.1 / A-3 / #178. This ADR removes its last unearned caller; it does not change the constructor.
- **No new decided negative.** Nothing here produces `Refuted`.
