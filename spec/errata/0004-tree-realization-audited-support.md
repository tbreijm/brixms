# 0004 — `audited_type_check_tree` publishes `Audited` on evidence derived from its own claim

**Lane:** semantics (brix-semantic / soc-regimes)
**Status:** filed 2026-08-08 — **open**, awaiting ruling (tracked by [#259](https://github.com/tbreijm/brixms/issues/259))
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

## Interim disposition (not a ruling)

ADR-0016 §7 gives the path an explicitly named row in the route table:

```rust
Route { authority: AuditChecker, outcome: Audited,
        support: SupportKind::TreeRealization, status: RouteStatus::Provisional }
```

`RouteStatus::Provisional` means: this route exists so the workspace compiles and behaves
exactly as it did, and it is **not** blessed. Behaviour is unchanged — no judgement that was
produced before is refused now, and no `JudgementId` moves. The change is that the hole has a
name and a table row instead of being invisible inside a general-purpose constructor.

## Conformance IDs affected

- **SOC-LAW-05** (authority non-escalation) — stays short of fully `enforced` while a
  `Provisional` route remains in `ROUTES`.
- ADR-0002 §4.1 `Audited` row — the sole-authority claim holds structurally for the
  `soc-core::audit` route and provisionally for this one.
- **ADR-0015 (judgment-scoped tightness)** — its argument that `Proven` is honest for
  arithmetic rests on the premise that a typing result is safely *capped at `Audited`*.
  The cap holds; what sits under it does not, on this path. `audited_type_check_tree` is
  the function behind every `brix check`, `brix why`, and `brix prove` typing result, so
  the weakness is not confined to a corner of the surface. This does not invalidate
  ADR-0015's reasoning — it means one of its stated premises is currently supported more
  weakly than written, and it resolves when this erratum does.

## Implementation alignment

- `crates/brix-semantic/src/publication.rs` — `SupportKind::TreeRealization`,
  `RouteStatus::Provisional`, and the `ROUTES` row, each commented back to this erratum.
- `crates/soc-regimes/src/type_realization.rs` — `audited_type_check_tree` migrated to
  `Judgement::publish(Authority::AuditChecker, …, Support::TreeRealization { body })`.
  Mechanical; no calculus, coverage, or inference change.
- `docs/audit/issue-63/README.md` — finding A-3's neighbourhood; this erratum is the
  bounded write-up A-3 asked for on the tree route specifically.
