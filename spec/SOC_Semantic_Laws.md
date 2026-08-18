# SOC semantic laws and conformance map

Status: **Normative, profile 1 (2026-08-01)**. Governed by
[ADR-0002](./adr/ADR-0002_SOC_Constitution.md). The machine-checked companion
is [`conformance/soc-semantic-laws.json`](./conformance/soc-semantic-laws.json).

This document is the common law registry for #52, #53, #56, #59, #61, and the
trusted-boundary audit in #63. Those designs may refine a law for a versioned
profile; they must cite its law ID and may not redefine the shared rule.

The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative. A law
is not satisfied merely because a test returned no counterexample.

## Reading the status column

- **Enforced** means an executable gate covers the explicitly bounded current
  profile. It is not a theorem about arbitrary future regimes or programs.
- **Partial** means a real validator or conformance gate exists, but a named
  authority, durability, context, or coverage obligation remains open.
- **Open** means the rule is normative but its defining validator or semantic
  interface has not landed. Existing tests may only protect prerequisites.

`Unknown`, unsupported, resource exhaustion, or absence of a counterexample
never counts as satisfaction. `Refuted` is reserved for a kernel-certified
negative proposition; a rejected proof candidate is not a refutation.

## Quantification and context

Every law quantifies over explicit semantic artifacts in one exact
`ContextId`. Today that ID has a frozen root and deterministic extension, but
the versioned context contents are still open under #59. Until #59 lands, no
law may silently assume that two equal digests carry compatible revisions,
principals, branches, observation profiles, kernel profiles, or resource
limits. A gate that exercises only `ContextId::root()` proves only that root
fixture.

## Registry

| ID | Law | Current status | Owning authority or validator | Open obligation |
| --- | --- | --- | --- | --- |
| SOC-LAW-01 | Canonical identity | Partial | `brix-canon` plus semantic canonical implementations | #56 |
| SOC-LAW-02 | Realization compositionality | Partial | `brix-kernel` for proof claims; regime owners for primitive relations; the typing regime's obligations are specified in [`Type_Realization_Contract.md`](./Type_Realization_Contract.md) | #53, #178 |
| SOC-LAW-03 | Audit honesty | Enforced | `soc-core::audit` in the current journal profile | #228 for publication hardening |
| SOC-LAW-04 | Epistemic non-escalation | Enforced | outcome lattice, audit, elaboration, and result-grade cap | — |
| SOC-LAW-05 | Authority non-escalation | Enforced | the two kernels and named external drivers | — |
| SOC-LAW-06 | Context confinement | Open | future context validator and transport checker | #59 |
| SOC-LAW-07 | Governance monotonicity | Enforced | `Adm` conformance over witness+successor candidates | #52, ADR-0028 |
| SOC-LAW-08 | Incremental agreement and cost honesty | Enforced | reference oracle, incremental engine, deterministic cost gate | #52, #178 for future regimes |
| SOC-LAW-09 | Correction and retraction non-erasure | Partial | evidence durability taxonomy and future invalidation engine | #59, #178 |
| SOC-LAW-10 | Observable-behavior fidelity | Partial | soc-core saturation, certificate, and bisimulation checkers | #59, #178 |
| SOC-LAW-11 | Translation fidelity | Partial | each frontend/adapter plus its reference relation | #52, #53, #178 |
| SOC-LAW-12 | Verifier closure | Partial | `brix-kernel` and canonical artifact validator | #56 |

## SOC-LAW-01 — Canonical identity

**Domain and context.** Versioned canonical values and every durable or
revision-scoped semantic identity derived from them, in an explicitly named
encoding version and domain separator. Context-sensitive artifacts include
their exact `ContextId`.

**Rule.** Equal semantic values MUST produce byte-identical canonical bytes
and identity digests. A durable identity MUST NOT depend on `Debug`/`Display`,
map iteration order, process state, host paths, locale, wall clock, or an
unversioned serializer. Decoding and validation MUST fail closed.

**Authority and evidence.** `brix-canon` owns primitive encoding and digest
domains; each `brix-semantic` artifact owns its field order and enum ordinals.
Frozen vectors and an independent reproduction are executable evidence.

**Failure form.** A vector mismatch, unknown version/tag, malformed bytes, or
noncanonical representation is a compatibility/validation failure, never an
alternate identity. Native proof-certificate identity is the pinned canonical
v1 envelope (ADR-0013), frozen by `vectors/kernel_certificate_v1.json` and
reproduced independently; #56 keeps this law Partial for the remaining durable
artifact encodings.

**Evolution.** Existing tags, field orders, domain separators, and ordinals are
append-only ABI. An intentional incompatible encoding requires a new profile or
version, never silent reinterpretation.

## SOC-LAW-02 — Realization compositionality

**Domain and context.** Configurations `x,y,z`, witnesses `f:x→y` and `g:y→z`,
their primitive relations, and explicit proof terms in one context.

**Rule.** Realization is normal lax: whenever `(x,y)∈ρ_f` and `(y,z)∈ρ_g`,
then `(x,z)∈ρ_(g∘f)`. Tensor composition obeys the corresponding componentwise
direction. For the logged tight subcategory generated by `𝒢`, an audited
decomposition additionally claims exact relational composition.

**Authority and evidence.** The kernel checks explicit composition/tensor proof
terms. Regime or generator owners remain responsible for the primitive leaf
relations; a kernel proof of composition conditional on leaf constants does
not discharge those leaves. `honest_result_outcome` caps the final typing grade
at `Audited` until every primitive leaf used by that result is registered as
tight.

**Failure form.** Endpoint, middle-object, witness, or tensor-shape mismatch is
rejected with no theorem. Missing primitive discharge remains `Audited` or
`Unknown`; it MUST NOT be presented as `Proven`.

**Evolution.** New composition rules require an explicit kernel profile and
adversarial vectors. New generators require a named semantics and discharge
story under #53/#178.

**Typing-regime contract.** [`Type_Realization_Contract.md`](./Type_Realization_Contract.md)
specifies that discharge story for the typing regime — every primitive
generator's exact source/target relation, derivation well-formedness, the
negative-outcome taxonomy, and what a discharge artifact must contain (§9).
Each clause carries its evidence status, so the document states what is pinned
by a test, what is only partly pinned, and what is specified with no
implementation. It **defines** these obligations; it does not discharge them,
and this law stays `Partial` until the obligations it names are met.

## SOC-LAW-03 — Audit honesty

**Domain and context.** A journaled `CommittedStep`, its recorded
`Decomposition`, generator registry `𝒢`, replay semantics, and exact context.

**Rule.** `Audited` MUST be a new judgement produced only after the recorded
chain, endpoints, registry membership, primitive relations, and log-derived
`Derived` judgement all replay exactly. The committed hot path may record a
decomposition; it MUST NOT assert replay verification.

**Authority and evidence.** `soc-core::audit::{audit_step,audit_journal}` is the
current reference checker. A successful audit emits a replay-verified
decomposition and a premise dependency to the unchanged `Derived` judgement.

**Failure form.** Corrupt endpoints, unknown generators, failed primitive
relations, non-recorded input, or log mismatch produce `AuditResult::Unknown`.
No partial replay is a pass. The enforcement status is bounded to this audit
entry point; public publication hardening remains #228.

**Evolution.** Alternative audit implementations must reproduce the same
canonical result and failure partition before receiving authority.

## SOC-LAW-04 — Epistemic non-escalation

**Domain and context.** Candidates, measurements, estimates, derivations,
audits, proof terms, evidence, and final judgements in one context.

**Rule.** No search result, estimate, candidate, committed derivation, or audit
may silently become a theorem. The only strengthening path is evidence-bearing
and creates new judgements: `Derived → Audited → Proven` (or a separately
certified `Refuted`). `Unknown` is bottom and cannot be laundered into truth or
falsity.

**Authority and evidence.** `Outcome` fixes the lattice and authority table;
audit and elaboration build explicit dependency edges; type realization caps a
conditional kernel composition proof by primitive-leaf discharge.
ADR-0011's `match … proving exhaustive` path likewise keeps the kernel-Proven
coverage proposition separate from the match expression's independently graded
typing result.

**Failure form.** Unsupported syntax, failed inference, failed replay, rejected
proof terms, and resource exhaustion retain their distinct failure/verdict
forms. An unsupported coverage-certificate shape is `CoverageOutcome::Unknown`,
not a structural pass mislabeled `Proven`. In particular, exhaustion maps to
`Unknown`, never `Refuted`.

**Evolution.** A new outcome or strengthening edge is an append-only semantic
ABI change and needs an authority row, canonical ordinal, validator, and
adversarial tests. Direct public judgement construction remains a known gap in
#228 and the realization routes recorded by #63/#178.

## SOC-LAW-05 — Authority non-escalation

**Domain and context.** Every code path capable of publishing an `Outcome`,
evidence identity, verifier identity, or elaboration-boundary dependency.

**Rule.** Inference and search may construct candidates but MUST NOT mint
settlement, audit, proof, refutation, or external-measurement authority. The
settlement kernel alone publishes committed `Derived`; the audit checker alone
publishes replay-backed `Audited`; the proof kernel alone accepts proof terms;
named external drivers own their certified envelopes.

**Authority and evidence.** The #63 TCB manifests and dependency gate fix the
current architectural boundary. Kernel production dependencies are mechanically
restricted.

**Failure form.** An unrecognized publisher, mismatched outcome/evidence pair,
or missing provenance is invalid authority and must fail closed without a
judgement. Today `Judgement::new` and the elaboration source contract do not
enforce this completely, so #228 keeps the law Partial.

**Evolution.** Adding a publisher requires an explicit authority-table change,
TCB review, dependency-closure review, and negative construction tests.

## SOC-LAW-06 — Context confinement

**Domain and context.** Logical/type assumptions and the future explicit
dimensions for revision, world/history, principal/capability view, branch,
time, observation/redaction, model/kernel/regime profile, and semantic resource
limits.

**Rule.** A judgement or certificate MUST NOT escape, erase, widen, or silently
default a context dimension. Cross-context reuse requires validated,
evidence-bearing transport with dimension-specific structural rules.

**Authority and evidence.** The current substrate proves only stable identity,
deterministic extension, and kernel digest mismatch rejection. #59 owns the
versioned contents, decoding, projection, transport, and confinement validator.

**Failure form.** Malformed, unavailable, incompatible, or unauthorized context
is a provenance-bearing context error or `Unknown`, not logical rejection and
not theorem satisfaction.

**Evolution.** `ContextId::root()` is frozen. New dimensions require canonical
omission/default rules and an explicit statement of weakening, exchange,
contraction, strengthening, transport, or prohibition.

## SOC-LAW-07 — Governance monotonicity

**Domain and context.** A fixed witness-provider presentation and execution configuration `e`, and
two admissibility predicates where `Adm_tight(c) ⇒ Adm_loose(c)`.

**Rule.** Tightening governance MUST shrink candidates and successors
pointwise: `cand_tight(e) ⊆ cand_loose(e)` and
`Succ_tight(e) ⊆ Succ_loose(e)`. Composition of tightenings remains a
tightening. Governance may remove authority to act; it cannot manufacture a
candidate, successor, or stronger epistemic outcome.

**Authority and evidence.** `soc-core`'s retained naive oracle is the reference
for the current `WitnessProvider`/`Adm` interface. The executable fixture checks every
reachable configuration and composed allowlists.

**Failure form.** A candidate or successor present only under the tighter
predicate is a minimal counterexample containing `e`, both policies, and the
offending canonical candidate. Exhausted exploration is `Unknown`, not success.

**Evolution.** New governance combinators must prove or conformance-test the
implication relation they claim; #52 owns the general resolution contract.

## SOC-LAW-08 — Incremental agreement and cost honesty

**Domain and context.** A fixed provider/admissibility presentation, world-delta stream,
reference candidate view, incremental materialized view, and deterministic
work-unit model.

**Rule.** After every delta, incremental and naive candidate views MUST be
byte/iteration-order identical. Per committed incremental step MUST scale with
`|Δ| × fanout`, not inert world size. Unknown cost MUST be represented as
unknown, never zero or omitted.

**Authority and evidence.** The naive recomputation is the semantic reference;
`IncrementalEngine` is the fast path. Differential fixtures cover ordinary and
tightened governance. The armed O(Δ) gate doubles inert configurations and
requires flat incremental work while proving the naive control grows.

**Failure form.** The first differing candidate/delta index is the semantic
counterexample. A cost regression reports both deterministic work counts and
fixture sizes. Wall-clock timing alone is not evidence.

**Evolution.** Every new incremental regime needs a shared-semantics naive
projection and differential stream. Alternative cost models must be versioned
and deterministic.

## SOC-LAW-09 — Correction and retraction non-erasure

**Domain and context.** Judgements, evidence durability, dependency edges,
revision/history identities, corrections, and retractions.

**Rule.** Retracting revision-scoped support invalidates dependent current
conclusions without deleting or rewriting durable theorem history. A corrected
claim is a new contextual judgement; prior evidence remains attributable to
its original context and revision.

**Authority and evidence.** `Evidence::durability` currently distinguishes
kernel evidence from revision-scoped replay/measurement evidence, and audit
upgrades create a distinct judgement plus dependency edge. A complete
invalidation traversal and historical context model have not landed.

**Failure form.** Missing support yields invalidated/unavailable or `Unknown`
with provenance, not a mutated old judgement, silent current theorem, or
`Refuted` claim.

**Evolution.** #59 owns correction/revision context; #178 owns the remaining
runtime integration. Any garbage collection must preserve canonical historical
identity and auditability.

## SOC-LAW-10 — Observable-behavior fidelity

**Domain and context.** Implementations of one declared observation profile,
including their administrative (`τ`) and visible realizing steps, exact
context/revision, and resource limits.

**Rule.** An implementation change MUST preserve divergence-sensitive
saturated behavior at the declared observation boundary. Finite `τ*;o` may be
hidden to `o`; infinite or exhausted administrative work MUST NOT be called
quiescent. Directional refinement may replace symmetric equivalence only when
the contract states that direction.

**Authority and evidence.** `soc-core`'s saturation, certificate, and
bisimulation checkers (ADR-0014). Canonical `τ`/realizing labels are a declared
profile projection over fully committed steps; quiescence and divergence are
versioned certificates with frozen vectors, fail-closed readers, and semantic
checkers that **re-derive** each claim rather than trusting it; weak
bisimulation and directional refinement are a lockstep walk whose counterexample
is minimal by construction.

**Failure form.** The unique shortest unmatched visible trace is the
counterexample. Certified divergence, unsupported analysis, and budget
exhaustion are all `Unknown` — certified divergence being a positive fact about
the system, exhaustion merely a fact about the analysis — and none of them is a
quiescence certificate. Only a replayable, independently re-derived certificate
establishes quiescence.

**Why Partial, not Enforced.** Four independent reasons (ADR-0014 §10), each
sufficient. The law's own domain is *"one declared observation profile"*, and
**#59** owns whether a profile identity is a valid context dimension — #61 ships
*a* boundary (`generator-partition@1`), not *the* boundary. Certification is
conditional on P1/P6, which the presentation **declares** and the engine only
bounded-checks, so a pass covers the profile plus a promise. The evolution
clause is undischarged: `soc-core` has no lowering dependency and cannot
validate that a caller derived its revision identity canonically (**#178**).
And by precedent — ADR-0013 fully pinned certificate identity with frozen
vectors and independent reproduction, and SOC-LAW-01/12 still stayed Partial.

**Evolution.** Observation profiles and saturation certificates are versioned
semantic artifacts and include the exact context and program/world revision.

## SOC-LAW-11 — Translation fidelity

**Domain and context.** A frontend, lowering, optimizer, backend, adapter, or
formalism translation; its declared source fragment; target representation;
and satisfaction/realization relation.

**Rule.** Every translation MUST state its supported fragment and the relation
it preserves. It MUST reject or report unsupported input rather than invent
meaning. A successful translation must preserve canonical source context,
types/realization, epistemic grade, and required provenance.

**Authority and evidence.** ADR-0009 and the current syntax→lowering→type-
realization fixtures provide fragment-specific parity. ADR-0011 adds ordinary
closed-sum match lowering plus a distinct kernel-certified coverage result for
`proving exhaustive`. Native analysis reports conflicts separately from
positive realization. There is not yet one generic translation-certificate
contract.

**Failure form.** Unsupported constructs, unresolved names, arity/type errors,
or source/target disagreement produce diagnostics or a minimal differential
counterexample, never a fabricated successful judgement.

**Evolution.** #52 defines regime results, #53 the type-realization contract,
and #178 the remaining language/runtime bridges. Each new translation registers
its relation and fixtures in this law map.

## SOC-LAW-12 — Verifier closure

**Domain and context.** A purported `Proven` or `Refuted` result and its exact
canonical proposition, explicit term/refutation artifact, context, kernel
profile, verifier identity, certificate, and semantic resource contract.

**Rule.** Every theorem-pole judgement MUST be independently checkable from
those artifacts without proof search, ambient state, or a presentation-layer
claim. Acceptance is total within its declared budget. Exhaustion is
`Unknown`; rejection of one proof candidate is not `Refuted`.

**Authority and evidence.** `brix-kernel::acceptance` checks explicit terms and
returns exhaustive verdicts; adversarial fixtures cover malformed, unsupported,
context mismatch, rejection, and exhaustion. ADR-0011's coverage builder also
constructs a term that the kernel independently accepts or rejects; missing
variants and unsupported wildcard/nested shapes never mint a coverage theorem.
Native code currently mints only accepted certificates, not refutation
certificates.

**Failure form.** Missing/malformed artifacts, unknown profiles/verifiers,
context mismatch, unsupported constructs, and exhausted budgets fail closed.
Certificate identity, its total envelope validator
(`brix_kernel::validate_material_v1`), and its frozen vectors are in place
(ADR-0013); the absence of a durable on-disk explicit-term artifact format
keeps this law Partial under #56/#58.

**Evolution.** A verifier or calculus profile needs a canonical versioned
artifact format, frozen independent vectors, total validation, and an explicit
compatibility policy.

## Executable conformance protocol

The companion JSON names the exact ADR, implementation, and test anchors for
each law. CI runs:

```sh
python3 scripts/check_soc_law_map.py
```

The checker fails if law IDs/statuses drift, a referenced path or test function
disappears, a non-Enforced law loses its bounded issue, or this normative
document loses a registered section. Cargo CI then executes the referenced Rust
tests. This is traceability, not a substitute for the validators themselves.

For any conformance run:

1. Pin the law ID, profile/version, exact `ContextId`, revision, implementation,
   observation profile, and semantic resource limits.
2. Run the named validator and executable gates.
3. Record success evidence or the smallest reproducible counterexample.
4. Report unsupported/exhausted/unavailable as such. Do not count them as pass.
5. When a law is broadened, update this document, the manifest, and its gate in
   the same change.

## End-to-end grading example

Consider:

```brix
fn id(n) = n
let literal = 42
let through_id = id(42)
```

The current executable path makes the distinction visible:

1. `brix-syntax` parses untrusted surface syntax; `brix-lower` resolves `id`
   and lowers both bindings into `type_realization::Expr`. Unsupported or
   ill-typed input stops with a diagnostic under SOC-LAW-11.
2. There is no longer a linear route. `type_check`/`audited_type_check` were
   retired by ADR-0018 (#262): they padded their configuration chain to
   `[src, dst, dst, …]`, so the decomposition they recorded misstated its own
   intermediate configurations, and nothing called them. The A-3 bypass this
   step used to describe is thereby closed — the tree route below is the only
   typing publisher.
3. `audited_type_check_tree` validates a well-formed realization tree and
   publishes an `Audited` source judgement. The settlement reference slice
   separately demonstrates the authority-correct `commit_tick` (`Derived`) →
   `audit_step` (`Audited`) transition in
   `literal_vertical_slice.rs`. Closing the production bridge is outstanding;
   the conformance map records that rather than inventing an end-to-end claim.
4. `elaborate_tree` constructs an explicit term and the proof kernel can
   certify the **composition theorem conditional on its primitive leaves**.
   That kernel certificate is genuinely `Proven` as a proposition about the
   supplied premises, but SOC-LAW-02/04 forbid laundering it into an
   unconditional typing result.
5. `honest_result_outcome` inspects leaf discharge. Literals, the STLC core,
   and the nonempty product/coproduct typing schemas rest only on discharged
   tight generators, so their final contextual typing results can be
   `@Proven`. Arithmetic, zero-field records, nullary constructors, and
   wildcard/variable catch-all matches retain undischarged leaves and remain
   `@Audited` even though the kernel accepts their conditional composition term.

The fixtures `test_let_lit_earns_proven`, `test_id_fixture_proven`,
`literal_equality_derives_then_audits_the_reflexive_witness`, and
`test_b3_end_to_end_audited_decomposition_to_proven` pin the four distinct
steps. They do not close #228's publisher API gap, #59's context semantics, or
#56's durable proof-artifact encoding.

ADR-0011 adds an orthogonal example:

```brix
config Opt = None | Some(Int)
let selected = match Some(3) {
  None => 0
  Some(k) => k
} proving exhaustive
```

Here the explicit-constructor match typing result is `@Proven`, while the
separate coverage proposition is independently `@Proven` because the kernel
accepts the closed-sum eliminator. Removing `None` makes coverage non-provable;
using the currently unsupported wildcard certificate shape yields
`CoverageOutcome::Unknown`, and its catch-all typing leaf remains `@Audited`.
The fixtures
`proving_exhaustive_match_gets_kernel_certified_coverage`,
`missing_variant_is_not_certified`, and
`wildcard_is_outside_the_certified_fragment` make that separation executable.
