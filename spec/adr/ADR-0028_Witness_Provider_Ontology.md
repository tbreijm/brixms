# ADR-0028 — Witness Provider Ontology Correction

Status: **Proposed** (2026-08-18).

## Decision

SOC's semantic candidate is exactly `(witness, successor)` in an execution
configuration `(world, policy, history)`. A candidate therefore carries no
discovery-provider identity. A selected candidate realizes `x —w→ y`; the
calendar alone turns that possibility into the committed successor/history and
publishes `Derived`.

`WitnessProvider` is the deliberately non-ontological discovery interface.
It may enumerate and index possibilities, but creates no semantic authority
and is not part of candidate identity, the `Realizes` judgement, canonical
candidate keys, journal identity, or audit evidence. `IncrementalWitnessIndex`
is its delta-driven counterpart. A certificate MAY bind the fixed provider
*presentation* only to scope an enumeration-completeness claim; that binding is
presentation metadata, never candidate meaning or authority. When duplicate
providers present the same `(witness, successor)`, the frontier deduplicates
it; the first provider in the fixed presentation supplies the recorded
decomposition. That ordering is likewise presentation metadata.

`RegimeId`'s semantic meaning is exclusively the provenance tag on
`brix_semantic::Witness`: it names the interpretation `ρ_w` under which that
witness is meaningful. Existing profiles may carry its digest as
compatibility/presentation metadata, but it does not identify a provider and
is not a field of `soc_core::Candidate`.
`AdmWitnessAllowlist` governs candidates by witness identity rather than by
provider identity.

## Compatibility and supersession

This ADR supersedes only the API wording in ADR-0002 §7 and the implementation
names inherited by ADR-0012/0014. It does not reinterpret frozen canonical
bytes: existing `regime_set` certificate/presentation fields retain their
wire names and bytes as presentation metadata, and existing `RegimeId` values
retain their witness-interpretation meaning. A future encoding that changes
those bytes requires a versioned certificate/profile migration.

ADR-0012's v1 policy envelope retains its `RegimeId` provenance field. Its
runtime filter is now the equivalent finite set of plan witness handles, so
the closed v1 plan admits the same witnesses without giving provider identity
semantic force.
