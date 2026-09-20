# ADR-0032 — Pure functions in finite-decision programs

Status: **Proposed implementation**, 2026-09-12. The user authorized the
bounded, nonrecursive function slice; the detailed contract below is the
implementation proposal for review, not a claim of separate ratification.

Extends ADR-0030 and ADR-0031 under `brix.l3.finite-decision@1`. It does not
widen either rule-agenda profile or change the SOC authority boundary.

## Motivation

The frontend already parses pure `fn` helpers, but finite-decision lowering
rejects them. Reusable eligibility and pricing expressions therefore cannot
participate in the otherwise complete execution and audit workflow.

This slice admits existing function syntax without turning helpers into
settlement rules, witness generators, or evidence authorities.

## Call and scope contract

```brix
fn enough(stock: Int, threshold: Int): Bool = stock >= threshold
fn both(a: Bool, b: Bool): Bool = match a {
  true => b
  false => false
}

input stock: Int
rule threshold() = 15
rule eligible(threshold) = enough(stock, threshold)
propose ship(eligible) priority 10 when both(eligible, true) = stock
commit shipping from (ship)
```

Functions have module-wide names and may call helpers declared later. Every
declaration is validated, including unused functions. Direct and mutual
recursion are rejected. There are no closures, first-class function values,
effects, or partial application.

A helper reads only its parameters and local match bindings, and may use
declared constructors or call other helpers. Global lets, external inputs,
and rule facts must be passed explicitly as arguments. At a rule or proposal
call site, the existing declared-dependency checks still apply to those
arguments. A helper cannot hide an undeclared rule read.

Arguments evaluate once, left to right, in the caller's environment before
the body runs. All arguments evaluate even if the body ignores them. In
particular, `first(1, 9223372036854775807 + 1)` fails if `first` ignores its
second argument; it cannot commit `1`. The body executes in a fresh lexical
environment. Parameter names and match bindings take precedence over outer
bindings; a nested call cannot capture its caller's locals.

Duplicate helper names, duplicate parameters, ambiguous helper/constructor
names, unresolved calls, and arity mismatches are lowering errors. Helpers
remain expressions: they produce no additional committed journal steps.

## Type and evidence contract

Annotations remain optional. This execution slice supports `Int`, `Bool`,
and `Str` annotations, optionally marked `@Derived`, on parameters and
returns. Argument contracts are checked on evaluated arguments before the
body, and the return contract is checked on the evaluated result. A mismatch
is a runtime fault yielding `Unknown` with no decision commitment.

This is runtime contract checking, not a claim of static inference or a
typing proof for every helper. Declaration-only `check` without required
external inputs validates syntax and supported contracts; it cannot discharge
value-dependent call checks. `check` with a complete snapshot exercises the
same evaluation as `run`.

Record and sum values may pass through unannotated parameters and returns.
Nominal record/sum annotations are refused in this slice: checking only a
value's outer name would fail to validate its field or payload types.
Anonymous record types, generic type applications, `Float`, nested grades,
`@Audited`, and `@Proven` contracts are also refused. An omitted annotation
does not certify a composite schema.

Runtime decisions remain `Derived`. Independent replay may issue separate
`Audited` receipts. Calling a helper never upgrades a value or a judgement.

## Resource contract

Helpers are represented by named calls and a function table, not expanded
by substituting bodies at every call site. This avoids exponential compiler
expansion and preserves eager argument evaluation.

The function-enabled path bounds declaration count, parameter count, source
expression size and depth, total evaluator nesting across calls, and work
per top-level evaluation. Value copying and construction also require bounds:
an acyclic helper that duplicates its input must not bypass the work budget
by cloning a growing tree. Limits are checked before the work or allocation
they govern. Exhaustion yields a typed refusal or `Unknown`, never a partial
decision, `Refuted`, or a certificate of divergence.

The limits apply to helper-enabled programs. Function-free executions keep
their prior profile behavior. Exact limits and their regression cases are
anchored in `finite_decision/plan.rs`, `l3_v2.rs`, and
`tests/finite_decision_functions.rs`.

## Canonical identity and replay

The existing program preimage remains unchanged when there are no functions.
For a nonempty function table, an additive frame follows configs and any
external-input frame, before lets:

```text
tag("brix.l3.finite-decision.functions@1")
uint(function_count)
for each function in source declaration order:
    uint(ordinal)
    ident(name)
    uint(parameter_count)
    for each parameter in argument order:
        ident(name)
        optional_contract
    optional_return_contract
    encoded_body
```

An optional contract uses enum `0` for absent and enum `1` for present.
A present contract encodes its type, followed by an optional grade. Scalar
type ordinals are `Int = 0`, `Bool = 1`, `Str = 2`. An absent grade uses enum
`0`; a present grade uses enum `1` followed by the grade ordinal (`Derived =
0`). Unsupported contract kinds cannot be emitted from accepted source.

Expression encoding appends ordinal `12` for a call, carrying the function
identifier, argument count, and argument expressions in order. Existing
expression ordinals `0` through `11` are unchanged. All encoding uses
`brix-canon`; this adds no new general canonical format or frozen-vector edit.

Bodies and contracts of unused declarations are included. Semantic edits to
helpers change the program pin even when a particular run returns the same
value. Comments and whitespace do not. External values continue to belong to
the snapshot/context identity, not the program identity.

Audit verification re-derives the helper table from caller-supplied source.
It uses the same evaluator, contracts and limits. A bundle cannot supply
trusted replacement helper definitions. The runtime's helper table is private
and constructed together with its pinned plan.

## CLI and library boundary

All six CLI commands use function-aware finite-decision lowering and
evaluation. `examples/shipping-functions.brix` and its JSON snapshot exercise
the source-to-decision-to-audit path.

The library also lowers helper calls in `show` expressions and evaluates them
through `FiniteDecisionRuntime::evaluate_shows`. The current CLI removes
`show` directives before lowering and displays its fixed decision report;
this slice does not change that existing presentation behavior. Consequently
CLI program pins exclude those removed directives, as before. Library plans
that retain shows bind them into their identity.

## Acceptance and implementation anchors

- `crates/brix-lower/src/finite_decision/plan.rs`: helper declarations,
  contracts, scope, cycle checks, limits, and canonical identity.
- `crates/brix-lower/src/l3_v2.rs`: call expressions, eager evaluation, fresh
  lexical environments, runtime contracts, and evaluator limits.
- `crates/brix-lower/src/finite_decision/runtime.rs`: private helper table
  shared by execution and independent replay.
- `crates/brix-lower/tests/finite_decision_functions.rs`: successful call
  sites, scoping, rejected contracts/cycles, resource limits and replay.
- `crates/brix-cli/tests/integration.rs`: six-command shipping workflow and
  rejection of changed helper source/program pins.

The merge bar includes the workspace tests, formatting and Clippy, canonical
cross-check, TCB dependency checks, and semantic-law traceability gates.
Existing alpha.2/alpha.3 workflows and their pins must remain reproducible.
