# ADR-0017 — Tree-Realization Support: The `TreeDerivation` Artifact and What `Audited` Means on the Typing Lane

Status: **Proposed** (2026-08-08) (rules on issue #259 / `spec/errata/0004-tree-realization-audited-support.md`; implements ADR-0007 §6's already-stated intent by giving it an artifact; retires the `Provisional` route [ADR-0016](./ADR-0016_Authority_Publication_Fence.md) §7 opened; restores the premise [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md) relies on).

Date: 2026-08-08.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4.1 the verifier-authority table, §5.1/§5.2 recorded vs replay-verified, §6 the `Decomposition` artifact), [ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md) (the `Derived → Audited → Proven` upgrade path on the typing lane), [ADR-0007: Tree-Structured Typing Derivations](./ADR-0007_Tree_Structured_Typing_Elaboration.md) (**§4 epistemic scope, §6 what the tree "audit" is, §7 the deferred ρ-audit** — the prior ruling this ADR implements), [ADR-0008: Lambda in Tree Elaboration](./ADR-0008_Lambda_In_Tree_Elaboration.md), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§7 the append-only evolution rule a new canonical artifact must follow), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (the capped-at-`Audited` premise), [ADR-0016: The Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md) (§4 routes, §6 the audited-source boundary, §7 the finding this ADR rules on).

This ADR adds no proof rules, no calculus, no inference change, and no CLI surface. **No grade moves.** It replaces circular evidence with an artifact, and adds one check the typing lane was missing.

---

## 1. The finding

`crates/soc-regimes/src/type_realization.rs`, `audited_type_check_tree`, publishes an
`Outcome::Audited` judgement whose evidence body is:

```rust
Digest::of(Domain::Value, prop.digest().as_bytes())
```

A digest of the proposition being claimed. It is computable from the claim, so it
distinguishes nothing and survives no tampering test — there is no independent artifact to
tamper with. `audited_type_check_tree` is the function behind every `brix check`,
`brix why`, and `brix prove` typing result.

It stayed invisible for a structural reason, the same one that motivated ADR-0016: a
`Judgement` carries an `EvidenceId`, and one digest looks like another. It was
`Evidence::SettlementReplay` in name only.

## 2. ADR-0007 §6 already ruled on the outcome

Issue #259 offered two readings — the outcome is wrong, or the support is wrong. The
question is narrower than it looks, because ADR-0007 §6 decided it:

> The "audit" for this slice is **tree well-formedness over real configs** (Seq middles
> match; endpoints are real inference configs, not padded) — the honest analogue of
> `replay_verified`; deep ρ-membership audit is the deferred tight direction.

And ADR-0007 §4 fixes the epistemic scope it goes with:

> `Proven` here asserts the **compositional-validity implication** — "given the leaf
> realizations, the composite holds" — a revision-invariant theorem about the derivation.
> It does **not** claim the settlement outcome is `Proven`; the typing judgement's
> settlement outcome stays `Audited`.

Those checks are not aspirational. `audited_type_check_tree` performs both today —
`tree.well_formed()`, plus `tree.src()`/`tree.dst()` equality against the *real* `expr` and
`final_ty` configs, unpadded. The `Audited` is a considered decision with provenance.

**So the outcome stands and the support is what is broken.** The evidence records none of
the work that was actually done. This ADR gives that work an artifact.

## 3. Why the other reading is not available

Worth stating, because #259's framing — "grades visibly moving down is a fine outcome if it
is the true one" — assumed a downgrade would be a downgrade.

Under ADR-0016 §6 the elaboration boundary requires an `AuditedSource`, which requires
`outcome == Audited`. Publishing `Derived` from `audited_type_check_tree` therefore does not
lower a grade. It:

1. fails in `Judgement::publish` immediately, since `Outcome::Derived`'s sole authority is
   `SettlementKernel` and the call claims `AuditChecker` — `WrongAuthority`, on every call;
2. or, if the authority is changed too, is refused one step later by
   `AuditedSource::verify` with `NotAudited`, so `elaborate_tree` returns `Refused` and
   `check_module` returns `Err`.

Either way **every typing result stops reaching the kernel** and every `let` binding flips
from a graded result to a diagnostic. `brix check` reports `not checked` for everything.
That is not a more honest grade; it is the removal of the typing surface. A ruling that
produced it would have to be justified by the evidence being *absent*, not merely
*unrecorded* — and §2 shows the checks are performed.

## 4. Check parity with the settlement lane

The honest way to state what this `Audited` is worth is to compare it, check by check, with
the one `soc-core::audit::audit_step` performs. That comparison is the substance of this
ADR, and it is why the artifact's verification tag is **not** named `ReplayVerified`.

| # | `audit_step` (settlement) | Typing lane before | After this ADR |
|---|---|---|---|
| a | Log-integrity cross-check of the antecedent `Derived` judgement against the recorded `Observation` | — (no journal, no `CommittedStep`; the typing lane has no logged antecedent) | — (structurally absent, not skipped) |
| b | Endpoint match — chain starts at `src`, ends at `dst` | ✅ `tree.src()`/`tree.dst()` vs real configs; `well_formed()` for Seq middles | ✅ unchanged, now recorded in the artifact |
| c | `registry.contains(g)` for every step | ❌ **absent** | ✅ **added** — every leaf generator must be in the regime's minted set |
| d | `semantics.realizes(g, x_i, x_{i+1})` for every step — the actual ρ check | ❌ absent | ❌ **still absent — deferred, see §7** |
| e | Chain-length invariant | ✅ by construction (tree shape) | ✅ by construction |

Row (c) is closed here because it is closeable: the regime's generator set is already
finite and closed — `generator_name` enumerates it — so membership can be *data* rather than
trust, exactly as `GeneratorRegistry` is on the settlement side.

Row (d) is ADR-0007 §7's explicitly deferred "tight ρ-audit" and ADR-0015 ⟨D-PRIM⟩'s job.
It is not closed here and this ADR does not pretend otherwise.

## 5. Decision

**D1 — A canonical `TreeDerivation` artifact.** The tree derivation becomes a
content-addressed artifact in `brix-semantic`, mirroring `Decomposition`: a verification tag
that is part of the canonical encoding, so a recorded and a verified tree over identical
data have different ids, with frozen append-only ordinals and golden vectors.

```rust
pub enum TreeVerification { Recorded, StructureVerified }   // ordinals 0, 1 — frozen

pub struct TreeDerivation { tree: RealizesTree, verification: TreeVerification }
```

`RealizesTree`/`TreeObj` move from `brix-elaborate` into `brix-semantic` to make this
possible — the artifact must live inside the TCB closures where `Decomposition` lives, and
`brix-semantic` may depend on `brix-canon` only. The kernel projections
(`witness_object`, `to_object_term`) stay in `brix-elaborate`, which is where the
`brix-kernel` dependency belongs, and `brix-elaborate` re-exports the types so no caller's
import path changes.

**D2 — The tag is named for the check it performs.** `StructureVerified`, not
`ReplayVerified`. §4 row (d) is open; a type that said `ReplayVerified` would be the same
class of error as an `Evidence::SettlementReplay` that replays nothing. The name carries the
limit so no future reader has to find a comment.

**D3 — A checker earns the tag.** `audit_tree` performs (b), (c), and (e) of §4 and returns
a typed error otherwise. `TreeVerification::StructureVerified` is reachable only through it.
Publication goes through ADR-0016's fence on a route conditioned on that tag.

**D4 — `Evidence::TreeDerivation`, appended at ordinal 7.** A typing derivation is not a
settlement replay. Reusing `SettlementReplay` is what let the defect hide. Existing ordinals
0–6 are untouched; this is an append, permitted by the same append-only discipline
ADR-0002 §4 used for `Audited`.

### 5.1 What `Audited` means on the typing lane

Normative, and the sentence a future reader should find first:

> An `Audited` typing judgement asserts that a tree derivation for the claimed proposition
> exists, is structurally well-formed, has endpoints equal to the real inference configs,
> and cites only generators in the regime's minted set — and that the judgement's evidence
> is the content-addressed identity of *that* derivation. It does **not** assert that any
> leaf's realization relation ρ_g actually holds. That is ADR-0007 §7's deferred tight
> direction and ADR-0015 ⟨D-PRIM⟩'s mechanism.

## 6. Consequence for ADR-0015

ADR-0015 reasons about whether `Proven` is honest for arithmetic on the premise that a
typing result is *safely capped at `Audited`* (§1, §3, §7 and Stage D). ADR-0016 §0 recorded
that the cap held but what sat under it did not.

After this ADR the premise is sound as written: the cap is `honest_result_outcome`, and what
it caps to is now a judgement bound to a real derivation. **ADR-0015 Stage B0 is unblocked.**

One correction of prose rather than behaviour: `honest_result_outcome`'s doc comment says
the capped result "is the **replay-verified** `Audited`". Nothing replays. It is the
structure-verified `Audited`, and the comment is corrected to say so.

## 7. Residuals — what this does not fix

Stated explicitly so nothing over-reads it.

1. **ρ-membership is still unchecked** (§4 row d). No leaf's realization relation is
   verified against a semantics oracle. `elaborate_tree` admits every leaf to the kernel as
   a *hypothesis*; the kernel proves the composition and never inspects a leaf. This is
   ADR-0007 §7 and PD-1, and it closes with ADR-0015 ⟨D-PRIM⟩'s kernel-owned
   primitive-relation registry — at which point `TreeVerification` gains a third, stronger
   tag rather than this one being redefined.
2. **The flat path has the same class of defect, worse.**
   `audited_type_check` pads its configuration chain to `[src, dst, dst, …]` and calls
   `Decomposition::replay_verified` on it. The evidence is non-circular — it binds to a real
   `Decomposition` — but the artifact it binds to is one an actual audit rejects:
   `test_multi_step_elaboration_tree_vs_linear_tension` already demonstrates that this
   padded chain fails `soc_core::audit_step` under sound generator semantics. Filed
   separately as `spec/errata/0005-flat-path-padded-decomposition.md`; **not fixed here**,
   per #259's own instruction not to expand scope quietly. **Subsequently ruled by
   [ADR-0018](./ADR-0018_Retire_The_Flat_Typing_Lane.md) (#262): the flat lane is retired
   rather than repaired — it had no caller, and the padding survives a downgrade.**
3. **`Decomposition::replay_verified` remains an unchecked stamp** (ADR-0016 §7.1). This ADR
   does not narrow it; that is audit finding A-3 / #178 territory.

## 8. On expiry markers for `Provisional` routes

#259 asked whether ADR-0016's interim `Provisional` route should have carried an expiry —
"a deliberately loud placeholder rather than a quiet one that outlives its reason."

This ruling removes the only such route, so the immediate question dissolves. The mechanism
is still worth having for the next one, and the deterministic form is a **structural
coupling, not a date**: `scripts/check_soc_law_map.py` gains a check that a
`RouteStatus::Provisional` row in `ROUTES` forces SOC-LAW-05 to remain `partial` with a
non-empty `open_issues`. Declaring the law enforced while a named hole is open then fails
CI instead of passing quietly. No clock, no network, no flake.

## 9. Non-goals

- **The frozen ABI, beyond the one declared append.** `Evidence` ordinals 0–6, `Outcome`,
  `DecompVerification`, `Committed`/`Observation`/`Journal`, `F_O`, and the ADR-0013
  envelopes are untouched. Existing vectors are not re-blessed; the new artifact gets its
  own `vectors/tree_derivation_v1.json` per ADR-0013 §7.
- **No grade moves.** `1 + 2` stays `Audited`; `@Proven` on it stays `GradeErasure`;
  tight-leaf bindings stay `Proven`. `crates/brix-lower/tests/lower_proven.rs` must pass
  unchanged, and that is this ADR's headline gate.
- **No calculus, inference, or coverage change.** ADR-0011 Track 2 (sums/matches/coverage)
  is untouched. `infer_tree` is not modified.
- **No new decided negative.** Every new failure path is a typed error; nothing reaches
  `Refuted`.
