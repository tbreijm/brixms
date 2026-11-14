# Alpha.9 development: scoped requests, relation composition, and bounded collections

**Status (2026-11-10): release candidate, native qualification complete.**
The reviewed implementation covers A9-1 through A9-7. All 30 scale runs and
the native macOS arm64 validation gates passed on source commit
`12e37f8b40f2137516e6c9ede60c7431d942fdd5`. No `v0.1.0-alpha.9` tag or
release archive has been published. The installation instructions in the
top-level README therefore continue to describe alpha.8. The workspace
candidate version is `0.1.0-alpha.9` and the Python package version is
`0.1.0a9`. Linux and cross-platform CI remain separate from native validation.

Alpha.9 builds on the reviewed #52 resolution, #53 typing and #59 context
foundation and on the alpha.8 persistent world runtime. Those foundations
provide bounded APIs and validators for the supported profiles; they do not
establish general completeness, unrestricted context transport, or theorem
proofs for every runtime result. A world winner remains a runtime settlement.

## New capabilities

### Persistent relations and decision keys

- Large grouped `collect` state survives fresh-process restore and continues to
  support bounded insertion, replacement, retraction and overflow rejection.
  An overflow failure does not publish a new HEAD.
- Plain `union` retains its disjoint-key behavior. Explicit `union distinct`
  combines schema-compatible sources by canonical typed row value, preserving
  support multiplicity and retracting a row only after its final support is
  removed.
- Decision keys can be ordered composite tuples while existing scalar-key
  identities remain stable. Equality joins on admitted nested field paths use
  maintained indexes and are checked against recomputation in the covered
  cases.

### Typed scoped named queries

Programs can declare typed named queries. The initial profile supports `Str`,
`Int` and `Bool` parameters and inferred result schemas; arguments are typed
data rather than interpolated source. Missing, extra and incorrectly typed
arguments fail before execution.

The CLI exposes `brix world query-named`. The read request combines the query,
typed arguments, revision, page limit and optional continuation cursor with a
separately selected host policy. The host policy authorizes partition fields or
explicit global relations and sets page, scan, work and output bounds. Filters
are applied to base relations before joins, groups and absence evaluation. The
initial profile rejects decision dependencies rather than exposing globally
selected winners as tenant-local results.

HEAD is resolved once for the first page. Bound cursors continue at the same
revision and digest if HEAD advances; changing query, arguments, scope, context,
world, revision or ordering binding invalidates the cursor. Reads use an
ephemeral scoped network and do not append facts, revisions, settlements,
receipts or persistent caches. `serve --stdio` requires explicit trusted-host
opt-in for this method. The embedding host is responsible for authenticating
the caller and selecting the host policy. Python exposes an immutable
`HostQueryPolicy` and `BrixClient.world_query_named` for that boundary.

The named-query profile is deliberately limited. It supports only declared
partition mappings and explicitly approved global relations, and rejects
unsupported transitive dependencies. Physical rows scanned count toward the
read cap even if they are filtered out before evaluation.

### Per-request evidence context

Finite-decision `run`, `why`/`whynot`, `audit` and `verify` can receive an
immutable host-selected `brix.request-evidence-policy@1`. It pins the actual
program, runtime claim identities, optional documentary snapshot, supported
verifier/profile, authorization dimensions and resource limits. Evidence
bytes are separate from those expected pins. The actual source-derived program
and V2 context are checked before evidence is admitted; missing, mismatched,
unsupported or over-budget evidence fails closed.

The CLI accepts `--request-context FILE`; Python exposes immutable
`RequestEvidencePolicy` values per operation on `BrixClient(trusted_host=True)`.
Default stdio requests do not inherit ambient environment pins. Standalone CLI
compatibility snapshots its legacy environment pins once per invocation.
Host-context portable audits use the additive
`brix.audit-request-evidence@1` wrapper while retaining their embedded V1 audit
bytes. Legacy audit identities are not relabeled as V2 contexts. This policy
covers the finite-decision request profile; it does not replace the separate
L3-v1 or world decision-evidence verification paths.

### String admission and collection ordering

- Unsupported `Str + Str` and `Str < Str` operations are rejected during
  finite-decision and world admission, including covered helper, nested
  expression and named-query paths. Integer arithmetic/order and string
  equality/inequality remain supported. A bounded string library and lexical
  search are not included.
- Existing full `collect(value, sort_key, limit)` remains an atomic bounded
  operation returning a list. The additive
  `collect(value[, sort_key]).take(n)` projection returns the structural record
  `{ items: List<T>, truncated: Bool }`; `n` may be zero. It preserves child
  multiplicity and the ordered tail needed to promote a child after retraction.
  Truncation does not bypass independent input, work or storage limits.
- New materialized-collection worlds record the typed child-key evaluator
  profile. Saved alpha.8 worlds keep their legacy evaluator and ordering; they
  are not silently migrated. The new `.take` form is rejected under that legacy
  evaluator. Portable verification follows the profile recorded by the
  producing world or audit.

## Compatibility

The syntax, wire forms, cursor ordering and evaluator additions are versioned.
Existing alpha.8 canonical program identities, frozen vectors, scalar decision
keys and audit encodings are preserved where semantics are unchanged. New
collection ordering is bound to the new evaluator/profile; legacy worlds and
artifacts retain their original semantics rather than being rewritten in
place. Host-context audits add a wrapper around unchanged V1 audit bytes.

This development branch does not claim that every historical alpha.8 world
has an automatic migration path. Incompatible or corrupt persisted state
continues to fail closed; the runtime does not reinterpret corruption as an
empty collection.

## Qualification and performance

The A9-7 fixture and capture design are documented in
[`alpha9-scale.md`](../performance/alpha9-scale.md). The fixture is a
deterministic **public synthetic Atlas-shaped workload**, not the Atlas model
or an Atlas dataset. No direct performance comparison with Atlas's reported
alpha.8 numbers is claimed. All five repetitions at 8k, 80k and 800k facts
for both one- and ten-batch profiles passed correctness checks, and each
ten-revision logical-storage budget passed. The report publishes medians,
ranges, storage accounting, memory, query timings and raw records. At 800k,
ten-batch load took a median 70.42 seconds and retained 1,314,944,760 logical
bytes against a 9,847,177,216-byte budget. Full-result derived pagination
rescans the relation on each page; returning all 80,000 fulfillment rows took
a median 55.65 seconds. This is a documented performance limit.

Final native validation passed 2,276 workspace tests and 13 doctests, all 16
release collection cases including the 5,000-child lifecycle, 44 Python SDK
tests, and 11 benchmark capture/report tests. Formatting, workspace all-target
Clippy with warnings denied, TCB inventory, SOC law mapping, 69 independent
canonical vectors and dependency advisory/license checks passed. The extracted
macOS arm64 package passed all ten packaging smoke stages and the Atlas-shaped
CLI workflow. Debug testing skipped only the separately qualified 5,000-child
case and 28 pre-existing ignored cases. Legacy frozen histories and audits
retain their expected identities. Downstream parity against actual Atlas data
and cross-platform release publication remain consumer/release steps.

The normative delivery scope and acceptance requirements remain in the
[Alpha.9 release contract](../../spec/Alpha9_Release_Contract.md). The
implementation notes for [scoped queries](./a9-scoped-queries.md),
[request evidence](./a9-request-evidence.md) and
[collection ordering and truncation](./a9-collect-order-and-take.md) describe
the initial supported profiles and their limits.
