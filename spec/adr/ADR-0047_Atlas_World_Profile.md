# ADR-0047: Atlas world profile and alpha.6 persisted state

Status: release candidate contract, 2026-10-13.

This records the R1–R4 implementation requested for alpha.6, on the alpha.5
persistent runtime. It extends ADR-0046's acyclic world profile and retains
ADR-0045's exact numeric semantics and the SOC authority boundaries.

## Language contract

1. Nominal records and variants are admitted world values. Names, field types,
   constructor arity and payloads are validated; identity includes the nominal
   declaration. Tuple transport, persisted state, decisions, explanations,
   history and audit preserve that identity.
2. A decision exposes one winner row per settled entity. The row carries its
   entity key, candidate, priority and typed value. Relations and later decisions
   may depend on it. The combined dependency graph must be acyclic; a batch
   publishes all affected stages atomically. Retraction removes only unsupported
   conclusions. Runtime winner rows remain `Derived`.
3. Grouped `sum` admits exact integers and decimals, with deterministic checked
   output. `min`/`max` retain supported ordered values through retraction.
   Intermediate scheduling and chunk boundaries do not decide whether an exact
   final result overflows. Ordinary empty groups produce no tuple.
4. `not exists` admits an indexed correlated equality over the acyclic graph.
   Absence is relative to this world revision, not a claim of universal falsity.
   `count_or_zero()` is an explicit parent-preserving operation: one keyed input
   parent, one child, one equality over the complete single-field parent key,
   that grouping key, and exactly one key/count projection pair. Unsupported
   shapes fail before execution.

The syntax and current admission limits are documented in
[`docs/world-profile.md`](../../docs/world-profile.md).

## Persisted identities and cache upgrade

Alpha.6 uses `brix.world.state-value@3`, `brix.world.network-state@2`, and
`brix.world.net-lowering@2`. The storage value codec adds structured values;
the header and lowering versions distinguish the new operator state from
alpha.5. Scalar value bytes are unchanged. Structured decoding checks depth,
container length, cumulative nodes, field ordering and complete consumption.

A well-formed previous-version cache is stale and is rebuilt from the bound
program and source facts. Missing or damaged state is not an invitation to
accept an alternative answer. Journal history, source identity and independently
supplied audit pins remain authoritative; caches never mint audit evidence.

Revision digests bind network-state roots. Therefore new alpha.6 revisions may
have different digests from alpha.5 even for the same scalar workload. Existing
historical revision records are not rewritten. The three chunked-persistence
goldens that include revision digests must be recorded as alpha.6 vectors,
separately from the historical pre-CPD claim. Their update is authorized by the
explicit new state/lowering identifiers here, not by a failing test alone.
Decision-delta bytes and settled scalar values must still agree with alpha.5
for those unchanged programs.

## Release obligations

- Every public batch route uses the same decision cascade, with one cumulative
  resource budget for the entire atomic operation.
- Operator indexes, aggregate state and typed values survive cold restore.
- Removing the last parent or child produces the correct absence transition.
- Incremental updates remain local; unrelated rows do not cause scans, and
  maintaining extrema does not serialize or scan an entire group per edit.
- Independent replay reproduces decisions and derived relations. Tampering,
  malformed state and exhausted budgets fail closed.
- The CLI/stdio workflow, Python bindings, canonical/dependency gates and release
  archive smoke pass at the actual alpha.6 version. Record the extent of scale,
  crash and older-world qualification without extrapolating beyond the runs.

This contract does not extend the beta compatibility promise to all world
interfaces, add general recursion, implement workflow grounding, or claim that
external observations are true merely because their processing verifies.
