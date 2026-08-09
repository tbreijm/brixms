# ADR-0019 — Verification Tags Are Earned, Not Stamped

Status: **Accepted** (2026-08-09) (rules on the ADR-0016 §7.1 residual / audit finding A-3 / the verification-tag slice of #178; follows [ADR-0017](./ADR-0017_Tree_Realization_Support.md) and [ADR-0018](./ADR-0018_Retire_The_Flat_Typing_Lane.md)).

Date: 2026-08-09.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4.1 verifier authority, §5.1/§5.2 recorded versus replay-verified evidence, §6 `Decomposition`), [ADR-0007: Tree-Structured Typing Elaborations](./ADR-0007_Tree_Structured_Typing_Elaboration.md) (§6 tree audit, §7 deferred ρ-audit), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md), [ADR-0016: The Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md) (§7.1 the residual ruled here), [ADR-0017: Tree-Realization Support](./ADR-0017_Tree_Realization_Support.md), and [ADR-0018: Retire the Flat Typing Lane](./ADR-0018_Retire_The_Flat_Typing_Lane.md).

This ADR changes who may construct verified canonical artifacts. It changes no proof rule, calculus, inference result, grade, canonical encoding, ordinal, or artifact id.

---

## 1. The finding

ADR-0016 §7.1 states that `Decomposition::replay_verified` does not replay anything. It stamps `ReplayVerified` on whatever generator and configuration vectors it is given. That is correct but incomplete.

The hole has two doors.

First, all three `Decomposition` fields are public:

```rust
pub struct Decomposition {
    pub generators: Vec<GeneratorId>,
    pub configs: Vec<ConfigId>,
    pub verification: DecompVerification,
}
```

A caller can therefore mint the claim without using `replay_verified`:

```rust
let mut d = Decomposition::recorded(generators, configs)?;
d.verification = DecompVerification::ReplayVerified;
```

Sealing only the constructor does not close this.

Second, ADR-0017 reproduced the constructor pattern for its new artifact. `TreeDerivation` correctly keeps its fields private, but `TreeDerivation::structure_verified` is public and performs no check. Its documentation explicitly delegates honesty to the caller.

There are therefore two instances of the same structural defect:

> A verification tag that contributes to a canonical artifact’s identity can be selected by a caller that has not produced the evidence the tag denotes.

Nothing in-tree currently assigns `Decomposition::verification` directly. That does not make the route unreachable. The soundness boundary is the API, not the current call graph.

## 2. The tag’s honest scope

The four steps in `soc_core::audit::audit_step` do not all describe the same fact.

1. Reconstructing the antecedent `Derived` judgement and matching it against the recorded `Observation` is contextual. It depends on the journal entry and `CommittedStep`.
2. Matching the decomposition endpoints against `step.src` and `step.dst` is contextual. It relates the artifact to one committed step.
3. Checking every generator against `GeneratorRegistry` and every link with `GeneratorSemantics::realizes` is intrinsic to the chain relative to the supplied registry and semantics.
4. Publishing the new `Audited` judgement is the consequence of the checks. It is not another property for the tag to attest.

`DecompVerification::ReplayVerified` shall attest to step 3, together with the chain-length invariant already enforced at recorded construction. It shall not attest to steps 1, 2, or 4.

That boundary is deliberate. A `ReplayVerified` decomposition canonically encodes generators, configurations, and the verification tag. It does not encode a journal entry, context, observation, or committed-step identity. Claiming that its frozen id attests to those absent values would dress an identity limitation up as proof depth.

`audit_step` remains responsible for steps 1 and 2 on every publication. It may publish only after the intrinsic transition in step 3 returns a verified artifact.

For `TreeDerivation`, ADR-0017 already defines the narrower claim. `StructureVerified` attests to:

- structural well-formedness;
- endpoints equal to the independently supplied claim endpoints;
- membership of every leaf generator in the independently supplied registry; and
- the chain-shape invariant.

It still does not attest to any leaf relation `ρ_g`.

## 3. Decision

**D1 — Verified tags are outputs, never constructor inputs.** For every canonical artifact whose verification tag contributes to its identity:

- artifact fields carrying identity material are private;
- recorded construction remains public;
- no public struct literal, setter, raw verified constructor, or constructor parameter accepts a verified tag;
- the owning crate exposes a checked transition from the recorded form to the verified form; and
- failure returns a typed error and produces no verified artifact.

`DecompVerification` and `TreeVerification` may remain public for inspection and diagnostics. Naming a variant is not authority to install it in an artifact.

This is the general rule. The implementation is specific to the two current artifacts; there will be no generic `Verified<T>` or generic capability framework. The two tags make different claims and need different inputs and errors. A generic wrapper would not make either check more sound.

**D2 — `Decomposition` becomes opaque and read-only outside `brix-semantic`.** Its `generators`, `configs`, and `verification` fields become private. Read-only accessors replace direct reads:

```rust
pub fn generators(&self) -> &[GeneratorId];
pub fn configs(&self) -> &[ConfigId];
pub const fn verification(&self) -> DecompVerification;
```

`Decomposition::recorded` remains the only public raw constructor.

`Decomposition::replay_verified` is removed. A consuming checked transition replaces it:

```rust
pub fn verify_replay(
    self,
    registry: &GeneratorRegistry,
    semantics: &dyn GeneratorSemantics,
) -> Result<Self, ReplayVerificationError>;
```

The transition requires `Recorded`, checks every generator for registry membership, calls `semantics.realizes` for every adjacent configuration pair, and only then constructs the same data with `ReplayVerified`.

Consuming `self` makes the state transition plain. Cloning an already earned verified artifact remains valid; re-verifying a verified artifact is refused.

**D3 — `GeneratorSemantics` moves down, and nothing else does.** `GeneratorSemantics` moves from `soc-core::audit` to `brix-semantic`, beside `GeneratorId` and `GeneratorRegistry`. `soc-core::audit` re-exports it so existing implementation paths need not change.

This is not a dependency inversion. `brix-semantic` still depends only on `brix-canon`; no `Cargo.toml` edge changes. The moved trait mentions only `GeneratorId` and `ConfigId`, both already owned by `brix-semantic`. Journal types, `CommittedStep`, `AuditResult`, audit orchestration, and publication remain in `soc-core`.

The trait’s conceptual split is:

- `GeneratorSemantics` states the primitive relation over canonical semantic identities;
- `soc-core::audit` owns settlement replay in journal context.

The latter remains the audit checker. Moving the relation interface does not move the settlement checker.

**D4 — `audit_step` delegates the intrinsic check; it does not check and then stamp.** The first two `audit_step` stages remain in their present order. Stage 3 becomes a call to `Decomposition::verify_replay` on a clone of the recorded artifact. Its typed errors map to the existing fail-closed `AuditResult::Unknown` reasons. Stage 4 publishes only with the returned artifact.

There must be no duplicated pattern of:

```text
check in soc-core
then call an unchecked constructor in brix-semantic
```

The code that sets `ReplayVerified` is the code that walks every link.

This preserves the working:

```text
commit_tick → audit_step → elaborate_decomposition
```

path. A failed transition yields `Unknown`; it does not yield a recorded substitute, `Audited`, or `Refuted`.

**D5 — `TreeDerivation::structure_verified` is removed and replaced by a checked transition.** `TreeDerivation::recorded` remains public. The replacement consumes a recorded derivation and requires independent expected endpoints and a `GeneratorRegistry`:

```rust
pub fn verify_structure(
    self,
    expected_src: &TreeObj,
    expected_dst: &TreeObj,
    registry: &GeneratorRegistry,
) -> Result<Self, TreeVerificationError>;
```

The transition performs ADR-0017 §4 rows (b), (c), and (e) itself before setting `StructureVerified`.

`soc-regimes::tree_audit::audit_tree` remains the regime-facing checker. It builds the typing registry from the regime’s independently declared generator enumeration, calls the semantic transition, and maps its typed errors to `TreeAuditError`.

The registry must not be assembled from the candidate tree’s leaves. Doing that would turn membership into “every cited generator is among the cited generators,” which checks nothing. The existing `generator_name` enumeration, including the numeric and grade lattice edges, becomes the single source used both for reverse naming and for the typing registry.

**D6 — No authority token and no proof-shaped stamp.** An opaque token does not solve this by itself. If its constructor is public and unchecked, the hole moves into the token. If its constructor performs the checks, it is the checked transition ruled above with another type in the middle.

Rust crate visibility also cannot express “constructible by this downstream crate, but by no other downstream crate.” `pub(crate)` in `brix-semantic` excludes `soc-core`; `pub` admits everyone. Moving the artifacts upward to exploit `pub(crate)` would either create a dependency cycle through `publication.rs` or move canonical evidence outside the TCB crate where ADR-0017 deliberately placed it.

Per-link witness values have the same requirement: unless their constructors execute the relation check, they are assertions. If their constructors execute it, the direct checked transition is simpler.

**D7 — Tests use the real transitions. There is no `test-support` escape hatch.** The existing test callers legitimately need verified artifacts. They shall construct a recorded artifact, supply a fixture registry and fixture semantics or expected endpoints, and run the same checked transition production uses.

Tests that currently require an impossible state — for example a malformed tree carrying `StructureVerified` — move to the verifier boundary and assert that the state cannot be produced. Publication and elaboration tests continue to use valid verified artifacts when exercising evidence binding and route conditions.

No public `test-support` feature is added. A non-default feature would still be an advertised way for a downstream build to restore the unchecked constructor. The tests do not require it.

Private `#[cfg(test)]` helpers inside the defining module are permissible only if needed for a local implementation invariant; they are not exported, are unavailable to integration tests, and are absent from a normal library build. This ruling does not presently require one.

**D8 — The canonical encoding is frozen and unchanged.** The following remain byte-for-byte identical:

```text
DecompVerification::Recorded       = 0
DecompVerification::ReplayVerified = 1

TreeVerification::Recorded          = 0
TreeVerification::StructureVerified = 1
```

The canonical field order and field values do not change. The checked transitions produce the same verified value that the old constructors produced after honest checks. Therefore every existing `DecompositionId` and `TreeDerivationId` remains unchanged.

`vectors/canon_vectors.json` and `vectors/tree_derivation_v1.json` are not edited or re-blessed. Their tests are rewritten only to obtain the verified value through a real transition before comparing the same bytes.

Making Rust fields private is a source-API break. It is not a canonical ABI change.

**D9 — This closes the named tag-minting residual, not #178 as a whole.** ADR-0016 §7.1 is closed: neither verification tag can be assigned or selected without executing its defining checks.

For the conformance map:

- SOC-LAW-03 remains `enforced` and removes issue 178 from `open_issues`.
- SOC-LAW-04 moves from `partial` to `enforced` and removes issue 178. In the current map this tag-minting residual is its only bounded open issue.
- SOC-LAW-05 remains `enforced` and removes issue 178 from `open_issues`.

ADR-0019 and its new negative gates are added as anchors for those rows. Existing gate-test names are not changed.

Issue #178 remains open elsewhere. In particular it still tracks work such as primitive typing relation discharge and other integration residuals recorded against SOC-LAW-02, SOC-LAW-08, SOC-LAW-09, SOC-LAW-10, SOC-LAW-11, and SOC-LAW-12. This ADR closes only the use of #178 as shorthand for unchecked verification-tag minting.

## 4. Why the other placements are rejected

**Move `Decomposition` to `soc-core`.** Rejected. `Decomposition` is canonical evidence consumed by `brix-semantic::publication`, and `soc-core` already depends on `brix-semantic`. Moving it upward either creates a cycle or requires moving the publication fence and evidence vocabulary with it. That is a TCB rearrangement to obtain crate-private syntax, not a soundness improvement.

**Move the whole audit checker to `brix-semantic`.** Rejected. It would require settlement journal concepts or an unstructured bundle of their fields in the canonical-artifact crate. `brix-semantic` is the right home for validating an artifact’s intrinsic transition, not for interpreting a `CommittedStep`.

**Leave the checks in the upper crate and pass a boolean or token.** Rejected. A public boolean is the existing stamp with a different spelling. A public token constructor has the same defect. A private token constructor cannot be called across the existing crate boundary.

**Bind registry, semantics, journal, and context into the verified artifact.** Not done. That would change what the artifact identifies and therefore change every verified artifact id. The honest form of that design is a new, versioned additive artifact or audit receipt with its own canonical encoding and vectors. It is not needed to close direct tag minting and is forbidden as an in-place change by the frozen ABI.

## 5. Consequences

The verification tag is no longer evidence of caller discipline. It is evidence that code in the artifact-owning crate executed the transition’s checks.

`audit_step` remains the only production settlement caller and preserves all four of its current stages. The stage-3 loop moves behind the transition that sets the tag; stages 1 and 2 do not move.

`audit_tree` remains the only production tree caller. ADR-0017’s outcome and epistemic scope do not change.

All external reads of `Decomposition` migrate to accessors. This affects `soc-core`, `brix-elaborate`, `brix-lower`, and tests, but does not change their values or behaviour.

The old unchecked constructors and direct field assignment become compile errors. A relation failure, an unregistered generator, a malformed tree, or an endpoint mismatch returns an error without producing a verified artifact.

No grade moves. No failure produces `Refuted`.

No dependency edge changes. `scripts/check_tcb_dependencies.py --write` is therefore not required; `--check` must remain green.

## 6. Residuals — what this does not fix

1. **A supplied semantics can lie.** `GeneratorSemantics` remains a trait supplied by the caller. An implementation that returns `true` for everything can make a chain pass relative to that implementation. This ADR guarantees that the predicate was executed, not that the oracle was independently authenticated. The present settlement design already trusts the supplied semantics in exactly this way.

2. **The verified id does not name the semantics or registry.** Identical chains verified under two implementations still have the same `DecompositionId`. Changing that requires a new versioned artifact or canonical audit receipt. It cannot be smuggled into the frozen encoding.

3. **`ReplayVerified` does not attest to journal integrity or committed-step endpoint agreement.** Those checks remain contextual obligations of `audit_step`. The artifact tag must not be read as a canonical receipt for a journal it does not encode.

4. **The public publication surface is not caller authentication.** `Judgement::publish` validates a route, support kind, tag, and evidence binding. It does not prove that the Rust caller is literally the `soc-core::audit` module. This ADR does not redesign authority as an unforgeable runtime capability.

5. **Tree leaf relations remain unchecked.** `StructureVerified` still does not mean any `ρ_g` holds. ADR-0007 §7 and ADR-0015 ⟨D-PRIM⟩ remain the owners of that work.

These are real limits. This ADR closes “the tag can simply be assigned or requested,” not “every verifier input is itself certified.”

**Residuals 1–3 are ruled by [ADR-0020](./ADR-0020_Oracle_Bound_Audit_Receipts.md).** The
open `GeneratorSemantics` trait leaves the production and verification boundary and becomes
canonical declared data (`GeneratorSemanticsV1`, with a content identity), and a
`SettlementAuditReceiptV1` binds the context, the committed step, the earned
`DecompositionId`, the registry id and the semantics id — checked by replay, never trusted
as a record. That is viable because both production oracles were already data-shaped: L3 is
a lookup in an immutable transition table, and literal equality is the diagonal.

Two consequences worth reading here rather than only there. **§6 residual 1 is narrowed, not
eliminated** — content addressing makes a substituted oracle *detectable*, not the declared
rows *correct*; a consumer must independently know which semantics id to expect, and one
that adopts the id shipped alongside the receipt has authenticated nothing. And the test
above, `an_always_true_semantics_still_passes_a_fabricated_chain`, is **consciously
superseded**: ADR-0020 D9 replaces it with two negatives — arbitrary code can no longer
implement an executable oracle at all, and a fabricated manifest earns a *different*
semantics id that fails against the expected one.

## 7. Non-goals

- **No canonical encoding or vector change.** No ordinal, field order, `DecompositionId`, `TreeDerivationId`, or file in `vectors/` changes.
- **No new artifact version.** A semantics-, registry-, context-, or journal-bound receipt is not introduced.
- **No inference or calculus change.**
- **No grade change.**
- **No settlement-lane behaviour change.**
- **No typing ρ-audit.**
- **No general proof-object or capability framework for two concrete transitions.**
- **No `test-support` feature.**
- **No decided negative.** Failures produce typed errors, `Unknown`, or the existing typing refusal; never `Refuted`.

