# ADR-0042 — One evaluator for one expression language

Status: **Proposed implementation**, 2026-09-28. Implements the "One
language" milestone `docs/planning/beta-roadmap.md` scoped for the beta.
The design and the acceptance checklist below describe the implementation
submitted for review, not separate semantic ratification.

Extends the `let` lane (`brix_lower::check_module`, native type realization)
and the finite-decision profile (`brix.l3.finite-decision@1`, ADR-0030 —
ADR-0040). It does not widen the finite-decision profile's rule-agenda
semantics, does not touch the SOC authority boundary, and does not change
type-realization checking or grades at all — every grade a `let` binding
earns still comes from exactly the same tree-elaboration path it always did.

## Motivation

`.brix` behaved like two half-languages. A module without `propose`/
`commit`/`input` is checked as a `let` module: `check_module` type-checks
every top-level binding through native type realization and prints a name
and a grade — never a value, even though the value was always computable in
principle. A module with any `propose`/`commit`/`input` is checked as a
finite-decision program: `lower_finite_decision_plan` lowers it onto
`L3ExprV2` and `brix_lower::l3_v2::eval` actually runs it — but that
evaluator refused a helper that called itself or called through a cycle of
other helpers (`FunctionCycle`, ADR-0032), and refused a generic config
outright (`"generic configs are unsupported"`), even as an ordinary internal
value with nothing depending on its type parameter.

Neither restriction was a semantic disagreement about what the *language*
means — `docs/brix-language.md`'s "Known inconsistencies between the lanes"
section (as it read before this ADR) is explicit that these were two
implementations of what looks like the same expression grammar drifting
apart for historical reasons, not because a program recursing, or a generic
config, means something different in the two places. That drift is the
defect this ADR corrects, by making both lanes lower the same expressions
onto the same IR and run them through the same evaluator. What a *profile*
still restricts — which items may appear, whether `Float` or witness
composition exist, whether a schema can name a generic type — stays a
profile choice, stated as such, not an accident of which lowering pass a
module happened to go through.

## Design: one evaluator, profiles restrict admission, never meaning

`brix_lower::l3_v2::eval` — the same interpreter `L3ExprV2` values, over the
same `L3ValueV2` — is now the one place either lane computes a value.
Nothing about `eval` itself, `L3ExprV2`, or `L3ValueV2` changed shape for
this ADR (no new expression ordinal was needed): what changed is which
lowering passes feed it, and two restrictions the finite-decision lowering
pass used to enforce unconditionally.

**The `let` lane now evaluates.** A new pass,
`brix_lower::let_eval::evaluate_let_module`, runs after (and independently
of) `check_module` over the same module. It builds the same arity-only
config tables (`variants_of`/`nullary`) the finite-decision lane's own
lowering builds, lowers every top-level `fn` through
`crate::l3_v2::lower_expr_v2` in the same pure-and-closed helper discipline
ADR-0032 established (a helper sees only its own parameters, never an outer
`let`), and lowers each `let`/`witness` binding's value the same way, in
declaration order, evaluating it against the functions and previously
evaluated `let`s already in scope. The result is
`brix_lower::LetEvalOutcome`: `Value(L3ValueV2)` when the binding's
expression — and everything it transitively calls — lies in the exact
fragment `l3_v2::eval` covers, or `NotEvaluated(reason)` naming exactly what
put it outside that fragment. `brix check` prints `name : Type @Grade =
value` or `name : Type @Grade (not evaluated: reason)` accordingly; the type
and grade are, and remain, exactly what `check_module` computed — evaluation
is strictly additive and never consulted by, or capable of influencing,
type-checking. `brix check`'s JSON output gains a parallel, additive
`bindings` array (one `BindingJson` per checked binding, with `value` xor
`not_evaluated` populated) under the unchanged `brix.cli.result@1` schema.

This also fixes a standing bug: `brix check`'s human-readable output printed
`name : — @Grade`, a literal em-dash where the inferred type belonged, even
though `CheckResult::ty` always carried it — the CLI never rendered it. The
type is now rendered (`brix_lower::render_ty`, made `pub` for this) whether
or not the binding evaluates.

**A binding never gets a guessed value.** Every construct outside the exact
executable fragment is reported by name, never silently rounded to a value
or to nothing: a `Float` literal or any arithmetic that produces one (`/`
included — see below), witness composition (`then`/`and`), `prove`/`why`/
`audit`, a wildcard or variable catch-all match arm (which *type-checks*, at
`@Audited`, but has no counterpart in `l3_v2`'s constructor-only pattern
language), and a nested constructor pattern. A binding that depends on one
of these — directly, or transitively through a helper call — is reported
`NotEvaluated` too, with the dependency named, rather than treated as
`Unbound` with no further explanation.

**Recursion is admitted in the finite-decision lane.** `lower_finite_decision_plan`
no longer builds a call graph and rejects a cycle in it
(`FunctionCycle`, `detect_cycles`/`dfs_cycle`, all removed — ADR-0032's
"Direct and mutual recursion are rejected" sentence is superseded, noted
in place there without rewriting its history). A helper may call itself
directly, or call through a cycle of other helpers. Termination is not
assumed and is not the lowering pass's job to decide: it is the evaluator's
call-depth and work-budget bounds (below) that decide it, at run time, the
same way they already bounded every other potentially-expensive expression
shape.

One static pass had to change to admit this safely.
`finite_decision::boolean_types::check` — the resource-bounded static
analysis that infers a call's Boolean operand shapes by re-walking the
callee's body inline — previously relied on cycle detection already having
rejected recursion, so it had no cycle guard of its own; naively re-entering
a genuinely recursive helper's body would have re-derived the same shape
forever, bounded only by exhausting its own depth/work budget
(`BooleanTypeAnalysisLimit`) even for a helper whose *evaluator* recursion
would terminate trivially. It now tracks which helper names are being
expanded on the current path and resolves a call back into one of them from
its declared return contract (or `Unknown`) instead of re-entering it — a
static shape approximation, not a claim about whether the program
terminates, which remains the evaluator's question alone.

**Generic configs are admitted as values in both lanes.** A `config
Stack<T> = Nil | Cons(T, Stack<T>)` declaration was always accepted by the
parser (`ast::ConfigDecl::params`); type realization already resolved a
declaration's own type parameters to `Ty::Param` and instantiated them at
each use (that machinery predates this ADR). What both the `let` lane's
config-table construction and the finite-decision lane's plan-lowering
config-table construction already did — build `variants_of`/`nullary` from
variant *arity* alone, never from a variant's declared parameter *types* —
turns out to already be exactly type-parameter erasure: a value is a
nominal constructor or record tagged with its declaring config's name,
independent of what type argument checked it. Nothing about evaluating a
generic config's value needed to change; what needed to change was a
single a-priori refusal in the finite-decision lane's *schema* collection
(`collect_schema`), which rejected any config reachable from an `input`
declaration's or a helper contract's type that had type parameters at all,
before ever getting to evaluate anything. That refusal stays — a generic
type genuinely cannot be validated as an input or contract schema, because
there is no payload shape left to check once the type parameter is erased —
but its diagnostic (`InvalidSchema`/`UnsupportedContractType`) now says why,
rather than asserting unsupported-ness with no reason. A generic config used
only as an internal value (never named in an `input` or a helper's
parameter/return annotation) now lowers and evaluates in the finite-decision
lane exactly as it always did in the `let` lane.

**Name clash with `List<T>`.** The finite-decision lane separately gained a
*built-in* bounded-list type also spelled `List<T>` (ADR-0037). A
type-position `List<...>` always resolves to that built-in there,
regardless of whether a module also declares `config List<T> = …`; the two
features were developed in parallel and this ADR does not attempt to
reconcile the name collision (a user-declared `config List<T>` is a
"generic config" as far as this ADR's evaluator support goes, but writing
one in the finite-decision lane is confusing rather than meaningfully
useful, since every type-position `List<...>` still means the built-in).
This ADR's own tests and examples use `Stack<T>`/`Tree<T>` for user-declared
generic configs to stay clear of it.

## Recursion bounds, and why they had to move

`l3_v2::EvalBudget` already bounded call-based recursion two ways —
`MAX_CALL_DEPTH` (simultaneously outstanding `Call` invocations) and
`MAX_EVAL_RECURSION_DEPTH` (every `eval_internal` nesting, since a single
logical call level costs several: the scrutinee, the chosen arm, the call's
own arguments, then the callee's body) — plus a total step budget
(`MAX_CALL_STEPS`). Those bounds existed for ADR-0032's pure-but-nonrecursive
helpers, where the deepest nesting came from source-level call chains of
distinct helpers, capped by `MAX_FUNCTION_COUNT` (256) long before either
bound could matter. Recursion changes that: `length` over an ordinary list a
few hundred elements long needs that many outstanding calls, from a single
helper. The bounds were raised (`MAX_CALL_DEPTH` 64 → 1,000;
`MAX_EVAL_RECURSION_DEPTH` 64 → 20,000; `MAX_CALL_STEPS` 10,000 → 200,000)
to make that a program `brix run`/`brix check` actually accepts, not a
theoretical allowance.

A bound only protects the process if the native stack it is meant to stand
in for cannot be exhausted first — and running `eval_internal`'s recursive
descent on whatever thread happened to call in (the CLI's unmodified process
main thread; a test harness thread; a future embedder) makes "never a stack
overflow" depend on a stack size nothing here controls. Only a helper call
can recurse, so an evaluation with a nonempty helper table runs on an
evaluation thread with a fixed, generous stack (`EVAL_THREAD_STACK_BYTES`,
256 MiB, reserved rather than committed, and sized against a debug build's
larger frames since the merge bar's `cargo test` runs debug). Without
helpers, native depth is bounded by expression nesting and the evaluation
runs on the caller's thread. A caller that evaluates many expressions enters
the evaluation thread once through `l3_v2::with_eval_stack`, and every
evaluation inside it runs inline; the CLI runs each command, including a
whole `brix serve` session, that way. A thread that cannot be created, or
that panics — `eval_internal` is total, so a panic there is a defect, never
an expected outcome — reports a resource fault rather than propagating to
the caller. The budget, not the caller's thread, decides whether a program is
admitted, and a failure is always
`Unknown(EvalFault::CallDepthExceeded | ResourceExhausted)`, never a crash.
`crates/brix-lower/tests/finite_decision_recursion.rs` exercises a
terminating direct- and mutual-recursion happy path, recursion just under
and comfortably past `MAX_CALL_DEPTH`, budget exhaustion from a
recursively-growing value (independent of the call-depth bound), run
determinism, and audit-bundle replay of a recursive helper.

### Evaluation budgets

This section supersedes the budget figures quoted in ADR-0037, ADR-0040, and
ADR-0043.

- **Work.** Every evaluation step, including reading a value or binding a
  list element, costs one step. One evaluation may take `MAX_CALL_STEPS`
  (2,000,000) steps, enough for a full join of two maximum-size list inputs
  (256 × 256 pairs) but not a three-way one. Every evaluation in one
  deliberation run (lets, rules, guards, values, and every `decide`
  instance) also draws on a shared counter bounded by `MAX_RUN_STEPS`
  (50,000,000), so a `decide` block cannot multiply the per-evaluation bound
  by its instance count. The worst case is about two seconds in a release
  build.
- **Memory.** `MAX_EVAL_VALUE_NODES` and `MAX_EVAL_VALUE_BYTES` bound values
  an evaluation *builds*: constructors, records, list literals, and the
  results of `map`, `filter`, and comprehensions. Reading a value the
  evaluation already holds is not an allocation: list values are shared
  (`L3ValueV2::List` holds an `Arc<[L3ValueV2]>`), and the evaluation
  environment shares inputs, lets, and facts, so a per-element environment
  copies only its local bindings. Charging reads as allocation made any
  input over about 1 MB unusable in an expression, and refused a join of two
  lists on memory it never kept.

Exceeding any bound is `Unknown` with `ResourceExhausted`, as before.
`crates/brix-lower/tests/eval_shared_values.rs` covers repeated reads of a
large input, a full two-list join, a refused three-way join, and the
run-wide bound across `decide` instances.

Two match arms in the recursive evaluation core — `eval_internal_body`'s
`Call`/`Match` dispatch and `boolean_types::Checker::expr`'s equivalent — sit
on the hot recursive path: every helper each delegates to
(`eval_fold`/`eval_filter`/`eval_map`/`eval_comprehension`/… on the evaluator
side, `list_expr` on the static-checker side) is marked `#[inline(never)]`,
so those helpers' own locals do not widen the recursive function's own stack
frame — a wider frame lowers the depth a fixed-size stack can reach before
the explicit bound is what stops it, which is exactly the property the
previous paragraph's stack-safety argument depends on.

## `/` has one meaning, admitted differently

`/` means exact-to-Float division, `Int / Int → Float`, and always did — in
the type-realization (`let`) lane, where it type-checks (at `@Audited`, the
same cap ordinary arithmetic earns) and, since `Float` is outside the
evaluator's exact fragment, is reported `not evaluated` rather than given a
value. The finite-decision lane refuses the operator outright
(`DivisionNotAllowed`) — not because its meaning is ambiguous, but because
`Float` values are not admitted in a finite-decision program at all, so an
operator whose only meaning produces one cannot be either. Both
diagnostics — the finite-decision refusal and the `let` lane's "not
evaluated" reason — now say exactly this, in the same words
(`l3_v2::division_not_admitted_reason`), and both name the same four exact
integer replacements ADR-0035 already defined: `div_floor`, `div_ceil`,
`div_half_even`, `mod_euclid`.

## Identity and replay

**No new canonical ordinal.** `L3ExprV2`'s encoding (`encode_expr_v2`) is
unchanged; recursion needed none (a recursive call is an ordinary `Call`
node naming its own function, already representable) and neither did generic
configs (a config's canonical encoding was always its variants'/fields'
*arity*, never their declared parameter types).

**Generic config declarations join finite-decision program identity,
additively.** `FiniteDecisionPlan` gains `generic_configs: Vec<(String,
Vec<String>)>` (declaration order), and
`finite_decision_program_preimage` writes a new tagged section,
`brix.l3.finite-decision.generic-configs@1`, **only when it is nonempty**.
A program that declares no generic config carries no such section and its
preimage — and therefore its `FiniteDecisionProgramId` — is byte-identical
to what it always was. This section exists so two declarations that
coincide on arity (a generic `Stack<T>` and a same-shaped, hypothetical
non-generic `Stack`) do not collide in identity purely because neither a
variant's declared payload types nor a config's parameter list were ever
otherwise part of the preimage.

**No other identity change.** Recursion needed none of its own: whether a
function's body happens to mention its own name, or a cycle of other
functions' names, was already fully determined by `L3ExprV2::Call { func,
args }` nodes the preimage already encoded — a recursive program's identity
was already exactly what its (previously refused) plan would have encoded.
Removing the a-priori refusal changes which programs are *accepted*, never
what an accepted one's identity is. `crates/brix-lower/tests/finite_decision_functions.rs`'s
existing frozen-pin regressions (`test_zero_function_preimage_byte_identical_compatibility`,
`test_function_free_frozen_known_shipping_identity`,
`test_program_id_deterministic_and_comment_invariant`) pass unchanged, and
this ADR's own work recorded `brix run`'s program/context ids for every
`examples/*.brix` before making any change and confirmed them unchanged
after.

**Replay.** `brix audit`/`brix verify` re-lower the source and re-run the
same runtime `brix run` does, so a recursive helper's replay runs the same
evaluator over the same encoding under the same bounds — deterministic for
the reason ADR-0034's short-circuiting replay is: the same interpreter, the
same budget, the same input.
`crates/brix-lower/tests/finite_decision_recursion.rs::test_recursive_helper_survives_audit_bundle_replay`
exercises this directly. `why`/`whynot` (`soc_regimes::finite_frontier`,
rendered by `crates/brix-cli/src/commands/why.rs`) explain a candidate's
guard/value by re-evaluating them, not by textually expanding a helper's
body — there is no separate "expand one level" explanation mechanism for
this ADR to bound, and a recursive helper's *evaluation* is already bounded
by the call-depth/work budget above, so an explanation cannot be made
unbounded by it either.

## Acceptance

- `brix check crates/brix-lower/tests/fixtures/id.brix` prints `r : Int
  @Proven = 42`, not `r : — @Proven`.
- Every `let`-lane construct in the exact executable fragment — literals,
  records, sums, `match`, functions, recursion (including over a
  user-declared generic config), generic configs — evaluates to a value;
  every construct outside it (`Float`, `/`, `then`/`and`, `prove`/`why`/
  `audit`, a wildcard/catch-all match arm) is reported `not evaluated` with
  a specific reason, never a value, and never silently as if the binding did
  not exist.
- Evaluating a binding never changes its grade: the grade is asserted equal
  to what `check_module` alone would have produced, for every case above.
- A finite-decision helper may call itself or call through a cycle of other
  helpers; a terminating call succeeds; a non-terminating one, and one that
  merely exceeds the call-depth bound, both fault closed to `Unknown` —
  never a lowering error, never a stack overflow — under `cargo test`'s own
  (debug-build, unmodified) thread stack.
- A generic config evaluates as an internal value in both lanes, including
  under recursion (a `length` helper over a user `Stack<T>`); it is still
  refused, with a diagnostic naming type-parameter erasure as the reason, in
  a finite-decision `input` declaration or helper contract.
- `/`'s finite-decision refusal and the `let` lane's "not evaluated" reason
  for it state the same meaning and name the same replacements.
- Program ids for every non-generic, non-recursive `examples/*.brix` are
  byte-identical before and after this change; a generic config's
  declaration is bound into identity only when the program declares one.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace`, `scripts/canon_crosscheck.py`,
  `scripts/check_tcb_dependencies.py --check`,
  `scripts/test_tcb_dependency_gate.sh`, `scripts/check_soc_law_map.py`, and
  `scripts/test_law_map_provisional_gate.sh` all pass.
