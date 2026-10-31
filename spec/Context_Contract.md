# Context Contract: Materialized Contexts, Identity Correspondence, and Validation

Status: **Proposed** (2026-10-27). Governs Issue [#59](https://github.com/tbreijm/brixms/issues/59).

Implementation scope (2026-10-30): §§2–8 describe the V1 structural envelope
and its compatibility limits. **§9 defines the additive V2 model used by the
new production consumers**, including owner-checked typed assumptions, referenced
artifact availability/digest checks, explicit dimensions, projection receipts,
and confinement. V1 is not silently promoted into a semantically checked V2
context. Legacy root and extension identities remain unchanged.

The format and consumer profile decisions below are explicit; general cross-authority or
cross-world transport remains prohibited, not automatically proved. Evidence
pins refer to named executable tests, with verification status recorded in §12.

---

## 1. Motivation and Problem Statement

In BrixMS, every deduction, typing derivation, and proof check is evaluated relative to an
assumption context represented by a `ContextId`. As defined in ADR-0001 §5.1 and ADR-0002 §6.1,
a context formally embodies:
$$\text{Context} = \text{World/Snapshot} \times \text{Program-Revision} \times \text{Assumptions} \times \text{Profile} \times \text{Resource-Limits}$$

Historically:
1. `ContextId` was introduced as an opaque 32-byte content-addressed identity (`Domain::Value`),
   anchored to a frozen legacy root digest (`ScopeId::root()` from `reflect.rs`).
2. Later (ADR-0028 ⟨D-CTXPUBLISH⟩), `ContextId::extend` enabled basic cryptographic chaining of
   parent contexts and assumption byte sequences.
3. However, **no materialized contract or wire format existed for context contents**. An ID
   alone does not establish that context contents are available, syntactically well-formed,
   bounded in resource consumption, structurally valid, or authorized for transport across
   trust domains.
4. The proof kernel's context check (`term.context == *context`) merely verifies digest equality;
   it does not authenticate or validate the context contents themselves.

This specification describes the implemented slice and future obligations:
- The canonical, versioned envelope for materialized context payloads.
- Strict correspondence rules between materialized context payloads and `ContextId`.
- An exhaustive inventory of context dimensions, their presence/omission semantics, and representation.
- Bounded, fail-closed decoding and validation disciplines preventing denial-of-service,
  circular references, authority escalation, and simulation/production confusion.
- The interface agreement enabling resolver (#52) and typing/kernel (#53, #56) consumers to
  safely require context validation.

---

## 2. Envelope Specification and Identity Correspondence

### 2.1 Domain and Envelope Markers

Materialized context payloads are canonically encoded using **`brix-canon`** under `Domain::Value`.
All serialized v1 context envelopes begin with fixed, pinned marker bytes followed by a format version:

```text
Marker:  b"brix.context.envelope"  (ASCII, length-framed via write_bytes)
Version: 1                         (u64 via write_uint)
```

No wire format or decoding path is permitted to bypass the canonical encoder or reader (`CONTRIBUTING.md`, `DEPS.md`).

### 2.2 Preimage Field Order for `ContextEnvelopeV1`

The canonical preimage for a v1 context envelope is written in strictly pinned field order:
1. `write_bytes(CONTEXT_MARKER)`: `b"brix.context.envelope"`
2. `write_uint(CONTEXT_FORMAT_V1)`: `1`
3. `write_str(profile)`: e.g. `"brix.context.core@1"`
4. `write_uint(kind)`:
   - Ordinal `1`: **MaterializedCoreV1** (structured core context profile).
5. Canonical encoding of the variant payload:
   - `parent`: `Option<ContextId>` (`0 = None`, `1 = Some(32-byte digest)`)
   - `program_revision`: `Option<Digest>` (`0 = None`, `1 = Some(32-byte digest)`)
   - `world_revision`: `Option<Digest>` (`0 = None`, `1 = Some(32-byte digest)`)
   - `assumptions`: count `u64`, followed by each assumption `(name: String, payload: Vec<u8>)`
   - `regime_profile`: `String` (registered profile, e.g. `"brix.kernel.profile@1.2"`)
   - `execution_mode`: `u64` (`0 = Production`, `1 = Simulation`)
   - `step_budget`: `u64`
   - `depth_budget`: `u64`

#### Legacy Envelopes (Preserved Frozen ABI)
To preserve byte-for-byte identity correspondence with existing systems:
- **LegacyRoot**: Encoded verbatim via `write_tag("brix.ir.reflect.ScopeId.root")`.
- **LegacyExtend**: Encoded verbatim via `write_tag("brix.semantic.ContextId.extend") || parent.digest() || assumption`.

### 2.3 Identity Correspondence Invariant

For any materialized context payload $C$, its content-addressed `ContextId` is defined strictly by:
$$\text{ContextId}(C) = \text{ContextId::from\_canon}(\text{canon\_encode}(C))$$
where `ContextId::from_canon` computes `Digest::of(Domain::Value, payload)`.

#### Preservation of Existing Identities
1. **The Frozen Root Invariant:**
   The legacy root context digest is permanently pinned (ADR-0001 §5.1, ADR-0002 §6.1):
   ```text
   a7d1f9a56c727ac00ad5dd6dd97d4af1e943df9f605efcc265248c2c7b355c5c
   ```
   When encoded as the legacy root anchor (`write_tag("brix.ir.reflect.ScopeId.root")`), the digest
   MUST equal this frozen value byte-for-byte. The validator accepts this legacy canonical form
   as `LegacyRoot`.
2. **The Extension Invariant:**
   `ContextId::extend(parent, assumption)` hashes:
   ```text
   write_tag("brix.semantic.ContextId.extend") || parent.digest() || assumption
   ```
   This hash function is frozen ABI. A structured extension under `brix.context.core@1` produces its
   own deterministic `ContextId` via `context_id_of_payload_v1(&payload)` without altering legacy `extend` hashes.

---

## 3. Inventory of Context Dimensions

A complete evaluation context governs ten distinct operational and semantic dimensions:

| Dimension | Description | First Profile (`core@1`) | Omission Semantics | Representation |
|---|---|---|---|---|
| **1. Lexical / Proof Assumptions** | Hypotheses and typed variable assumptions $\Gamma$ | Structural payload only: named primitive canonical bytes; typed interpretation remains Specified | Empty: no local payloads; parent interpretation remains consumer-owned | Ordered sequence; duplicate local names forbidden; not checked as types/propositions |
| **2. Program Revision** | Identity of the program AST and static item declarations | Supported: optional `ProgramId` digest | Absent: program-independent assumption context | 32-byte canonical Digest reference |
| **3. World / History Revision** | Persistent world snapshot sequence, manifest, and trie root | Supported: optional `WorldRevisionId` digest | Absent: ephemeral / uncommitted computation | 32-byte canonical Digest reference |
| **4. Principal / View / Redaction** | Security principal, namespace visibility, redaction level | Not represented; Specified | Absent: no authorization assertion; consumer must supply its own policy | Future explicit binding; never inferred from naming |
| **5. Scenario / Execution Mode** | Production vs. Simulation / Counterfactual | Supported: `ExecutionMode` enum (`Production`, `Simulation`) | Core: required, no default; legacy root: Production; legacy extension: inherits validated parent mode | Enum ordinal |
| **6. Time Coordinates** | Valid / transaction time coordinates | Reserved: rejected in `core@1` | Absent: timeless / instantaneous logic | Integer timestamps (fixed epoch) |
| **7. Observation Profile** | Trace / audit capture and redaction policy | Not represented; Specified | Absent: no observation/redaction guarantee | Future explicit profile binding |
| **8. Kernel / Regime Profile** | Pinned calculus / typing regime specification | Supported: required profile string (e.g. `"brix.kernel.profile@1.2"`) | Required: omission is an error | Pinned canonical ASCII string |
| **9. Validity Envelope** | Temporal or revision range where context validity holds | Not represented; Specified | Absent: no validity-duration guarantee | Future range bounds |
| **10. Semantic Resource Limits** | Maximum evaluation steps, recursion depth, memory bounds | Supported: `ContextBudget` (steps, depth) | Required: explicit bounded values | Pair of `(u64, u64)` |

### Explicit Presence, Default, and Omission Rules
- **Required fields** (e.g. Regime Profile, Resource Limits) MUST be present in every `core@1` payload; missing values are fatal decode errors.
- **Optional fields** (e.g. Program Revision, World Revision) are explicitly encoded via `Option` (enum ordinals `0 = None`, `1 = Some(value)`).
- **Unsupported/Reserved fields** have no wire representation or dimensional
  flags in this slice. Extra bytes are rejected as `TrailingBytes`; unknown
  format/profile/kind has its corresponding error. No `UnsupportedDimension`
  error or generic dimension negotiation is implemented.
- Program/world digests bind bytes into context identity. The validator does
  not resolve those artifacts, validate their schemas, or check their authority.
  The consumer must perform those checks before relying on the references.

---

## 4. Representation and Bounded Verification Disciplines

### 4.1 Canonical Ordering and Deduplication
1. **Assumptions:**
   - Assumption bindings within `core@1` are stored as an ordered sequence of `(String, Vec<u8>)` (where `Vec<u8>` is length-framed canonical assumption data).
   - Duplicate variable names within a single scope level are strictly forbidden and rejected during decoding (`DuplicateAssumptionName`).
   - Binder shadowing (e.g. inner assumption overriding an outer assumption) is valid only when constructed via parent extension, never within the same local assumption block.
   - **Implemented structural limit:** each core assumption payload must be
     exactly one canonical length-framed byte string, unsigned integer, or
     signed integer, with complete consumption. The byte-string contents are
     opaque: arbitrary contents can pass. This is not a grammar for `Ty`, a
     proposition decoder, or an independent typing check. A profile-specific
     typed schema and checker are **Specified**, not supplied by this module.
   - Legacy extension assumptions preserve existing arbitrary byte payloads;
     the core primitive check must not change their frozen identity scheme.
2. **Digest Fields:**
   - All references to external artifacts (`ProgramId`, `WorldRevisionId`, parent `ContextId`) are fixed 32-byte digests. Decoders enforce exact length framing.

### 4.2 Structural Well-Formedness
A decoded envelope is structurally valid if and only if:
- All variable names are non-empty, valid ASCII identifiers (beginning with `a-z`, `A-Z`, or `_`, containing only alphanumeric, `_`, or `.`), and $\le$ `MAX_ASSUMPTION_NAME_BYTES` (64 bytes).
- The assumption count does not exceed `MAX_CONTEXT_ASSUMPTIONS` (256).
- The total payload size does not exceed `MAX_CONTEXT_BYTES` (64 KiB).
- Step and depth resource limits are non-zero and within permissible bounds (`1 <= step_budget <= 1_000_000_000`, `1 <= depth_budget <= 100_000`).
- No cycles exist in parent context chains.
- No trailing unconsumed bytes exist after decoding.

The registered regime names are a local allowlist of metadata identifiers, not
evidence that the kernel supports the payload's assumptions. Consumers must
compare their independently expected regime/profile and verify its semantics.
Do not use the fixture profile `brix.test@1` as production authority.

---

## 5. Operations and Confinement Laws

Except for the simulation ancestry checks and explicit `require_mode` gate,
these are **Specified** consumer obligations. This module does not implement
publication, assumption substitution/discharge, authorization restriction or
transport. Distinct digests alone do not demonstrate non-leakage.

### 5.1 Context Extension
Extending context $C$ with assumption $A$ yields $C' = C \rhd A$:
1. $C'$ records $\text{parent} = \text{ContextId}(C)$.
2. The assumption $A$ is appended to $C'$'s assumption set.
3. The identity $\text{ContextId}(C')$ satisfies $\text{ContextId}(C') \neq \text{ContextId}(C)$.
4. **Non-Leakage / Confinement:** Judgements derived under $C'$ CANNOT be published under $C$. Weakening must not permit local assumptions of $C'$ to escape into the parent.

### 5.2 Simulation vs. Production Isolation
A context marked with `ExecutionMode::Simulation` CANNOT satisfy a consumer requiring `ExecutionMode::Production`.
Evidence derived under a simulation context cannot be transported into production without an explicit,
validated translation bridge.

### 5.3 Authority Non-Escalation
Restricting a context (e.g. projecting to a smaller assumption set or a restricted view) MUST NOT
widen authority or erase validity constraints.

---

## 6. Resource Limits and Failure Classification

### 6.1 Decode Budgets
Decoders must strictly enforce limits before unbounded allocation:

| Limit | Maximum Value | Enforcement Point |
|---|---|---|
| `MAX_CONTEXT_BYTES` | 65,536 bytes (64 KiB) | Checked on input byte slice |
| `MAX_CONTEXT_ASSUMPTIONS` | 256 assumptions | Checked during core assumption decoding |
| `MAX_ASSUMPTION_NAME_BYTES` | 64 bytes | Checked during core identifier decoding |
| `MAX_ASSUMPTION_VALUE_BYTES` | 4,096 bytes | Checked during core assumption payload decoding |
| `MAX_CONTEXT_DEPTH` | 128 levels | Checked during recursive parent validation |

Caller budgets may tighten these hard maxima, not raise them. Parent depth is
the number of parent edges (a parent-free envelope has depth zero); a zero-depth
budget refuses a child before consulting the resolver. The parent resolver
supplies already structurally validated handles. These limits bound this
module's decoding/traversal, not arbitrary resolver I/O or cumulative semantic
evaluation. A consumer must budget those separately.
Legacy extension payloads remain opaque and are bounded by total envelope bytes;
they do not have core named-assumption count/value semantics.

### 6.2 Fail-Closed Error Taxonomy
Every validation failure maps to a typed, exhaustive `ContextValidationError`:
- `MalformedMarker`: Preimage marker does not match `b"brix.context.envelope"`.
- `UnknownVersion(u64)`: Envelope version is unsupported.
- `UnknownKind(u64)`: Payload kind ordinal is not recognized.
- `UnsupportedProfile(String)`: Context profile is not implemented.
- `IdentityMismatch { claimed: ContextId, expected: ContextId }`: Payload digest does not match expected identity.
- `Truncated`: Premature EOF during canonical reading.
- `TrailingBytes`: Input contains unconsumed data after envelope.
- `NonMinimalInt`: Canonical integer uses non-minimal encoding.
- `BadLength`: Length prefix exceeds buffer or is malformed.
- `DuplicateAssumption(String)`: Repeated variable name in assumptions.
- `AssumptionNameTooLong`: Name exceeds 64 bytes.
- `AssumptionValueTooLong`: Assumption payload exceeds 4,096 bytes.
- `ExcessiveAssumptions`: Assumption count exceeds 256.
- `DepthLimitExceeded`: Parent hierarchy exceeds max depth.
- `CycleInParentHierarchy(ContextId)`: Cyclic dependency detected in parent references.
- `InvalidAssumptionName`: Variable name is empty or contains illegal characters.
- `InvalidAssumptionPayload`: Assumption payload was empty, malformed canonical data, or left unconsumed trailing bytes.
- `InvalidBudget`: Step or depth budget is zero or exceeds arithmetic overflow limit.
- `EmptyRegimeProfile`: Regime profile string was empty.
- `MalformedRegimeProfile(String)`: Regime profile string contained control characters, NUL bytes, or non-ASCII characters.
- `UnsupportedRegimeProfile(String)`: Regime profile is not recognized or in the registered allowlist (`SUPPORTED_REGIME_PROFILES`).
- `RegimeProfileTooLong`: Regime profile string exceeded 128 bytes.
- `UnknownExecutionMode(u64)`: Execution mode ordinal is not recognized.
- `ModeMismatch { claimed: ExecutionMode, expected: ExecutionMode }`: Simulation/Production boundary violation (checked at every parent-child edge).
- `UnresolvedParent(ContextId)`: Required parent context is unavailable.

**Epistemic Separation:** None of these failures is a logical refutation (`Outcome::Refuted`). Context validation failure means context contents cannot be established, resulting in typed rejection or epistemic `Outcome::Unknown`.

---

## 7. Interface Agreement with Issue #52 (Resolvers)

Resolvers (such as Issue #52's candidate outcome resolver) require context validation across deliberation boundaries.
The following is the V1 structural interface. Production materialized-context
admission is the V2 `soc_core::contextual` seam in §10; #52 can consume that
seam without defining a parallel encoding:

1. **Two-Tier Validation Seam with Sealed Types:**
   - #52 continues to use `ContextId` as its opaque identity in candidate and goal structures.
   - When authenticating a single context envelope without walking parents, callers invoke `validate_context_v1`:
     ```rust
     fn validate_context_v1(
         bytes: &[u8],
         expected: &ContextId,
         budget: &ContextBudget,
     ) -> Result<AuthenticatedContextEnvelopeV1, ContextValidationError>;
     ```
     `AuthenticatedContextEnvelopeV1` authenticates cryptographic binding and local structure, but does NOT assert execution mode or guarantee parent soundness.
   - When full ancestry validation and execution mode guarantees are required (e.g. evaluating production goals), callers invoke `validate_context_with_parent_resolver`:
     ```rust
     fn validate_context_with_parent_resolver<F>(
         bytes: &[u8],
         expected: &ContextId,
         budget: &ContextBudget,
         parent_resolver: F,
     ) -> Result<ValidatedContextV1, ContextValidationError>
     where
         F: Fn(&ContextId) -> Option<ValidatedContextV1>;
     ```
     `ValidatedContextV1` guarantees parent identity binding, absence of cycles, bounded depth, and edge-by-edge simulation isolation.
     It does not check typed assumptions or non-parent referenced artifacts.
     Its private fields and lack of public unchecked constructors are part of
     this boundary; adversarial fabricated handles belong only in crate-local
     test code, never a production `for_test` API.
2. **No Parallel Encodings:**
   - #52 SHALL NOT invent private context serialization formats or parse unvalidated assumption strings.
3. **Authentication Boundary:**
   - The consumer (#52) MUST supply the `expected: ContextId` from an independently verified origin (e.g. goal specification, prior judgement), NEVER from the untrusted context envelope itself.
4. **Denial Behavior:**
   - If context validation returns an error, resolution fails closed with `Outcome::Unknown` or an explicit resolution error; it NEVER proceeds with unvalidated context assumptions.

## 8. V1 evidence and migration boundary

The bounded slice has these existing pins in
`crates/brix-semantic/tests/context_contract.rs` (unit-only fabricated ancestry
cases live inside `src/context_contract.rs`):

| Property | Evidence / status |
|---|---|
| Frozen legacy root and extension IDs | Pinned: `test_legacy_root_compatibility_and_frozen_digest`, `test_legacy_extend_compatibility` |
| Core wire field order and independent primitive reconstruction | Pinned: `test_core_context_independent_primitive_reproduction`, `test_specification_kind_discriminant_accepted` |
| Identity mismatch, malformed/oversized/trailing input | Pinned: corresponding hostile-input and identity tests |
| Requested parent identity and legacy mode inheritance | Pinned: `test_wrong_parent_handle_is_rejected`, `test_legacy_extension_of_simulation_inherits_simulation_mode` |
| Separate envelope and ancestry result types | Implemented; root/parent resolver entry points return the stronger structural handle |
| No public unchecked validated-handle construction | Pinned: `ValidatedContextV1` compile-fail doctests for private fields and the removed `for_test` constructor |
| Ancestry cycles and every-edge mode checking | Pinned: crate-local fabricated-ancestry unit tests; not publicly constructible handles |
| Depth checked before legacy parent lookup; exact core/legacy/mixed boundary | Pinned: `legacy_extension_checks_zero_depth_before_resolving_parent`, `exact_depth_boundary_passes_for_core_legacy_and_mixed_chains` |
| Caller budgets cannot raise hard decoder/ancestry maxima | Pinned: `loose_budgets_cannot_exceed_hard_caps` |
| Fixture consumer rejects identity/parent/profile/mode failures before consumption | Pinned: `fixture_consumer_requires_identity_parent_profile_and_mode`; not production #52 wiring |
| Typed assumptions, binder semantics and discharge | Specified: primitive framing and distinct IDs do not implement these |
| Program/world artifact availability and semantic compatibility | Specified: only digest fields are bound today |
| Principal confinement, redaction, observation profile and validity range | Specified: absent from the first wire profile |
| Production resolver/kernel/typing/evidence integration | Specified: existing consumers retain their previous identity-only checks |

The following V1-only gaps motivated V2 (§9). Legacy entry points preserve
their old contracts; materialized consumers use the new APIs in §10. Migration
responsibilities:

1. #52: replace the fixture validator with a concrete adapter, independently
   supply expected identity/profile/mode, and prevent resolution after failure.
   A fixture adapter test is not a production integration.
2. #53/kernel: define the typed-assumption interpretation and accepted profile
   before using materialized contents; preserve existing context binding and
   frozen certificates. Do not add a kernel-to-resolver dependency.
3. Finite/world evidence: bind independent host expectations and referenced
   artifacts; validate authority separately from structural context success.
4. Alpha.9 scoped requests: explicitly extend the supported profile for host
   scope and evidence. Unsupported dimensions cannot be hidden in opaque
   assumptions and described as validated authorization.

Each migration requires positive/negative tests through its real consumer.
General weakening, restriction, substitution, discharge, historical correction
and cross-mode transport need accepted semantics and their own enforcement pins.
Until then, unsupported transformations must be refused by consuming APIs.


## 9. V2 canonical model and structural rules

Implementation: `crates/brix-semantic/src/context_model.rs`. This is an additive
format, not a new spelling for V1. The preimage uses `brix-canon`, marker
`brix.semantic.context-model`, uint version `2`, and this exact order:

1. Parent `Option<ContextId>`.
2. Program and world references, each `Option<(kind: ident, digest: bytes32)>`.
3. Namespace, tenant, principal, capability-view identifiers.
4. Scenario ordinal (`0` production, `1` simulation), optional branch digest.
5. Valid-time start/end uints; optional transaction, correction, simulation times.
6. Observation, redaction, kernel, regime, axiom-profile identifiers.
7. Validity start/end, step and depth budget uints.
8. Assumption count, followed by strictly name-sorted `(name: ident, schema: ident,
   canonical_value: framed bytes)` entries.

Options use uint `0`/`1`; a present digest is length-framed 32 bytes. All required
identifiers are explicit, nonempty bounded ASCII; there are no ambient defaults.
Absent program/world means no artifact binding, not permission to use arbitrary
state. Absent optional time means that coordinate is not asserted. Required
profiles must name the consumer's supported interpretation; `none` explicitly
means no additional axioms. An application can choose an explicit unscoped
namespace/view identifier only under its own host policy. Naming a principal
or capability never authenticates it.

Production requires no branch; simulation requires a branch identity. Intervals
must be ordered; present time coordinates must be within the declared validity
interval. Time values are nonnegative profile-owned coordinates; this schema
adds no implicit wall clock, unit conversion or expiry oracle. The host binds
expected time/profile values for each use. Different snapshots, corrected times,
profiles, limits or authority views produce different context identities.

Limits: 64 KiB cumulative context-chain bytes, 256 cumulative assumption entries,
128 parent edges, 256-byte identifiers, 4 KiB per assumption, at most 16 artifact
references and 64 KiB cumulative artifact preimages. Host budgets may tighten
these limits. Schema validators consume a shared byte-work budget. Resolver
implementations must bound their own I/O/allocation before returning bytes;
returned bytes are checked before decoding, hashing or subsequent acceptance.
No type callback or source resolver is loaded from untrusted input: these are
explicit embedding trust boundaries.

The model reconstructs every requested parent ID from its bytes. Parent traversal
is iterative and bounded. All non-assumption dimensions must match across a
parent edge, including authority, scenario, world, times, profiles and budgets.
V2 parent references resolve V2 envelopes only; legacy V1 roots are not silently
converted into typed V2 parents. Changing non-assumption dimensions requires a
new separately admitted root; it is not
logical weakening. References are resolved from the host store and checked
against `Digest::of(Domain::Value, canonical_preimage)`. Missing or wrong bytes
fail before a checked handle exists. Artifact kind/ownership/semantic policy
belongs to the owning resolver and consumer; digest matching alone is not
program verification or authorization.

Typed assumptions are schema-indexed values. V2 never accepts a payload as a
type/proposition just because it is a canonical integer or byte string. The
owning validator must decode and check its supported schema. Private
`CheckedContextV2` fields prevent raw construction; consumer adapters also
validate their exact supported regime/profile and decode their owned payloads.
A host providing a custom validator is explicitly inside the trust boundary.

### Operations

| Operation | Implemented rule / explicit prohibition |
|---|---|
| Construction and comparison | Bounded canonical construction, exact ID equality; different IDs are not inferred equivalent |
| Typed extension | A checked child contains a new local binding and references the checked parent; same-name local binding shadows inherited binding |
| Rebinding / substitution of a context binding | `rebind_typed` creates and validates a new child; replacing a used binding invalidates transport of the old claim |
| Exchange / contraction | Local bindings are canonical name-sorted; duplicate local names are rejected, not silently merged |
| Local restriction | `restrict` removes selected local bindings; removing inherited bindings this way is refused |
| Effective projection | `project_assumptions` materializes selected effective bindings in a separately checked context; receipt retains the full source evaluation-context ID |
| Binder discharge | Owning typing/kernel lambda rules discharge binders; deleting an assumption does not discharge a proof obligation |
| Logical transport | Source and target are checked; every declared used binding must have identical name/schema/value and every non-assumption dimension must be equal |
| Authority / privacy / world / mode / time / profile / resource transport | Explicitly prohibited across differing dimensions; no implicit widening, correction, view lifting or simulation-to-production bridge |
| General term substitution or equivalence | Only existing owning calculus rules; no new arbitrary cross-context equivalence engine |

`ContextTransportReceiptV2` records source ID, destination ID and used-assumption
set digest, with its own canonical identity. It records checked correspondence,
not a theorem. The caller's used-name set is a dependency claim; a proof or type
consumer **must recheck the term in the destination**. No receipt API upgrades
an outcome, changes a certificate's context, or discards historical identity.
Projection is sufficient only for the supplied dependency set, not a claim of
globally minimal context.

## 10. Production consumers

### Native typing (#53)

`soc_regimes::context_validation` implements schema `brix.type.ty@1` by decoding
the existing canonical `Ty` encoding. It checks closed scalar/function/product/
record/sum/recursive types, scoped, constructor-guarded recursion variables, duplicate names,
canonical field order, full consumption and hard byte/node/depth/member limits.
Inference variables and declaration parameters are rejected. Accepted constants
match the native vocabulary, including `Complex`.

The supported consumer is `regime_profile = brix.type@1`,
`kernel_profile = brix.kernel.profile@1.2`, `axiom_profile = none`.
`ty_context_from_model_v2`, `infer_tree_in_context_v2`, and
`audited_type_check_tree_in_context_v2` perform real inference/audit with the exact
materialized context ID and nearest-scope bindings. The type adapter's step/depth
profile counts input expression nodes/nesting during admission; it does not
claim to meter all unification operations or wall time. Existing inference and
canonical type identities remain unchanged. A typed judgement remains scoped;
it does not prove an evaluated value.

### Kernel (#56)

`brix_kernel::acceptance_in_context` admits `brix.kernel.prop@1` canonical
propositions under the existing kernel profile, `brix.proof@1` regime and `none`
axiom profile. It validates the host's expected identity/dimensions/mode,
references and ancestor assumptions, then supplies checked hypotheses to the
existing proof checker. Kernel checking uses the smaller host/context budgets.
Malformed context is a distinct `ContextAcceptanceError`, not logical rejection.
Term-context mismatch remains a distinct verdict.

Accepted contextual proofs use verifier `brix.kernel.context@1` and a new
`brix.kernel.context-certificate` preimage (version 1, kernel profile, context ID,
canonical proposition, canonical explicit term). That certificate identity is
computed only after bounded acceptance. Legacy `acceptance` and certificate-v1
vectors are preserved. Contextual certificates must be replayed through the
contextual entry point; they cannot be presented as legacy verifier results.
Rebinding a term to a projected context requires checking it again: an external
`Hyp` fails when its assumption is removed, while a closed lambda identity can
still be proved there.

### SOC discovery (#52) and saturation/observation (#61)

`soc_core::contextual::ResolutionContextV2::validate` provides real admission
before provider enumeration. Candidates retain the exact context. Discovery
requires a `soc.exec@1` world artifact binding portable world and policy digests
and history (`brix.soc.context-execution@1`); changing any component refuses the
request before provider invocation. This is admission, not a completeness proof
or a second resolver. #52's higher-level outcomes can consume this seam without
copying the context codec.

`check_quiescence_in_context` and `check_divergence_in_context` bind the context,
`soc.presentation@1` program reference, and exact observation-profile digest to
the existing presentation before independent replay. The initial saturation
consumer uses `brix.soc.saturation@1`, kernel/axiom profiles `none`, no external
logical assumptions and no
separate world reference (the presentation/certificate bind the world). Existing
certificate semantics remain `Derived`/`Unknown`, never theorem authority. The
context step limit caps replayed prefix length before replay; provider execution
remains the existing total-provider trust boundary.

### CLI inspection and admission

```
brix context context.canon --json
brix context context.canon --expect <independent-context-id> \
  --store ./artifacts --profile typing --json
```

Without `--expect`, output explicitly says `decoded-only`. With an independent
pin, the command validates supported typing/kernel assumption schemas, mode,
parents and artifact preimages before saying `validated`; output always says
`authority: none`. `--simulation` explicitly selects simulation admission.
Parents resolve as `<digest>.context` and artifact preimages as
`<digest>.artifact`; untrusted kinds never select filesystem paths. Reads are
bounded and create no world state. Rejections include the expected context and
available observed provenance, not `Refuted`.

The inspection shows program/world references, namespace/tenant, principal/view,
scenario/branch, times, validity, observation/redaction, profiles and limits.
Existing finite `audit`/`why`/`verify` commands retain their program-bound context
identities and legacy evidence semantics; use the explicit context inspector
for V2 material. Per-request host contexts across those finite execution APIs
and Python remain alpha.9 A9-4 integration, not an implicit change to alpha.8.
There is no new general-purpose `prove` surface or evidence authority shortcut.

## 11. Conformance and compatibility

- `tests/context_model.rs`: V2 canonical round-trip and independent pinned vector;
  expected binding/ID failures; missing/forged references; cumulative budgets;
  ancestry identity/mode checks; shadowing; restriction/projection; all nonlogical
  dimension transport refusals; corrected-used-assumption refusal; historical
  source immutability. Private checked handles and receipts have compile-fail pins.
- `soc-regimes/tests/context_validation.rs`: real inference and audit in parent/
  child scopes, exact judgement context, malformed/open types and unsupported
  profiles, decoder limits, expression admission budgets, lambda discharge and
  projected-context rechecking.
- `brix-kernel/tests/context_validation.rs`: actual hypotheses, missing references,
  schema/profile/mode mismatch, scoped object binders, private external Gamma,
  context budget enforcement, lazy certificate construction, projection recheck
  and independent contextual-versus-legacy certificate identity.
- `soc-core/tests/contextual.rs`: actual quiescence replay under exact context/
  observation/presentation bindings; discovery refuses changed world/policy/history
  before calling a provider.
- `brix-cli/tests/context_model.rs`: separate-process decoding versus admission,
  independent expected pin, profile/mode rejection, artifact availability/tamper,
  dimensions in diagnostics and read-only envelope behavior.

Existing legacy root/extension and kernel/saturation certificate vectors are
unchanged. The first V1 envelope remains structural and opt-in. V2 is a new
identity domain/format; old context digests are never relabelled. The initial
supported consumers deliberately refuse unsupported profiles and transports.
Broader schema ownership, principal authentication and context admission at a
remote host remain explicit embedding obligations, not inferred from names.

## 12. Verification ledger

Combined-worktree checks on 2026-10-31:

- Workspace clippy with `--all-targets -- -D warnings`: passed.
- `cargo test --workspace --no-run --offline`: all workspace test targets built.
- Canonical Python cross-check: all 69 frozen canon/1 vectors reproduced.
- TCB dependency inventory/policy: passed; no new dependency edges.
- Formatting and diff whitespace: checked after final edits.
- Combined V2 model and context integration tests passed, including the
  independent encoding vector, inherited-binding transport, changed-binding
  rejection, and arithmetic bridge closure under the materialized context.
- `python3 scripts/test_workspace.py`: **2,157 tests passed**, 28 existing ignored
  tests skipped; all workspace doctests passed. Total time: 158.88 seconds.
  The earlier macOS `_dyld_start` launch stall and relocated CLI binary path
  issue are resolved.

This validates the combined context foundation against the workspace's standard
test gate. Ignored scale/release workloads have not been release-qualified here.
The finite execution/Python integration described in §10 remains alpha.9 A9-4
work; the shared foundation alone does not close every integration item in #59.

## 13. Combined #52 / #53 / #59 foundation

The resolution adapter uses `AdmittedContextValidator` to compare request
identities against a V2 context already admitted by the common validator.
`ResolutionReport` retains the exact query and operational outcome; it does not
publish an epistemic grade. A caller-authored finite domain and a v1 saturation
quiescence certificate cannot establish resolution-goal completeness and are
refused as unsupported evidence.

Native arithmetic can close its two primitive occurrences while retaining the
materialized typing context. `arithmetic_bridge_closure_retains_the_materialized_context`
checks real inference, elaboration, occurrence closure and wrong-context replay
rejection together. Literal and structural split antecedents remain in the
accepted conditional proposition.

The kernel context decoder admits the appended structural identity object
ordinal under its existing byte/depth limits. The
`identity_propositions_roundtrip_through_materialized_contexts` test checks
this path with independent kernel acceptance. Existing encodings and identity
vectors are preserved.
