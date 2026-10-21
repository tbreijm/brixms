# ADR-0048 — Evidence-bearing types and proof-carrying judgments

Status: **Proposed** (2026-10-17). This ADR specifies a language feature. It
does not implement it, does not reserve new keywords, and does not move any
surface into [`Beta_Contract.md`](../Beta_Contract.md) **Stable**.

Every syntax block below is **illustrative proposed notation**. It is not a
claim about the current parser, and source text must not advertise a proof
capability before the corresponding certificate route exists.

Foundation documents: [ADR-0001](./ADR-0001_Proof_Substrate.md) (outcome
lattice, authority, retraction), [ADR-0003](./ADR-0003_Proof_Kernel_Profile.md)
(independent kernel, explicit terms, budget),
[ADR-0013](./ADR-0013_Canonical_Certificate_Envelope.md) (fail-closed
certificate identity), [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md)
(a typing discharge is only `HasType`),
[ADR-0016](./ADR-0016_Authority_Publication_Fence.md) (who may publish which
outcome), [`Type_Realization_Contract.md`](../Type_Realization_Contract.md)
§10, and [`SOC_Semantic_Laws.md`](../SOC_Semantic_Laws.md) SOC-LAW-04, 05, 06,
09, and 12.

## 0. Milestone

**Beta 1: proof-carrying judgments with a stable, bounded proof profile.**

The evidence required for a claim is part of the language’s enforceable
contract, not merely something attached to its explanation. The goal is that
nothing can claim more than its evidence establishes. The goal is not that
every construct is proven.

Two scopes are distinct, and only the first is the foundation:

1. **Proof-producing typing.** A supported fragment emits checkable `HasType`
   certificates, and unsupported constructs keep their weaker evidence status.
2. **Proof-backed policy claims.** An operation requires a named proposition
   about a named subject in a named context. This is a larger milestone. It
   is admitted only after that proposition’s evaluation semantics and context
   handling are explicit judgments, not relabeled typing results.

[`docs/planning/beta-roadmap.md`](../../docs/planning/beta-roadmap.md) currently
deprioritizes further kernel discharge and context confinement. Accepting this
ADR is the decision that reverses that ordering for the bounded profile
defined here. Until this ADR is accepted, that roadmap remains the plan.

## 1. Three claims

These claims are not interchangeable. Evidence for one does not establish
either of the others. A proof from explicit assumptions establishes its
conclusion under those assumptions; it does not establish the assumptions.

| Claim | What it establishes |
| --- | --- |
| This expression has type `Bool`. | The expression satisfies the typing rules. |
| This expression evaluates to `true`. | A particular computation produces that result under specified semantics and inputs. |
| This order satisfies the dispatch policy. | The stated policy conditions follow from the specified facts and assumptions. |

[`Type_Realization_Contract.md`](../Type_Realization_Contract.md) §10.3 already
fixes the first distinction: discharging a typing rule discharges `HasType`
and nothing else — not evaluation, value equality, totality, progress, or
termination — and no `EvaluatesTo` judgment exists. A policy claim is a third
judgment. Introducing it requires a new, explicitly defined proposition. It
is not a new label on a typing certificate.

A proof that a stock check has type `Bool` does not prove that stock is
sufficient. Proof of compliance with a modeled policy is not proof that the
policy is good, and it is not proof that an external source reported reality
correctly.

## 2. Current substrate

This feature completes and integrates existing machinery. It does not replace
it.

**Proof kernel.** `brix-kernel` checks an explicit proof term against a
proposition in an assumption context and returns a total verdict
(ADR-0003). It depends only on `brix-semantic` and `brix-canon`. Proof
search, tactics, and solvers stay outside the kernel. `Accepted` is the sole
source of a kernel proof. `ResourceExhausted` is not `Refuted`. Rejection of
one candidate is not a refutation of the proposition (SOC-LAW-12).

**Typing realization.** `soc-regimes::type_realization` separates “this
comparison is well-typed” from “this comparison is true.” `g_cmp` is
deliberately undischarged: `@Audited` grades the typing derivation, never the
truth of the comparison. Arithmetic typing is capped by the undischarged
bridges `g_arith_input` and `g_arith_result`. `TypingArithV2` decides
`g_arith`’s realization relation, and no certificate yet closes that leaf
([`Type_Realization_Contract.md`](../Type_Realization_Contract.md) §11;
ADR-0023). Those leaves are the concrete completion targets for the typing
fragment. They are not a reason to prove every construct at once.

**Authority.** `Judgement::publish` checks `(authority, outcome, evidence
kind)` against `ROUTES` (ADR-0016). Only `Authority::ProofKernel` may publish
`Proven` or `Refuted`. SOC-LAW-04 and SOC-LAW-05 remain **Partial**: #228
still records remaining public-construction and elaboration-boundary work.
A runtime derivation is not automatically an audit, and an audit is not
automatically a theorem.

**Context and history.** SOC-LAW-06 (context confinement) is **Open** and
owned by #59. The substrate proves stable identity, deterministic extension,
and kernel digest mismatch rejection. It does not yet validate context
contents, transport, or confinement. SOC-LAW-09 (correction and retraction)
is **Partial**: durable history is distinguished from revision-scoped
support, and a complete invalidation traversal has not landed. Incremental
decision maintenance (SOC-LAW-08) is agreement of candidate views. It is not
incremental proof maintenance. [`Type_Realization_Contract.md`](../Type_Realization_Contract.md)
§11 records that incremental invalidation of typing derivations does not
exist.

**Compatibility.** [`Beta_Contract.md`](../Beta_Contract.md) covers the
finite-decision profile. It leaves the `let` lane, type realization, the
proof kernel, certificates, and the L3 v1 profile outside its stability
guarantees. This ADR does not change that sentence. A later acceptance stage
that wants a stable proof profile amends the beta contract explicitly, with
frozen vectors for any new artifact schema.

## 3. Decision

Preserve the separation between a value’s type and the evidence supporting a
judgment. An operation may declare the exact proposition and evidence
strength it requires. Static checking enforces how that evidence flows.
Runtime certificate checking applies when the facts become available. Not
every concrete proof has to exist at compile time.

Illustrative shape, not current syntax:

```text
dispatch(
    order: Order<ctx>,
    evidence: Proof<CanDispatch(order.id), ctx>
) -> DispatchDecision<ctx>
```

`ctx` identifies the relevant world, revision, policy, and assumptions. The
intended guarantee is that this dispatch path accepts the order only when
the caller supplies checked evidence for that order, under that policy, in
that context. A proof about another order, a hypothetical scenario, an
obsolete revision, or a different policy does not satisfy the requirement.

For a proof-required operation, failed proof search means the required
evidence is unavailable. It does not mean the proposition is false, and it
does not mean the operation is accepted anyway.

## 4. Out of scope for the first profile

Unrestricted dependent programming, general recursive theorem proving,
automatic discovery of business policy, and a new effects executor are
outside the initial profile. External provers may produce candidate
certificates. They cannot replace the independent acceptance boundary. Proof
generation must not become a mandatory unbounded operation on the decision
hot path.

Decimal predicates, division, bounded collection predicates, and quantified
propositions require separately specified extensions. There is no implicit
coercion to floating point or to mathematical unbounded integers.

Finite-decision coverage and persistent-world coverage are declared
separately. Support in one profile does not imply support in the other.

## 5. Requirements

### R1 — Nominal semantic types

Programs must be able to declare distinct nominal types with the same
underlying representation. Declared indices may distinguish units,
dimensions, versions, or other immutable identities. An alias must remain
distinguishable from a new nominal type in syntax and diagnostics.

```text
newtype CustomerId = Str
newtype Quantity<Unit> = Decimal

fn add<U>(a: Quantity<U>, b: Quantity<U>) -> Quantity<U>
```

Values with different indices are not implicitly interchangeable. A
conversion names its conversion rule and any required parameters. A nominal
name alone establishes neither a predicate nor an external fact. The initial
index language is limited to declared nominal tags, admitted immutable
scalar values, and canonical identities, without arbitrary evaluation during
type equality.

### R2 — Refinement types

A refinement `{ x: T | P(x) }` denotes values of `T` satisfying a declared,
pure predicate. Constructing a value requires establishing the predicate
through an admitted check or proof route.

```text
type PositiveInt = { x: Int | x > 0 }
type BoundedIndex<n: Int> = { i: Int | 0 <= i && i < n }
```

Predicates have no I/O, mutable global reads, clock reads, or nondeterministic
operations. External state and time enter as explicit immutable parameters.
The language rejects predicates outside the declared fragment, or leaves
their obligations explicitly unresolved. It does not assume them true.

The initial predicate fragment covers Boolean combinations, equality of
admitted canonical values, ordered integer comparisons, and checked integer
addition and subtraction. Partial operations generate definedness and
overflow obligations.

Refinement subtyping requires an admitted argument that the source predicate
entails the target predicate. Forgetting a refinement to use the underlying
value is safe and follows a documented widening rule. Strengthening a
refinement requires a new obligation. Widening does not let the original
evidence establish a different claim. Mutable containers do not permit
unsound writes through variance or aliasing.

### R3 — Function contracts

Functions may declare preconditions on arguments and postconditions relating
arguments to the returned value. Callers establish preconditions.
Implementations establish postconditions for each successful return.

```text
fn increment(x: Int) -> Int
  requires x < Int.max
  ensures result == x + 1
```

A contract with an explicit failure variant constrains the successful
result. It does not silently eliminate failure. Declaring a postcondition
is not evidence that the implementation satisfies it. A conditional result
property does not by itself prove termination or that a successful result
will exist.

At control-flow joins, only facts established on every reaching branch are
retained without additional evidence. Exported signatures retain the
contracts consumers need. A library cannot hide an unchecked assertion
behind an apparently stronger return type.

### R4 — Evidence names an exact proposition

The conceptual interface is `Evidence<P, Context, Authority>`, where `P` is
the claim, `Context` identifies its assumptions and semantic dependencies,
and `Authority` identifies the accepted verification route. Surface syntax
need not use these exact generic parameters.

An evidence requirement may name a value by a canonical identity or refer to
bound function arguments. Evidence about one value does not satisfy a
requirement about another merely because their base types match.

```text
fn consume<T>(value: T, evidence: Evidence<Valid(value), C, ProofKernel>)
```

### R5 — Verification routes stay distinct

A successful local predicate check may construct a refined value within its
declared runtime trust boundary. It must not manufacture a kernel proof
certificate. The evidence model preserves at least these distinctions:

| Route | Permitted claim |
| --- | --- |
| Runtime derivation or predicate check | The declared computation or check succeeded under the named semantics and inputs. |
| Independent audit | The exact proposition supported by the admitted audit procedure, such as agreement with a replayed computation. |
| Proof kernel acceptance | The exact proposition follows from the explicit context through an accepted certificate. |

These are authority requirements, not freely castable annotations.
Independent replay does not automatically prove arbitrary mathematical
properties. A stronger route satisfies a weaker consumer only through an
explicitly defined entailment rule for the same claim and a compatible
context. There is no generic grade comparison that substitutes unrelated
evidence.

### R6 — Evidence construction is protected

Ordinary source code, external input, casts, and deserialization do not
construct accepted evidence by setting an enum or string. Only the admitted
verifier issues the corresponding accepted handle. A serialized certificate
is an untrusted candidate until it is checked against the expected
proposition, context, and verification profile.

Proof search, solvers, compiler inference, and AI-generated terms remain
outside the proof kernel. They may propose proofs. The existing independent
kernel checks those proofs using an explicitly supported calculus. Any new
primitive requires its own semantics and verification justification. It
cannot enter as an unexplained trusted solver answer.

### R7 — Every obligation has an explicit disposition

The compiler attempts checks in a bounded, reproducible fragment. If an
obligation cannot be discharged statically, the program provides evidence,
chooses an explicit runtime validation boundary, or fails checking at the
point that requires the stronger contract. Compilation never inserts an
invisible assumption and never silently weakens a required contract.

```text
match validate<PositiveInt>(raw) {
  Satisfied(value, receipt) => use_positive(value)
  Violated(details)         => handle_invalid_input(details)
  Unknown(reason)           => handle_unresolved_check(reason)
}
```

`Satisfied` identifies the checking route used. It does not universally mean
`Proven`. An external input can be checked against a predicate without
proving the external source truthful.

### R8 — Failure semantics are precise

A completed concrete predicate check returning false is a violation for that
value. A rejected proof candidate establishes only that the candidate was
not accepted. Neither result is a universal logical refutation. Publishing
`Refuted(P)` requires the admitted certificate for the negation or another
existing refutation route.

Resource exhaustion, missing evidence, unsupported proof constructs, and
incomplete search do not become false, `Refuted`, or a satisfied obligation.
Diagnostics distinguish these reasons from malformed input and context
mismatch. Each consumer handles unresolved evidence explicitly. An
unavailable proof cannot authorize a branch that requires it.

### R9 — Verification is bounded

Publish limits for predicate work, proof size, recursion or normalization
depth, memory, and imported dependencies. Budget exhaustion terminates
checking with an explicit unresolved result. A change in search strategy or
budget may affect proof discovery. It does not change the meaning of the
proposition or the acceptance rules.

### R10 — Contexts are inspectable and bound to evidence

A context identifies the relevant type definitions, contracts, numeric
semantics, input snapshots, assumptions, and dependencies. Identity digests
are useful references. Verification resolves and validates the material they
identify. Hash equality alone is not proof that the context is well formed
or sufficient.

Assumptions are visible in the interface or in evidence inspection output.
An unchecked assumption may support a conditional proposition. It does not
satisfy an unconditional evidence requirement. There is no exportable
`assume` or escape hatch that silently turns supplied facts into established
premises.

This requirement depends on SOC-LAW-06. A restricted first release that
cannot yet validate context contents must exclude the uses that need that
guarantee, and must say so. It must not treat digest equality as confinement.

### R11 — Historical validity and current applicability are separate

A certificate about snapshot A remains a certificate about A after state
changes. It does not automatically satisfy a contract about snapshot B.
Reuse is permitted only when the relevant dependency identities remain
valid, or a checked transport or equivalence argument establishes
applicability to the new context.

An incremental runtime invalidates derived applicability when any relevant
dependency changes. A refined value that depends on mutable state either
captures an immutable snapshot or is revalidated before use. Mutation does
not leave a stale refinement attached to the value.

Facts such as freshness, permission, or resource availability are claims
relative to explicit observations and time or state parameters. Proofs do
not freeze external systems. An effectful consumer remains responsible for
checking current preconditions at its execution boundary. A receipt alone
does not supply atomicity and does not prevent duplicate effects.

For the first persistent-world stage, conservative rechecking is acceptable.
Optimizing proof reuse comes after establishing that stale evidence cannot
pass. Deleting an old proof, or declaring it false because a later revision
differs, is not the rule. SOC-LAW-09’s non-erasure rule stays in force.

### R12 — Contracts and evidence compose at checked boundaries

Sequential composition establishes that the earlier postcondition entails
the later precondition, with matching subjects, types, and contexts.
Parallel composition checks the declared compatibility conditions rather
than concatenating certificate identifiers.

Evidence retains its actual intermediate values or their resolvable
identities, component claims, assumptions, and verification routes.
Composition does not strengthen an unsupported premise. A stronger
conclusion requires a new admitted derivation or certificate for that
conclusion.

### R13 — Generic and cross-module code preserves obligations

Generic declarations expose their required predicates and evidence.
Instantiation substitutes parameters without capturing names or losing
identity constraints. Module imports and exported contracts bind to
versioned semantic identities. Imported proofs are checked using compatible
profiles, regardless of which compiler or solver produced them.

The initial feature supports only the bounded generic and index fragment
specified by its language profile. More expressive type computation is
introduced explicitly in later profiles.

### R14 — Evidence artifacts are portable and versioned

Existing artifact schemas and identities stay frozen. Additions that affect
semantics require new versions. Canonicalization distinguishes semantically
different contracts and preserves existing identities for unchanged legacy
programs.

The canonical envelope carries or resolves:

- the schema and verification profile versions;
- the exact proposition, subject identities, and context identity;
- explicit assumptions and required dependency artifacts;
- the proof or check artifact and its claimed authority route;
- the semantic versions needed to interpret predicates and arithmetic;
- the verification outcome and applicable resource-accounting metadata.

Consumers supply the proposition and context they expect. The artifact does
not choose its own acceptance target. Unknown formats, mismatched
identities, missing dependencies, and tampering fail closed. Producer
metadata, signatures, and content hashes alone are insufficient evidence of
the claim.

Cached verification is keyed by the complete acceptance inputs, including
profile and verifier compatibility. Reusing a cache does not bypass the
consumer’s current resource or applicability policy.

An implementation may erase proof terms from executable code after
verification when execution does not inspect them. It retains resolvable
artifacts required for export, independent verification, or replay. Erasure
does not remove a runtime validation that has not already been discharged,
and it does not erase the evidence needed at a trust boundary.

### R15 — Tooling exposes obligations without overstating guarantees

Extend the existing check, explain, and verification workflows to show the
precise proposition, source location, required authority, assumptions,
supplied evidence, and unresolved reason. Support machine-readable output
for CI and editor integrations.

Diagnostics distinguish wrong nominal type, failed predicate, absent
evidence, insufficient authority, incompatible context, unsupported
fragment, and exhausted budget. A failed solver search is not reported as a
type contradiction. Proven typing is rendered as a typing claim, not as a
proof of evaluation correctness.

## 6. Delivery

Each stage states what is executable, what is independently checked, and
which claims remain unsupported. A stage does not replace a missing proof
with a registry flag or a passing test suite. Tests challenge the
implementation. The certificate and checking rules justify the particular
claim.

1. **Specify and check construction.** Nominal types, the initial predicate
   fragment, obligation outcomes, and authority rules. Checked construction
   and diagnostics. No kernel-proof promotion.
2. **Function contracts and runtime validation.** Module boundaries and
   explicit runtime validation. A local predicate success stays on the
   runtime-check route.
3. **A small certificate fragment.** Certificate generation and independent
   acceptance for a documented proposition fragment, plus portable evidence
   and negative controls. For each admitted rule: premises, conclusion,
   certificate representation, and checking procedure. Anything outside that
   fragment retains its weaker evidence status. Completing `g_cmp` and the
   arithmetic typing bridges belongs here only as `HasType` discharge, under
   ADR-0015 ⟨D-JUDGE⟩.
4. **Persistent applicability.** Checked composition and dependency-aware
   reuse in persistent worlds. Expand predicate and proof fragments only
   with corresponding verification coverage. One narrowly scoped policy
   workflow is admitted in this stage only once its evaluation semantics
   and context handling are explicit.

The acceptance test for the policy workflow, when that stage is in scope:

> An application requests a policy-backed decision, receives a checkable
> certificate, and can use it only for the exact claim and context it
> covers.

Changing the order, policy, revision, or certificate rejects each
inappropriate reuse. An independent checker validates the retained
historical claim without trusting the runtime’s cached answer. The trusted
components and assumptions are documented.

The bounded application path is implemented by
`brix_kb::world::policy_evidence`: a `WorldSession` requests `qty>=1` for an
order in a retained base relation, stores a portable certificate beside that
revision, and checks use against the consumer's exact target. The artifact
includes the revision record and verified Merkle lookup path, so its journal's
quantity is tied to the persisted world rather than a caller-supplied digest.
`verify_dispatch_evidence` checks the portable artifact without a running world
or cached decision. The consumer supplies independently selected world/revision
identities; general context confinement and effect execution remain outside
this slice. See [the world profile](../../docs/world-profile.md#policy-backed-dispatch-requests)
for the API, semantics and trust assumptions, and
`crates/brix-kb/tests/world_policy_evidence.rs` for the acceptance case and
negative controls, including verification in a fresh process after the world
store is removed.

Keyword reservation (`newtype`, `requires`, `ensures`, and any evidence
former) is a compatibility event. It lands with the stage that parses those
words, through the additive process in
[`Beta_Contract.md`](../Beta_Contract.md), and programs that do not use the
new forms keep their existing identities.

## 7. Acceptance criteria

| Case | Required result |
| --- | --- |
| Two nominal types share an underlying scalar | An implicit interchange is rejected. An explicit admitted conversion succeeds. |
| Refinement construction from known valid and invalid constants | The valid case is admitted. The invalid case reports the failed predicate. |
| External value requiring validation | Only the successful validation branch obtains the refined value. |
| Predicate arithmetic can overflow or be undefined | The definedness obligation is discharged, or checking fails explicitly. |
| Function violates its postcondition on one return path | Static verification rejects it, or the declared runtime check prevents a successful refined return. |
| Generic call lacks a required premise | Instantiation reports the missing obligation. |
| Branch-specific evidence crosses a control-flow join | The stronger claim is retained only if every reaching branch establishes it. |
| Caller submits a forged authority label or certificate | No accepted evidence handle is created. |
| Certificate concerns a different value, predicate, or context | Verification refuses the mismatch. |
| Predicate check succeeds without a proof certificate | The value may be locally validated. It is not labelled kernel-proven. |
| Proof candidate is rejected or search exhausts its budget | No logical refutation and no successful authorization is inferred. |
| Proof relies on an assumption | Inspection exposes it. An unconditional consumer cannot silently accept it. |
| Relevant state or contract changes | Old evidence remains historical. Current applicability is rechecked or refused. |
| Unrelated state changes | Reuse is allowed only through the declared dependency rules. |
| Sequential stages have incompatible endpoints | Composition fails with the incompatible obligation identified. |
| Independent verifier runs without the producing compiler or solver | Supported certificates verify using only the admitted verification dependencies. |
| Existing unannotated program and artifact | Behavior and frozen identities remain compatible. |

The implementation includes corruption tests, unsupported-profile tests,
resource-exhaustion tests, and differential tests between direct predicate
evaluation and any certificate-producing implementation of that predicate
fragment. Differential agreement is a regression check. It is not a
substitute for the proof acceptance rules.

## 8. Non-effects

- No change to frozen program, context, input, audit, or knowledge-base
  identities.
- No new `EvaluatesTo` judgment, and no reading of `HasType` as evaluation
  or policy compliance.
- No promotion of `Derived` or `Audited` to `Proven` by annotation, registry
  membership, or test success.
- No claim that SOC-LAW-06 or SOC-LAW-09 is closed.
- No amendment of the beta compatibility contract until a later ADR says
  which proof-profile artifacts, if any, become stable.
