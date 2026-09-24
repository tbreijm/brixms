# Proposed BrixMS language issues

These are local issue drafts for review. The examples marked **proposed syntax** describe the requested behavior; they are not claims that the snippets parse today. Contract names and reported module/exporter behavior below come from the supplied suite-world roadmap and have not been independently verified against suite-world files.

## 1. Bounded list inputs and folds

**Title:** Add bounded list inputs and deterministic collection folds

**Problem**

The accepted external-input contract `brix.input@1` is scalar-only. ADR-0033 is a pending, uncommitted implementation proposal for records and sums with `brix.input@2`; local implementation work is in progress, so this support should not be described as shipped. Neither the published `@1` contract nor pending `@2` defines list transport or collection folds. Modules therefore cannot yet receive a bounded collection of vehicles, modifiers, corrections, or bids and compute over it in Brix. This is separate from rule-schema quantification: folding a supplied finite list computes a value; it does not instantiate rules over facts or establish a rule-level search policy.

**Requested behavior**

Extend structured input types with bounded `List<T>` values, where `T` is an admitted scalar or nominal schema type. A declaration must state or inherit a finite maximum length, enforced during transport decoding before unbounded allocation. Preserve strict decoding, canonical identity, and fail-closed behavior. Do not retrofit composite values into published `brix.input@1`. Settle the transport version and encoding for structured lists before this feature lands: ADR-0033's `brix.input@2` remains pending, so decide whether bounded lists belong in that proposal or require a later version. This draft does not decide that version boundary.

Provide bounded, deterministic folds over a list whose length is fixed by the validated input snapshot: integer `sum`, predicate `count`, `all`, and `any`. Specify empty-list behavior (`sum` and `count` are zero; `all` is true; `any` is false), evaluation order, checked integer overflow, and evaluator work accounting. Each element is evaluated at most once per fold unless a construct explicitly says otherwise.

**Proposed syntax (illustrative, not runnable today)**

```brix
config Vehicle = { lf: Int, price_cents: Int }
input vehicles: List<Vehicle> max 64

let total_lf = sum(vehicles, v => v.lf)
let all_priced = all(vehicles, v => v.price_cents > 0)
let priced_count = count(vehicles, v => v.price_cents > 0)
```

The precise list declaration and lambda syntax are open design points. The contract is a finite list with an enforced width bound and folds that cannot outlive or grow their input domain.

**Roadmap contracts this may unblock**

- Batch split at total LF 99.
- Per-car price allocation.
- Legacy prefix and suffix modifier chains.
- V3 explain distance bands and the Segment 7 ladder.
- Correction ledger active-correction totals and cash allocation.
- Recognized economics, finance, and monthly aggregates.
- Auction bid history.

**Acceptance criteria**

- List values have a versioned canonical input representation; list order is preserved and bound into snapshot/context identity.
- Maximum length is checked before allocation and is included in the input contract. Nested list/record bounds compose with ADR-0033 depth, node, and width limits.
- Invalid element types, oversized lists, duplicate object keys, and malformed nested values fail closed.
- Fold semantics define empty inputs, overflow, ordering, evaluation faults, and work limits.
- A fold over a list is documented as expression-level computation only. It does not claim rule-schema quantification, strict-first matching, or witness composition.
- Published `brix.input@1` retains its scalar meaning and identity. Resolve the pending ADR-0033 `@2` record/sum contract before assigning list encoding a transport version.

**Local references**

- [ADR-0033: structured inputs and composite function contracts](../../spec/adr/ADR-0033_Structured_Input_Contracts.md) proposes `brix.input@2` for records and sums and defines structured resource limits; it is not yet shipped.
- [ADR-0031: external inputs](../../spec/adr/ADR-0031_External_Input_Alpha.md) defines the frozen scalar `brix.input@1` contract and strict bounded shard transport.
- [ADR-0032: pure functions](../../spec/adr/ADR-0032_Finite_Decision_Functions.md) describes bounded helper evaluation and checked runtime faults.

## 2. Exact integer division and rounding

**Title:** Add explicit signed integer division, rounding, and modulo operations

**Problem**

The current finite-decision lowering rejects division (`DivisionNotAllowed`), even though exact signed integer arithmetic is available. The supplied suite-world roadmap reports workarounds where callers supply a rounded result and Brix checks inequalities. That is cumbersome for LF calculations, per-car allocation, ratios, and percentages, and pushes the arithmetic result outside the audited expression.

**Requested behavior**

Add explicit signed integer operations with named rounding behavior: `div_floor`, `div_ceil`, and `div_half_even`, plus Euclidean modulo. All operations use exact integer semantics, checked intermediate/result bounds, and no floating point. Division or modulo by zero is an evaluation fault that yields no committed decision. Define `mod_euclid(a, b)` by `a = b*q + r`, where `0 <= r < abs(b)` and `q` is the corresponding Euclidean quotient; do not pair the remainder with `div_floor` blindly when `b` is negative. `div_*` on `i64::MIN` and `-1` faults on quotient overflow. Specify modulo's result independently for that pair (mathematically zero), so it need not fault merely because the quotient does not fit in `i64`. Intermediate arithmetic used to determine absolute values or remainders must itself avoid overflow, for example by using a wider checked representation.

`div_half_even(a, b)` rounds the exact rational quotient to the nearest integer, resolving an exact halfway case toward the even integer. The specification must state behavior for negative quotients as well as positive ones.

**Proposed syntax (illustrative, not runnable today)**

```brix
let lf_units = div_half_even(total_lf, 250)
let per_car_cents = div_floor(price_cents, car_count)
let remainder = mod_euclid(price_cents, car_count)
```

The names and call form are proposals. This example does not imply current parser or evaluator support.

**Roadmap contracts this may unblock**

- LF conversion using `/250` and specified rounding.
- Price division across cars.
- Ratio and percentage rules that currently depend on caller-supplied rounded values.

**Acceptance criteria**

- Signed semantics are fully specified, including ties, negative divisors, zero divisors, and overflow.
- Include examples such as `div_floor(-7, 2) = -4`, `div_ceil(-7, 2) = -3`, and a negative halfway case for `div_half_even`; specify the result of `mod_euclid(7, -3)` through its defining identity.
- Division-by-zero, overflow, and invalid arithmetic yield typed evaluation faults and never produce a partial or committed result.
- Arithmetic behavior is deterministic and included in the evaluator-semantics version bound into program identity.
- Audit and replay use the same operator semantics and reproduce the result or fault.
- No floating-point values enter decision arithmetic.

**Local references**

- [ADR-0030: finite-decision alpha](../../spec/adr/ADR-0030_Finite_Decision_Alpha.md) defines the finite-decision profile. [ADR-0027: L3 v2 derivation](../../spec/adr/ADR-0027_L3_V2_Derivation.md) concerns the separate rule-agenda execution profile and provides relevant general versioning and termination constraints; it does not define the finite-decision arithmetic profile.
- [ADR-0032: pure functions](../../spec/adr/ADR-0032_Finite_Decision_Functions.md) requires bounded checked evaluation and replay through the same evaluator.
- `crates/brix-lower/src/l3_v2.rs` currently maps `/` to `DivisionNotAllowed` and has checked arithmetic-fault machinery.

## 3. Audited witness composition

**Title:** Lower and audit sequential and parallel witness composition

**Problem**

L3 source accepts the `then` and `and` operators syntactically, but the current L3 lowering rejects witness composition. The supplied suite-world roadmap reports that existing order modules do not form one composed witness chain; this draft has not independently checked that suite-world claim. Source-level composition would let the system represent and audit a multi-stage chain.

**Requested behavior**

Define and lower `then` as sequential witness composition and `and` as parallel/tensor composition, with explicit typing, endpoint, context, and grade rules. A composed witness must retain its actual intermediate configurations and component witnesses. Check endpoint alignment semantically before composing: a hash of component witness IDs alone does not show that their source and destination configurations join. Permit the same-context tensor case when its context requirements are checked; combining distinct contexts requires an explicit, checked context bridge, with no claim that plain `ContextId` equality proves confinement. Composition must not upgrade evidence grades: a composite requires checked component claims and cannot exceed the weakest component. Failures or missing components must fail closed without publishing a successful composite witness.

**Proposed syntax (illustrative, not runnable today)**

```brix
witness order_flow = prepare then bind_quote then reserve then persist then effects then handoff
witness pricing = resolve then match_quote then compute_price then allocate
```

The spelling follows parsed operators but does not claim the declarations lower or produce an audited witness today. Exact declaration placement and grouping syntax remain to be specified.

**Roadmap chains this may enable**

- Order: prepare → bind-quote → reserve → persist → effects → handoff.
- Pricing: resolve → match → compute → allocate.
- The corresponding lifecycle chain.

**Acceptance criteria**

- Specify sequential endpoint compatibility and parallel source/context compatibility.
- Preserve each real intermediate configuration and component identity; do not pad a chain or infer intermediate states.
- Provide a replay/checking path that independently validates each component, sequential endpoint alignment, and the composition identity.
- Specify the admissible same-context tensor case and require an explicit checked bridge for cross-context composition; do not make completion of all of issue #59 a prerequisite for every restricted composition form.
- Keep the existing authority and grade boundaries; composition alone cannot claim `Audited` or `Proven`.
- Bind composition structure into stable program/witness identity and preserve frozen profile behavior through a versioned extension where required.
- Demonstrate one complete audited chain. This changes what can honestly be claimed about composition; it does not add suite-world contract coverage by itself.

**Local references**

- `crates/brix-lower/src/l3.rs` defines `WitnessCompositionNotAllowed` for parsed `then`/`and` forms.
- [ADR-0007: tree-structured typing and elaboration](../../spec/adr/ADR-0007_Tree_Structured_Typing_Elaboration.md) specifies structured witness composition and identity.
- [ADR-0018: retire the flat typing lane](../../spec/adr/ADR-0018_Retire_The_Flat_Typing_Lane.md) explains why composed chains must carry real intermediate configurations.
- [ADR-0016: authority publication fence](../../spec/adr/ADR-0016_Authority_Publication_Fence.md) defines the authority boundary composition must preserve.

## 4. Finite rule schemas and quantified guards

**Title:** Add finite rule schemas with bounded grounding and ordered matching

**Problem**

ADR-0027 describes rule schemas and guards as the v3 direction for the rule-agenda profile: an ordinary rule dependency names one earlier fact, while a schema ranges over a family of values and needs an explicit grounding discipline. The supplied suite-world roadmap identifies ordered filter matching (“for each filter in order, choose the first one that yields exactly one row”) as an unmet need; this draft has not independently verified suite-world implementation status. Collection folds over input values do not solve this problem: rule-level quantification creates rule instances/facts and needs its own termination, identity, and evidence rules. Keep two mechanisms distinct: finite pre-grounding can produce an acyclic set of rule instances; ordered first-success selection among matching outcomes belongs to the proposal/deliberation layer and needs an explicit priority/selection rule, rather than being hidden in monotonic rule derivation.

**Requested behavior**

Add a finite, explicitly bounded schema mechanism that grounds rule or proposal instances over an admitted finite domain. A rule-schema guard may quantify over grounded values and derive facts through the declared rule model; finite pre-grounding must preserve acyclicity and commit bounds. Bound the grounding domain and total instantiated rules/proposals/work before evaluation, and define canonical instance identities. For ordered matching, represent the choice in the proposal/deliberation layer: distinguish no result, exactly one result, and ambiguity; state how priorities encode strict-first and fallback behavior, and make the selection deterministic. Portal eligibility may be expressed as grounded declarative rules or proposals according to whether it derives facts or selects an outcome, rather than as caller-precomputed scalars.

**Proposed syntax (illustrative, not runnable today)**

```brix
rule has_match(order, filter) over filters(order) {
  when exactly_one(row in rows(order, filter)) => Matched(filter, row)
}

propose choose_match(order, filter) over filters(order)
  priority filter_rank(filter)
  when has_match(order, filter) = true
```

This is notation for the requested contract only. It is not a current grammar proposal with settled parsing, typing, or semantics. The first construct sketches finite fact derivation; the proposal sketches ordered selection. In particular, folds over `List<Record>` inputs from the list issue do not imply that `over`, proposal schemas, or quantified rule guards exist.

**Roadmap contracts this may enable**

- A proper `match@1` over ordered filters, where the first filter yielding exactly one row wins.
- Strict-first and fallback policy.
- Portal-eligibility rule sets.

**Acceptance criteria**

- Specify the finite grounding source, maximum domain and instance counts, and preflight resource checks.
- Preserve acyclicity and derive termination from a finite pre-grounding/commit bound for rule instances. If schemas can be generated from a changing fact base, specify and check the required termination discipline. Do not rely on an input-fold bound as a substitute.
- Define stable identity for schema definitions, grounded instances, and ordering; replay reconstructs them from source and inputs.
- Define ambiguity and failure behavior, including exactly-one semantics and strict-first versus fallback selection in the deliberation layer.
- Ensure a schema instance cannot read undeclared facts or silently widen its grounding domain.
- Preserve evidence grades and fail closed on exhausted limits or evaluation faults.

**Local references**

- [ADR-0027: L3 v2 derivation](../../spec/adr/ADR-0027_L3_V2_Derivation.md), especially §3 on the distinction between one-fact dependencies and quantifying rule schemas, and §4 on termination.
- [ADR-0033: structured inputs](../../spec/adr/ADR-0033_Structured_Input_Contracts.md) supports nominal values but does not define rule schemas.
- `crates/brix-lower/src/l3_v2.rs` represents declared dependencies as one named earlier rule and rejects `then`/`and` and division in its current lowering path.

## Alongside lists: Boolean expression operators

**Title:** Add short-circuit Boolean `&&`, `||`, and `!` expressions

Boolean operators do not add suite-world contract coverage by themselves, but they substantially shorten policy expressions. The supplied roadmap reports current modules encode conjunction through nested `match`; that suite-world behavior is not independently verified here. Add ordinary expression operators with a defined precedence table, type errors for non-Boolean operands, and short-circuit semantics. Lazy evaluation may skip dynamic faults in an unevaluated branch, as with an unselected `match` arm, while static checking still validates the whole expression. Use `&&`, `||`, and `!` so the existing `and` spelling remains available for witness composition.

**Proposed syntax (illustrative, not runnable today)**

```brix
let eligible = has_stock && is_allowed && !is_blocked
```

Consider delivering these operators with the first usability milestone that changes the expression grammar. They can ship independently from witness-level `and` composition because their spellings are distinct.

**Acceptance criteria:** Specify precedence/associativity and short-circuit fault behavior; include canonical encoding and replay; preserve existing behavior for expressions without these operators. Include an example such as `false && (1 div_floor 0)` and specify whether the skipped right branch faults at runtime (proposed: it does not, while its syntax and static types remain checked).

## Follow-up: per-fact `ConfigId` in the public L3 API

**Title:** Expose the actual configuration identity for each exported fact

The supplied suite-world roadmap reports that the public L3 witness exporter leaves per-fact `ConfigId` empty rather than inventing one; this is not asserted here as a verified suite-world finding. Locally, `DerivedFact` at `crates/brix-lower/src/finite_decision/runtime.rs:85` contains rule, value, ordinal, and grade but no `ConfigId`; `FactJson` at `crates/brix-cli/src/json.rs:173` likewise has no per-fact `ConfigId`. Add an API that exposes the actual identity once the representation can provide it. If a fact has no corresponding configuration identity, preserve that absence explicitly and do not synthesize a placeholder. This follow-up is separate from the list/division coverage estimate.

**Local reference:** `crates/brix-lower/src/finite_decision/runtime.rs` and the public witness/export API should be reviewed to pin the current empty-field behavior before filing.

## Follow-up tracking note: context contents and SOC-LAW-06 (#59)

Use existing issue #59 for the missing public, replayable context-content representation and SOC-LAW-06 verifier; do not file a duplicate. Context identity binding program and input snapshot does not by itself prove context confinement. The issue should require re-deriving context contents from source and supplied inputs, binding the representation to the existing context identity, independently verifying the confinement law, and failing closed when required material is absent or inconsistent. Until that law is discharged, modules using real-data adapters should keep claims bounded and must not claim context confinement.

**Local reference:** [SOC semantic laws](../../spec/conformance/soc-semantic-laws.json) contains SOC-LAW-06. The association with existing issue #59 was checked by the parent agent against the tracker.

## Coverage note

The estimate that bounded list folds plus division/rounding would move coverage from approximately 52 to 64 of 87 is user-supplied and unverified. Treat it as a planning hypothesis until the suite-world contract inventory is checked. The drafts above cite only the contract names supplied with the request and do not invent suite-world file references.

## Requested roadmap order

Keep the requested delivery order: bounded list inputs/folds first, division/rounding second, witness composition third, and finite schemas fourth. Deliver Boolean operators alongside the list milestone. The order is a planning preference; each milestone still needs its own profile, identity, resource, and evidence review. In particular, a finite pre-grounding design can preserve acyclic rule derivation, while ordered first-success choice must be specified in deliberation.
