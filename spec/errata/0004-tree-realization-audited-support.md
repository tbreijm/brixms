# 0004 — `audited_type_check_tree` publishes `Audited` on evidence derived from its own claim

**Lane:** semantics (brix-semantic / soc-regimes)
**Status:** **ruled 2026-08-08** by [ADR-0017](../adr/ADR-0017_Tree_Realization_Support.md) (filed 2026-08-08; tracked by [#259](https://github.com/tbreijm/brixms/issues/259))
**Affected conformance:** SOC-LAW-05 (authority non-escalation); ADR-0002 §4.1 (verifier-authority table), §5.1/§5.2 (recorded vs replay-verified); ADR-0007 (tree-structured typing derivations)

## Provenance

Found while building the authority publication fence for issue #228
([ADR-0016](../adr/ADR-0016_Authority_Publication_Fence.md)). Enumerating the legal
(authority, outcome, evidence-kind) routes forced every existing `Audited` publisher to
name the artifact standing behind it. Two could. One could not.

Issue #228 says: *"If the work reveals that a currently-legal path is actually unsound,
**stop and report it** rather than quietly fixing it; that would be a finding worth its own
erratum."* This is that erratum. The path is unchanged in code.

## The defect

`crates/soc-regimes/src/type_realization.rs`, `audited_type_check_tree`:

```rust
let prop = Realizes::new(witness_id, expr.config_id(), final_ty.config_id()).proposition_id();
let evidence = Evidence::SettlementReplay {
    body: brix_canon::Digest::of(brix_canon::Domain::Value, prop.digest().as_bytes()),
}
.id();
let audited = Judgement::new(context, prop, Outcome::Audited, evidence);
```

The evidence body is `Digest::of(Domain::Value, prop.digest())` — **a digest of the
proposition being claimed**.

ADR-0002 §4.1 gives the `Audited` row a single authority: "the audit-factorization checker —
the reference replayer, replaying a `Decomposition` against the log", and adds that "the
engine's hot loop may *record* decompositions, never assert their verification." ADR-0002
§5.1/§5.2 make the recorded-vs-replay-verified distinction part of the artifact's canonical
identity precisely so that *"a claim to have recorded a chain is not a claim to have replayed
and verified it."*

Here there is no chain at all. `audited_type_check_tree` returns a `RealizesTree`, not a
`Decomposition`; nothing is replayed, and the evidence that is supposed to record *why* the
claim holds is computable from the claim itself. It therefore:

- distinguishes nothing — every caller who can state the proposition can produce the evidence;
- survives no tampering test, because there is no independent artifact to tamper with;
- is `Evidence::SettlementReplay` in name only, which is how it stayed invisible: the old
  `Judgement` surface took an `EvidenceId`, and one digest looks like another.

The neighbouring `audited_type_check` at least constructs a `Decomposition::replay_verified`
and binds its id into the evidence. This one does not.

## Why it cannot be guessed

Two readings are available and they differ in consequence, so the fix is a ruling, not an
implementation detail:

1. **The outcome is wrong.** A tree-structured typing derivation that nothing replayed is
   `Derived`, and `Audited` requires the replay. Then `audited_type_check_tree` is misnamed
   and `brix-lower`'s `check_module` pipeline currently elaborates from an overclaimed source.
2. **The support is wrong but the outcome is right.** ADR-0007's tree derivation is a genuine
   verified object — `RealizesTree::well_formed()` does check that every `Seq` middle matches —
   and what is missing is an artifact recording it, analogous to `Decomposition` but tree-shaped,
   whose identity would carry a `ReplayVerified`-style tag.

Reading 2 is the more likely intent and the larger job: it needs a canonical tree artifact with
frozen ordinals, which is an ABI addition and squarely ADR-0007/Track 2 work.

## Ruling (adopted 2026-08-08, ADR-0017)

**Reading 2: the outcome stands, the support was wrong.**

ADR-0007 §6 had already decided what the tree "audit" is — "tree well-formedness over real
configs (Seq middles match; endpoints are real inference configs, not padded) — the honest
analogue of `replay_verified`" — and `audited_type_check_tree` performs exactly those
checks. The `Outcome::Audited` was a considered decision with provenance. What was missing
was an artifact recording the work: the evidence named the claim instead of the derivation.

[ADR-0017](../adr/ADR-0017_Tree_Realization_Support.md) therefore:

- adds the canonical `TreeDerivation` artifact in `brix-semantic`, mirroring
  `Decomposition` — verification tag in the canonical encoding, frozen append-only
  ordinals, golden vectors — so a built and a checked derivation over identical trees have
  different ids;
- names the tag **`StructureVerified`, deliberately not `ReplayVerified`**, because
  ρ-membership is still unchecked and a tag claiming otherwise would be this same class of
  defect;
- adds the leaf **generator-membership** check the tree lane was missing and the settlement
  lane has (`registry.contains(g)` in `audit_step`), via `soc-regimes::tree_audit::audit_tree`;
- appends `Evidence::TreeDerivation` at ordinal 7, so a typing derivation is no longer
  encoded as an `Evidence::SettlementReplay` — the naming lie that kept this invisible.

**Reading 1 was not available.** Publishing `Derived` would not have lowered a grade: under
ADR-0016's fence `AuditedSource::verify` requires `Audited`, so `elaborate_tree` would
refuse, `check_module` would return `Err`, and every typing result would stop reaching the
kernel. That is the removal of the typing surface, not a more honest grade — and it would
have contradicted ADR-0007 §6.

**No grade moves.** `1 + 2` is still `Audited`; `@Proven` on it is still `GradeErasure`;
tight-leaf bindings are still `Proven`. `crates/brix-lower/tests/lower_proven.rs` passes
unchanged, which is ADR-0017's headline gate.

### What is still not established

ρ-membership. No leaf's realization relation is verified against a semantics oracle;
`elaborate_tree` admits every leaf to the kernel as a hypothesis. This is ADR-0007 §7's
deferred tight direction and closes with ADR-0015 ⟨D-PRIM⟩, at which point
`TreeVerification` gains a third, stronger tag rather than this one being redefined.

A separate defect on the sibling flat path is filed as
[`0005-flat-path-padded-decomposition.md`](./0005-flat-path-padded-decomposition.md) /
[#262](https://github.com/tbreijm/brixms/issues/262).

## Superseded interim disposition (ADR-0016 §7)

Retained for the record. Between #228 and this ruling, the path had an explicitly named row
in the route table:

```rust
Route { authority: AuditChecker, outcome: Audited,
        support: SupportKind::TreeRealization, status: RouteStatus::Provisional }
```

`RouteStatus::Provisional` meant: this route exists so the workspace compiles and behaves
exactly as it did, and it is **not** blessed. **That row is now retired** — ADR-0017
replaced it with a `Settled` route conditioned on `TreeVerification::StructureVerified`,
and no `Provisional` route remains in `ROUTES`.

## Conformance IDs affected

- **SOC-LAW-05** (authority non-escalation) — was held at `partial` while a `Provisional`
  route remained in `ROUTES`. That route is retired, so the law moves to `enforced`, and
  `scripts/check_soc_law_map.py` now *couples* the two: a `Provisional` row forces the law
  back to `partial` with an open issue, so the next such hole cannot be declared closed
  quietly (ADR-0017 §8).
- ADR-0002 §4.1 `Audited` row — the sole-authority claim now holds structurally for both
  the `soc-core::audit` route and this one, each conditioned on a verification tag its own
  checker earns.
- **ADR-0015 (judgment-scoped tightness)** — its argument that `Proven` is honest for
  arithmetic rests on the premise that a typing result is safely *capped at `Audited`*.
  Before this ruling the cap held but what sat under it did not, and
  `audited_type_check_tree` is the function behind every `brix check`, `brix why`, and
  `brix prove` typing result, so the weakness was not confined to a corner of the surface.
  **Resolved:** the cap now caps to a judgement bound to a real, checked derivation, so the
  premise is sound as written and ADR-0015 Stage B0 is unblocked.

## Implementation alignment

- `crates/brix-semantic/src/publication.rs` — `Support::Tree`, `SupportKind::Tree`,
  `RouteCondition::Tree`, `PublicationError::TreeVerificationMismatch`, and the now-`Settled`
  `ROUTES` row that replaced the `Provisional` one.
- `crates/brix-semantic/src/tree.rs` — `RealizesTree`/`TreeObj` (moved from
  `brix-elaborate`), `TreeVerification`, `TreeDerivation`, `TreeDerivationId`.
- `crates/soc-regimes/src/tree_audit.rs` — `audit_tree`, the checker that earns the tag.
- `crates/soc-regimes/src/type_realization.rs` — `audited_type_check_tree` publishes with
  `Support::Tree(&derivation)` and returns the artifact. No calculus, coverage, or
  inference change.
- `vectors/tree_derivation_v1.json` — the new artifact's frozen vectors (ADR-0013 §7).
- `docs/audit/issue-63/README.md` — finding A-3's neighbourhood; this erratum is the
  bounded write-up A-3 asked for on the tree route specifically.
