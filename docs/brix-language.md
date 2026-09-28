# Brix Language Overview

**Brix** (files: `.brix`) is a programming language built on the **SOC paradigm**.

> **Paradigm vs. Language:**
> Just as Object-Oriented Programming (OOP) is the paradigm and Java is a language realizing it, **SOC is the paradigm and Brix is the language.**

This document describes what the current `brix` CLI actually accepts and
reports. Every snippet below was run through `target/debug/brix` from a
source checkout (see the [README](../README.md) for build instructions) and
its output pasted verbatim; a handful of snippets are marked as fragments
because they intentionally illustrate a refusal rather than a working
program.

---

## The two lanes

`brix check <file.brix>` does one of two structurally different things,
decided by one rule in `crates/brix-cli/src/commands/check.rs` (around line
71): if the module contains any `propose`, `commit`, or `input` item, it is
checked as a **finite-decision** module; otherwise every top-level `let`
binding is type-checked on its own.

| | The `let` lane | The finite-decision lane |
|---|---|---|
| Triggered by | no `propose`/`commit`/`input` items | any `propose`, `commit`, or `input` item |
| Implementation | `brix_lower::check_module` | `brix_lower::finite_decision::lower_finite_decision_plan`, then the finite-decision runtime for `run`/`audit`/`verify`/`why`/`whynot` |
| Produces | a name + evidence-grade judgement per `let` binding, no value | a settled `@Derived` decision, with facts and candidate dispositions |
| `brix run` | not applicable — the lane only checks, it never executes | executes the plan to completion |
| Recursive `fn` | allowed, with mandatory type annotations, capped at `@Audited` | refused (`FunctionCycle`) |
| Generic/recursive configs (e.g. `List<T>`) | allowed | refused (`invalid schema '…': generic configs are unsupported`) |
| `&&`, `\|\|`, `!` | refused (`Unsupported("… not in L2-first fragment")`) | supported (ADR-0034) |
| `div_floor`/`div_ceil`/`div_half_even`/`mod_euclid` | refused (`Unresolved(…)`) | supported (ADR-0035) |
| `/` | `Int / Int → Float` (field-of-fractions division) | refused (`DivisionNotAllowed`; use the four named operations above) |
| unary `-` | supported | supported (ADR-0036) |
| `then`/`and` (witness composition) | supported | not part of the finite-decision expression grammar |
| external `input` declarations | not supported | the whole point (ADR-0031, ADR-0033) |

The finite-decision lane is not a superset or subset of the `let` lane — it
is a separate, deliberately bounded execution profile
(`brix.l3.finite-decision@1`,
[ADR-0030](../spec/adr/ADR-0030_Finite_Decision_Alpha.md)) built for a
different job: settling one bounded decision instead of type-checking
arbitrary bindings. But the two lanes do currently disagree on what looks
like the same expression grammar, and that is real, current behavior worth
stating plainly rather than discovering by trial and error — see "Known
inconsistencies between the lanes" near the end of this document.

---

## 1. The `let` lane: `brix check`

```bash
brix check <file.brix>
```

For a module with no `propose`/`commit`/`input` item, `brix check` parses the
source, lowers it onto native type-realization expressions, and type-checks
each top-level `let` binding. For each one it prints:

```text
  name : — @Grade
```

The `—` is not a placeholder that happens to be empty in these examples: the
CLI does not print the inferred type on this line today (the type is
computed — `brix_lower::CheckResult::ty` carries it — but `brix check`'s
human-readable output never renders it; see
`crates/brix-cli/src/commands/check.rs`). The type is checked, just not
echoed.

### Literals earn `@Proven`

```brix
let x = 42
let s = "hi"
let f = 3.14
```

Output of `brix check`:

```text
  x : — @Proven
  s : — @Proven
  f : — @Proven
```

### Composite expressions earn their weakest leaf grade

```brix
let c = 1 + 2

let p = Item { a: 1, b: 2 }
let v = p.a

fn double(x) = x + x
let r = double(2)
```

Output of `brix check`:

```text
  c : — @Audited
  p : — @Proven
  v : — @Proven
  r : — @Audited
```

`c` and `r` are capped at `@Audited` because arithmetic's primitive typing
generator is not yet kernel-discharged (see "Epistemic grades and honest
status" below) — not because anything about the expression is in doubt.

A record literal's fields are checked against a declared `config Name = {
… }` of the same name **if one exists**; `p` above has no declared `config
Item`, so it type-checks as an unvalidated ad hoc product. Declare one and
a missing or extra field is rejected by name (`MissingField`/`UnknownField`),
not silently accepted.

### What the fragment covers

The current `let`-lane fragment supports:

- **Literals:** `Int` (e.g. `42`), `Str` (e.g. `"hi"`), and `Float` (e.g.
  `3.14`).
- **`let` bindings**, including an optional `name: Type @Grade` contract that
  the checker must establish, not just document.
- **Functions and application:** `fn` definitions (including recursive and
  mutually recursive ones, see below), lambdas, and calls, inlined to
  application `App(Lam, arg)` when non-recursive.
- **Records and field access:** structural construction and projection, with
  missing/unknown-field validation against a same-named declared `config`.
- **Finite sums, recursive and parameterized configs, and matching:**
  `config Decision = Yes | No`, self-referential configs such as
  `config List<T> = Nil | Cons(T, List<T>)`, exhaustive `match` over
  wildcard, variable, and constructor patterns (there is no integer- or
  string-literal pattern — match a sum's constructors, or bind a variable and
  compare), and optional kernel-certified `proving exhaustive` coverage.
- **Arithmetic:** `+`, `-`, `*`, `/` over a numeric coercion lattice with
  witnessed `Int ↪ Float` promotion (`/` is field-of-fractions division:
  `Int / Int → Float`), plus unary `-`.
- **Comparison and Boolean values:** `<`, `<=`, `>`, `>=`, `==`, `!=`, and the
  `Bool` sum itself — but not `&&`, `||`, or `!` (see the two-lanes table
  above; those are finite-decision-lane only today).
- **Witness composition:** `and` (parallel/tensor — composes any two values
  unconditionally into a product) and `then` (sequential — requires a shared
  middle object, refused with `CompositionEndpointMismatch` otherwise) now
  lower onto the kernel's own composition operators. `witness w = e` is
  checked exactly like `let w = e`, kept as a distinct surface form for
  round-trip fidelity.
- **Modules:** `use pkg.name` brings another package's top-level
  declarations into scope (name conflicts, missing packages, and import
  cycles are all refused by name); see `packages/brix.soc` for a real
  package and `crates/brix-lower/tests/packaged_brix.rs` for the gate that
  keeps it checking.
- **Grade assertions:** `@Proven`, `@Audited`, and `@Derived`, checked
  through the grade lattice; strengthening beyond the earned grade is
  rejected as epistemic erasure.

Parallel composition, always `@Proven` for two already-proven operands:

```brix
let pair = 1 and "x"
```

```text
  pair : — @Proven
```

### Recursive functions

Commit `a9eb98f` added real recursion to the `let` lane: a definition's own
declared type is bound (`Expr::Fix`) while checking its body, so a recursive
call is a hypothesis lookup rather than an attempt to inline a copy of the
body forever. Both direct and mutual recursion work, and both parameter and
return types must be **declared** — they cannot be inferred from a body that
mentions the name(s) being defined.

```brix
config Nat = Z | S(Nat)
config Parity = IsEven | IsOdd

fn is_even(n: Nat): Parity = match n {
  Z => IsEven
  S(k) => is_odd(k)
}

fn is_odd(n: Nat): Parity = match n {
  Z => IsOdd
  S(k) => is_even(k)
}

let four = S(S(S(S(Z))))
let r = is_even(four)
```

```text
  four : — @Proven
  r : — @Audited
```

`r` is capped at `@Audited`, and that cap is structural, not a bug to fix:
the recursive-typing rule (`g_fix`) *assumes* the very obligation it is
checking, which is the standard sound rule for a **typing** judgement but not
one the kernel can independently discharge. A typing judgement never claims
the function terminates, either — so this type-checks, correctly:

```brix
fn loop(x: Int): Int = loop(x)

let r = loop(1)
```

```text
  r : — @Audited
```

Leaving out an annotation on a recursive definition is refused by name
rather than guessed:

<!-- brix-snippet: fragment -->
```brix
fn loop(x) = loop(x)

let r = loop(1)
```

```text
  r: not checked: RecursiveFunctionNeedsAnnotation { function: "loop", missing: "return type" }
```

Sequential composition with no shared middle object is refused the same
way — by name, not as a bare structural failure:

<!-- brix-snippet: fragment -->
```brix
let x = 1 then 2
```

```text
  x: not checked: TypeError(CompositionEndpointMismatch)
```

---

## 2. External inputs (finite-decision lane only)

A finite-decision module (one with a `commit`) can declare external
operational parameters at top level:

<!-- brix-snippet: fragment -->
```brix
input stock: Int
input threshold: Int
input urgent: Bool
```

- **Syntax:** `input <name>: <type>` at top level.
- **Scope:** an input is bound into scope for rule bodies, proposal guards
  (`when`), proposal values, and `show` expressions.
- **Types accepted directly:** `Int` (`i64`), `Bool`, and `Str`. `Float` and
  bare records/sums are rejected fail-closed with no lossy coercion — *except*
  through the `brix.input@2` structured-input extension below, which accepts
  whole records and sums.
- Input names participate in top-level collision checks against every other
  declared name.

### Artifact schema `brix.input@1` (scalars)

```json
{
  "schema": "brix.input@1",
  "values": {
    "stock": { "type": "int", "value": "12" },
    "urgent": { "type": "bool", "value": true }
  }
}
```

The envelope is strict: exactly `"schema"` and `"values"`; `int` values are
decimal strings (no IEEE 754 precision loss, no leading zeros); duplicate
keys anywhere (envelope, `values`, or inside a tagged object) fail closed
rather than last-write-wins. Limits: 1 MiB per file, 4 MiB aggregate across
at most 16 shard files, at most 256 inputs, 64-byte identifier length, 64 KiB
string length.

### Artifact schema `brix.input@2` (structured records and sums, ADR-0033)

A program can declare a whole domain value as an input and pass it to a
checked helper:

```brix
config Destination = Domestic | Export(Str)
config Order = { units: Int, destination: Destination }

input order: Order
fn enough(o: Order): Bool = o.units >= 10

propose accept() priority 10 when enough(order) = true
propose hold() priority 100 when true = false
commit decision from (accept, hold)
```

with a matching `brix.input@2` value (see
[`examples/order-policy.json`](../examples/order-policy.json) for the full
tagged-record/sum shape). Running it (`brix run examples/order-policy.brix
--input examples/order-policy.json`) prints:

```text
inputs:
  order: Order { destination: Export("EU"), units: 12 } @Derived
candidates:
  accept: selected (priority 10) — selected: minimal calendar key
  hold: admitted-not-selected (priority 100) — admitted but overshadowed by candidate 'accept'
decision: accept = Ship @Derived
status: selected
```

(program/context/input-snapshot hex ids omitted above — they are real,
content-addressed, and printed by the actual command; they are simply not
useful to reproduce byte-for-byte in prose.) `brix.input@1` scalar artifacts
remain accepted unchanged. Recursive and generic input schemas are **not**
part of this slice — the "generic configs are unsupported" refusal in the
two-lanes table above applies here too.

### CLI behavior and epistemic status

- `brix check <file.brix>` with no `--input` validates syntax, imports, and
  the input/plan contract only (`status: checked-input-contract`), binding a
  stable `ProgramId` over input *declarations*, never values.
- `brix check <file.brix> --input <path>...` performs full preflight:
  type/coverage validation and a dry-run of deliberation.
- Multiple `--input` flags supply disjoint shards; overlapping keys fail
  closed.
- External input values enter deliberation strictly at `@Derived` — an
  unverified external claim never ambiently upgrades to `@Audited` or
  `@Proven`. The runtime decision commits and **stays** `@Derived`;
  independent replay (`brix verify`) issues and checks a *separate*
  `@Audited` receipt, it does not upgrade the original judgement.

---

## 3. Functions, Booleans, and integer division in the finite-decision lane

Three source extensions widen what a finite-decision program's rule bodies,
proposal guards, and proposal values can express, without turning any of
them into settlement rules, witness generators, or a new evidence authority.

### Pure, nonrecursive helpers (ADR-0032)

```brix
fn enough(available: Int, needed: Int): Bool = available >= needed

input stock: Int
rule threshold() = 15
rule eligible(threshold) = enough(stock, threshold)

propose ship(eligible) priority 10 when eligible = stock
commit shipping from (ship)
```

Helpers take their data explicitly as arguments — inputs, global `let`s, and
rule facts are **not** captured from the surrounding module — and evaluate
arguments once, left to right, including unused ones. Recursive calls are
rejected here (`FunctionCycle`), unlike in the `let` lane.

### Short-circuiting `&&`, `||`, `!` (ADR-0034)

```brix
config Decision = Approved | Rejected

fn eligible(score: Int, flagged: Bool): Bool = score >= 700 && !flagged

rule credit_score() = 750
rule fraud_flag() = false
rule is_eligible(credit_score, fraud_flag) = eligible(credit_score, fraud_flag)

propose approve(is_eligible) priority 1 when is_eligible = Approved
propose reject() priority 10 when true = Rejected
commit c from (approve, reject)
```

`brix run` selects `approve = Approved @Derived`. `&&`/`||` genuinely
short-circuit (`false && e` never evaluates `e`, so a fault in `e` does not
stop the decision), and replay reproduces that short-circuiting exactly,
which is what makes it safe under `audit`/`verify`. `&&` binds tighter than
`||`; both bind looser than comparison; `!` binds tighter than every binary
operator. Operands must be `Bool` — there is no truthiness or `Int`
coercion, and this is caught statically even on a skipped branch (`false &&
1` is a `check`-time error).

### Exact integer division (ADR-0035) and unary minus (ADR-0036)

`/` stays refused in this lane (`DivisionNotAllowed`) because integer
division has no single correct rounding — `-7 / 2` is `-3` in Rust and `-4`
in Python, both defensible. Four named operations replace it:

| Operation | Rounds toward |
|---|---|
| `div_floor(a, b)` | negative infinity |
| `div_ceil(a, b)` | positive infinity |
| `div_half_even(a, b)` | nearest, ties to even |
| `mod_euclid(a, b)` | not a rounding — the remainder `r` with `0 <= r < \|b\|` |

Both operands must be `Int`; `b == 0` raises `DivisionByZero` for all four;
`Int::MIN` divided by `-1` raises `DivisionOverflow` for the three division
operations (`mod_euclid` never overflows). [`examples/allocation.brix`](../examples/allocation.brix)
splits a price across a car count and checks the remainder:

```text
$ brix run examples/allocation.brix --input examples/allocation.json
inputs:
  batch: Allocation { car_count: 5, price_cents: 12000 } @Derived
facts:
  share: 2400 @Derived
  leftover: 0 @Derived
  evenly_split: true @Derived
  meets_minimum: true @Derived
candidates:
  balanced: selected (priority 10) — selected: minimal calendar key
  insufficient: rejected-guard-false (priority 20) — guard condition evaluated to false
  uneven: admitted-not-selected (priority 100) — admitted but overshadowed by candidate 'balanced'
decision: balanced = Balanced @Derived
```

Unary minus (`let below = -7`, `-a * b` parses as `(-a) * b`) is a grammar
addition only — no new evaluator operation, no new canonical ordinal — added
alongside ADR-0035 so a negative dividend could be written at all.

---

## 4. Bounded lists and finite relations (ADR-0037, ADR-0040)

A finite-decision program can declare a bounded, homogeneous list as a
top-level input, and combine two of them the way a real settlement policy
usually needs to: a join, not just a summary.

<!-- brix-snippet: fragment -->
```brix
config Order = { id: Int, sku: Int, units: Int }
config Stock = { sku: Int, on_hand: Int }

input orders: List<Order> max 32
input stock: List<Stock> max 32

rule coverable_ids() =
  for o in orders, s in stock where s.sku == o.sku && s.on_hand >= o.units yield o.id
rule short_orders(coverable_ids) = filter(orders, o => !(o.id in coverable_ids))
rule short_count(short_orders) = len(short_orders)
```

[`examples/fulfillment.brix`](../examples/fulfillment.brix) is a complete,
verified program built from this: it joins a bounded list of orders against a
bounded list of stock rows and decides whether to ship everything, ship what
it can, or hold, with [`examples/fulfillment.test.json`](../examples/fulfillment.test.json)
covering all three outcomes plus the empty-list case.

- **`input <name>: List<T> max N`** declares a bounded list input, `T` a
  scalar, record, or sum type reachable the same way ADR-0033's `brix.input@2`
  reaches one, and `N` a compile-time bound (`1..=256`). The matching artifact
  schema is **`brix.input@3`**, a strict superset of `@2` that additionally
  admits a `{"type": "list", "items": [...]}` value at the top level only — a
  list nested inside a record field or a sum's argument is still refused,
  fail-closed, on every schema version. `@1`/`@2` continue to refuse a list
  value outright, so an existing input artifact is unaffected.
- **Folds** — `sum`, `count`, `all`, `any`, `min`, `max` — reduce a list to a
  scalar: `sum(xs, x => e)`, `count(xs, x => cond)`, and so on. `min`/`max` of
  an empty list is a typed fault (`EmptyAggregate`), never a default value —
  guard with a `count`/`len` check first, exactly as `max_order_size` does
  above via `match order_count > 0 { true => ..., false => 0 }`.
- **`filter(xs, x => cond)`** and **`map(xs, x => e)`** produce a new list,
  each element visited left to right.
- **A comprehension**, `for x in xs, y in ys where cond yield e`, nests one
  generator per list left to right — a later generator (and `where`) may
  reference an earlier one's binder, which is what makes it a join rather
  than two independent loops. `where` is optional.
- **`[e1, e2, ...]`** is a list literal; **`e in xs`** is structural-equality
  membership, at comparison precedence and equally non-associative (`a in xs
  in ys` is refused by name, like `a < b < c`); **`len(xs)`** and
  **`distinct(xs)`** (first occurrence kept) round out the surface.
- A lambda (`x => expr`) is recognized structurally as a call argument, not
  a keyword — `filter`/`map`/`sum`/`count`/`all`/`any`/`min`/`max`/`len`/
  `distinct` are reserved *operation names* (a module cannot declare a helper
  or constructor with one of them), the same discipline ADR-0035 uses for
  `div_floor`/`mod_euclid`/etc. `for`/`in`/`where`/`yield` **are** new
  keywords, since a comprehension's grammar cannot be spelled as an ordinary
  call. A lambda is never a first-class value — it cannot be bound, returned,
  or passed anywhere but one of those ten call sites.
- Every derived list (a fold/filter/map/comprehension/literal's result) is
  capped at 4,096 elements regardless of any input's own `max`, and every
  element visited is charged to the same bounded evaluator work budget every
  other expression in this lane already runs under (ADR-0032) — a large join
  fails closed with a typed fault rather than exhausting memory.
- `brix why`/`whynot`'s derivation trace, `brix kb`'s persistent storage, and
  `brix test`'s fact assertions all understand list values, rendered the same
  way `brix run` prints them (`[1, 2, 3]`); a fold/filter/map/comprehension's
  trace shows a bounded sample of its elements rather than one node per
  element, since the source list can be as large as 4,096 entries.

See [ADR-0037](../spec/adr/ADR-0037_Bounded_Lists_And_Folds.md) (lists and the
first four folds) and [ADR-0040](../spec/adr/ADR-0040_Finite_Relations.md)
(`filter`/`map`/comprehensions/`in`/`len`/`distinct`/`min`/`max`) for the full
semantics, canonical encoding, and acceptance checklist.

---

## 5. Epistemic grades and honest status

Brix does not collapse every outcome into `true`/`false`:

- **`@Proven`** — certified end-to-end by the kernel down to discharged,
  tight generator leaves.
- **`@Audited`** — certified compositionally given primitive generator leaves
  whose semantic validity remains open, or independently verified via an
  offline audit-bundle replay (issuing a *separate* `@Audited` receipt).
- **`@Derived`** — an unverified candidate fact or a runtime settlement
  commitment. A finite-decision runtime decision commits at, and stays,
  `@Derived`; only a successful independent replay (`brix verify`) issues a
  distinct `@Audited` receipt over it.

The kernel certifies the *composition* theorem — given the primitive typing
leaves as generators, the derivation establishes `e : T`. The honest grade is
`@Proven` only when every leaf is independently tight. As of this
writing that includes literals, the simply-typed λ-calculus core, records
and field access, non-nullary **and** nullary constructors, zero-field
records, and explicit-constructor matches. Still capped at `@Audited`:
arithmetic and numeric coercion, wildcard/variable catch-all matches, and any
recursive `fn`'s own typing judgement (`g_fix`, discussed above) — their
kernel rules are not yet available or fully discharged.

---

## 6. Type normalization and coercion lattices

Type normalization runs on one declared, witnessed-coercion mechanism,
`CoercionLattice`, with two live instances:

1. **`NUMERIC`** — `Nat ↪ Int ↪ Rat ↪ Real ↪ Complex` (safe widening) plus a
   lossy `Int ↪ Float` branch (`join(Float, Rat) = None`: mixing float and
   exact rational/real types is a type error). Only `Int` and `Float` are
   reachable from surface literal syntax today; the wider lattice exists in
   `soc-regimes` for future numeric sorts and coercion tests.
2. **`GRADE`** — `Proven ↪ Audited ↪ Derived` (safe *weakening* of
   certainty). The forbidden direction, `Derived → Proven`, has no upward
   path and is rejected as epistemic erasure.

```brix
// Division yields the field of fractions (Int / Int -> Float)
let ratio = 7 / 2

// Mixed integer and float addition (Int safely coerces to Float)
let mixed = 1 + 2.5
```

```text
  ratio : — @Audited
  mixed : — @Audited
```

Remember that this `/` is the `let`-lane one: the finite-decision lane
refuses `/` outright (see §3 above).

---

## Known inconsistencies between the lanes

These are current, real behaviors — not typos in this document — surfaced by
running the CLI, kept here so nobody has to rediscover them by trial and
error. `spec/Next_Steps.md` tracks reconciling them.

- **`&&`, `||`, `!` and the four division built-ins only work in the
  finite-decision lane.** The same syntax is refused in the `let` lane:

  <!-- brix-snippet: fragment -->
  ```brix
  // Both rejected in the `let` lane — see §3 above for where they work.
  let a = true && false
  let c = div_floor(7, 2)
  ```

  ```text
    a: not checked: Unsupported("'AndAnd' not in L2-first fragment")
    c: not checked: Unresolved("div_floor")
  ```

- **`/` means different things in the two lanes**: `Int / Int → Float`
  division in the `let` lane, `DivisionNotAllowed` in the finite-decision
  lane (which names `div_floor`/`div_ceil`/`div_half_even`/`mod_euclid` as
  the replacement in its own diagnostic).

- **Recursion and generic configs work in the `let` lane and are refused in
  the finite-decision lane.** A generic config used anywhere in a
  finite-decision module's schemas — even just as a helper's parameter
  type — is refused outright:

  <!-- brix-snippet: fragment -->
  ```brix
  config List<T> = Nil | Cons(T, List<T>)
  config Decision = Yes | No

  fn head_or(xs: List<Int>, fallback: Int): Int = match xs {
    Nil => fallback
    Cons(h, _) => h
  }

  rule x() = head_or(Cons(1, Nil), 0)

  propose yes(x) priority 1 when x == 1 = Yes
  propose no() priority 100 when true = No
  commit d from (yes, no)
  ```

  ```text
  brix check: rejected: lowering error: invalid schema 'List': generic configs are unsupported
  ```

  A directly self-recursive helper is refused the same way, by a dedicated
  error naming the cycle:

  ```text
  brix check: rejected: lowering error: function cycle detected involving 'loop': loop -> loop
  ```

- **`brix check`'s human output never prints the inferred type**, in either
  lane's binding-level report — only the name and the grade (`name : —
  @Grade`). The type is computed and available on `CheckResult::ty`; it is
  simply not rendered today.

---

## Roadmap and execution profiles

- **Finite-Decision Alpha** (accepted,
  [ADR-0030](../spec/adr/ADR-0030_Finite_Decision_Alpha.md)): candidate
  deliberation over complete frontiers, structured rejection reasons, and
  deterministic calendar selection. Decisions commit `@Derived`; independent
  replay issues a separate `@Audited` receipt via `brix verify`.
- **External Inputs** (accepted,
  [ADR-0031](../spec/adr/ADR-0031_External_Input_Alpha.md)): `input`
  declarations, `brix.input@1` strict scalar schema, bounded disjoint shards,
  deterministic context identity.
- **Reusable functions** (landed and tested; ADR status "Proposed
  implementation",
  [ADR-0032](../spec/adr/ADR-0032_Finite_Decision_Functions.md)): pure,
  nonrecursive helper `fn`s in finite-decision programs.
- **Structured inputs** (landed and tested; ADR status "Proposed
  implementation",
  [ADR-0033](../spec/adr/ADR-0033_Structured_Input_Contracts.md)):
  `brix.input@2` whole-record/sum inputs and composite function contracts.
- **Boolean operators** (landed and tested; ADR status "Proposed
  implementation",
  [ADR-0034](../spec/adr/ADR-0034_Boolean_Operators.md)): `&&`, `||`, `!`,
  precedence-climbing expressions.
- **Integer division** (landed and tested; ADR status "Proposed
  implementation",
  [ADR-0035](../spec/adr/ADR-0035_Integer_Division.md)): `div_floor`,
  `div_ceil`, `div_half_even`, `mod_euclid`.
- **Unary minus** (landed and tested; ADR status "Proposed implementation",
  [ADR-0036](../spec/adr/ADR-0036_Unary_Minus.md)): negative literals and
  prefix `-`.
- **Bounded lists and folds** (landed and tested; ADR status "Implemented",
  [ADR-0037](../spec/adr/ADR-0037_Bounded_Lists_And_Folds.md)):
  `List<T> max N` inputs, `brix.input@3`, and deterministic `sum`/`count`/
  `all`/`any` folds.
- **Finite relations** (landed and tested; ADR status "Implemented",
  [ADR-0040](../spec/adr/ADR-0040_Finite_Relations.md)): `filter`, `map`,
  comprehensions (joins), list literals, `in`, `len`, `distinct`, and `min`/
  `max` folds over the same bounded lists — see §4 above.
- **CLI driver:** `check`, `run`, `audit`, `verify`, `why`, `whynot`, and
  `test` (regression suites), all accepting repeatable `--input`, plus the
  `kb` family (`init`/`assert`/`retract`/`program`/`log`/`show`/`diff`/
  `audit`/`verify`) for a persistent, revisable knowledge base
  ([ADR-0041](../spec/adr/ADR-0041_Persistent_Knowledge_Base.md)).
  `verify --profile l3-v1` rejects `--input`.
- **L3 v2 derivation** ([ADR-0027](../spec/adr/ADR-0027_L3_V2_Derivation.md)):
  Stages A–C landed in `brix-lower`, defining the derivation evaluator and
  eligibility rules on committed dependencies — a separate executable
  profile from finite-decision, not exposed by `brix run`.

For the fuller picture of what a beta needs beyond this — one shared
expression language across lanes, relations and per-entity decisions, a
persistent revisable knowledge base — see
[`docs/planning/beta-roadmap.md`](./planning/beta-roadmap.md).
