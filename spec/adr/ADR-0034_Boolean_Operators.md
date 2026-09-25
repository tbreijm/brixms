# ADR-0034 — Short-circuiting Boolean operators and precedence-climbing expressions

Status: **Proposed implementation**, 2026-09-25. Follows the authorized
usability milestone after ADR-0033. The encoding and evaluation rules below
describe the implementation submitted for review, not separate semantic
ratification.

Extends the finite-decision profile from ADR-0030 and the helper-evaluation
boundary from ADR-0032. It does not widen either rule-agenda profile, does not
touch the external-input transport of ADR-0031/ADR-0033, and does not change
the SOC authority boundary.

## Motivation

Every nontrivial policy is a conjunction of conditions. Without `&&`, `||`, and
`!` a module states one by nesting `match` arms or by exporting a separate rule
per clause, which relocates the policy out of the expression that decides and
into the shape of the module. Both readings are worse than the source sentence
the author meant to write.

The narrower point: the existing expression grammar was a fixed four-level
descent (`then`/`and` → comparison → additive → multiplicative). Each new
precedence level cost another hand-written level and another `match_binN`
helper, and the levels could only be read by tracing which function called
which. Two Boolean levels have to sit *between* `then`/`and` and comparison,
which is exactly the middle of that cascade.

## User-visible behavior

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

The operators are available wherever an expression is: global lets, rule
bodies, helper bodies, and proposal guards and values.

This milestone does not add bitwise operators, a ternary conditional, exclusive
or, or user-defined operators. `&` and `|` alone remain unknown characters —
`&` is refused by name with a suggestion rather than being read as anything.

## Precedence and associativity

The four-level descent is replaced by one precedence-climbing loop. Levels,
loosest to tightest:

| Level | Operators | Associativity |
|---|---|---|
| 1 | `then`, `and` | left |
| 2 | `\|\|` | left |
| 3 | `&&` | left |
| 4 | `<` `<=` `>` `>=` `==` `!=` | **non-associative** |
| 5 | `+` `-` | left |
| 6 | `*` `/` | left |
| 7 | `!` (prefix), `prove`, `audit` | prefix |

Three properties this table has to preserve, stated because each is a decision
rather than a convention inherited for free:

**`&&` binds tighter than `||`.** So `a || b && c` is `a || (b && c)`. This is
the near-universal reading and disagreeing with it would make correct-looking
policies wrong.

**Comparison stays non-associative.** `a < b < c` remains refused by name,
under the reasoning ADR-0010 ⟨D-OPARROW⟩ records: chaining is composition of
order arrows, not conjunction, and this fragment has no value-level order
regime to compose in. Introducing `&&` does *not* retroactively make `a < b < c`
mean `a < b && b < c`, and refusing it now keeps that meaning available later
without breaking a program that exists today.

**`!` binds tighter than every binary operator.** `!a == b` is `(!a) == b`.
`!` applies to the smallest following prefix expression, so `!!x` is
well-formed, and a Boolean negation of a comparison is written `!(a < b)`.

Witness composition (`then`/`and`) stays loosest and is unchanged. `and` and
`&&` are deliberately distinct: `and` is tensor composition of witnesses, `&&`
is Boolean conjunction of values. They are not spellings of one another and
neither is defined in terms of the other.

## Evaluation

`&&` and `||` **short-circuit**. `false && e` and `true || e` do not evaluate
`e` at all — not its faults, not its work against the evaluation budget, not
its helper calls.

This is a semantic commitment, not an optimization. Under ADR-0032 an
expression can fault (overflow, budget exhaustion), and a fault stops the
decision. Whether `false && (big + big > 0)` yields `false` or a typed
overflow fault is therefore observable in the committed outcome, so it must be
specified rather than left to the evaluator.
We specify the short-circuiting reading: the left operand decides, and an
unreached right operand contributes nothing, including nothing that can fail.

Operands must be `Bool`. A non-`Bool` operand raises the existing
`EvalFault::OperandShape`, which fails closed: no decision is committed, and
the fault surfaces at `brix check` preflight rather than only at `run`. There
is no truthiness, no coercion from `Int`, and no null.

`!` evaluates its operand and negates it, with the same `Bool` requirement.

Left operands are evaluated before right operands, and each reached operand is
evaluated exactly once. Work is charged against the same ADR-0032 budget, so an
unreached operand's cost is genuinely not spent.

## Identity and replay

`L3ExprV2` **appends** three ordinals to its canonical encoding:
`And = 13`, `Or = 14`, `Not = 15`. Ordinals `0`–`12` keep their existing
meanings and bytes.

Because the new ordinals are reachable only from source that uses the new
operators, **every program that did not use them encodes exactly as before**.
Alpha.2/alpha.3 program pins, snapshot and context identities, and the frozen
vectors are unchanged. A verifier built before this milestone fails to decode a
program that uses the operators; that is a refusal, never a misreading, and it
is the intended fail-closed behavior.

Replay runs the same evaluator over the same encoding, so short-circuiting
holds identically under `audit` and `verify`. An unreached operand is unreached
on replay, which is what makes the fault behavior above reproducible rather
than incidental.

## Frontend resource bounds

`ParseLimits::max_nesting_depth` (ADR-0022 D6) bounds recursive-descent depth
and refuses **before** descending, so a hostile module is rejected rather than
overflowing the verifier's stack.

`!` is the first prefix operator that costs a single byte per level, which
makes deep prefix nesting cheap to write. Implementing it surfaced a
pre-existing defect: the prefix parser recursed **into itself** for `prove` and
`audit` without charging depth, so those descents were never bounded and the
parser aborted the process on a deep input instead of refusing it. That
contradicted the documented contract of `LimitExceeded::NestingDepth` and,
transitively, `Type_Realization_Contract.md` §5.5, which cites the parser's
depth bound as a sound floor beneath the kernel abort it records.

Every self-recursive descent in the prefix parser now charges and releases
depth through one helper. The rule this milestone adopts: **a depth bound is
only as strong as the set of recursive call sites that charge it**, so a new
recursive parser function must route through the charged entry point or charge
directly.

This is local resource policy, not a canonical artifact. It is never encoded,
never hashed, and contributes to no identity, exactly as ADR-0022 D6 requires.

## Acceptance

- Full truth tables for `&&`, `||`, and `!`, including double negation.
- Short-circuiting is asserted against a *faulting* right operand, so the test
  fails if evaluation stops being lazy — and the complementary cases
  (`true && fault`, `false || fault`) are asserted to fault.
- Non-`Bool` operands fault with `OperandShape` on both sides of both binary
  operators and under `!`.
- Precedence and associativity are asserted at the AST for the parser and at
  the value for the evaluator, so an agreeing-but-wrong pair cannot pass.
- `&` alone is refused with a suggestion; `!=` still lexes as one token.
- Deep `!`, `prove`, `audit`, and parenthesised nesting are refused by the
  depth bound rather than aborting; a chain just under the bound is accepted;
  many shallow prefixes still parse, so depth is released and not merely
  charged.
- A finite-decision program using the operators runs end to end through the
  CLI and its program id is reproducible.
- Scalar and structured workflow pins and frozen vectors remain unchanged.
- Workspace tests, formatting, Clippy, canonical cross-checks, TCB dependency
  checks and semantic-law traceability pass before the milestone PR.
