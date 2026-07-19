# Brix CLI executable scenario subset

`brix test` is compiler-grounded and currently executes a deliberately small
scenario subset. This is an implementation contract for the runtime facts the
current toolchain can establish, not full BrixMS v9 scenario conformance.

A scenario is executable when all of these conditions hold:

- it explicitly declares one fixed natural-number seed, not `seed each`;
- it has no clock/protocol bindings;
- it has no `setup`, `step`, or `at` transaction blocks;
- it declares at least one `assert at end` assertion;
- every assertion uses only Boolean literals, parentheses, `!`, `and`, and
  `or`.

The parser rejects missing and duplicate `seed` declarations before execution,
so `check`, `test`, and `quality` share the same single-explicit-seed invariant.

The evaluator processes declarations and assertions in source order. Selectors
are exact scenario names; duplicate declarations or unknown selectors fail the
gate. False assertions are evaluated failures. Any selected scenario outside
the subset is unavailable, and its unsupported constructs are included in the
structured evidence. A known failure takes precedence over unavailable work,
because that is already sufficient to reject the gate.

| Code | Meaning |
|---|---|
| `BRX-TEST-0000` | all selected supported scenarios passed |
| `BRX-TEST-0001` | selected scenario semantics are unavailable |
| `BRX-TEST-0002` | a supported assertion evaluated to false |
| `BRX-TEST-0003` | selectors are unknown or scenario names are ambiguous |

JSON and SARIF results contain the aggregate status, requested selectors,
supported-subset version, and per-scenario/per-assertion evidence. The output
contains no timestamps or random identifiers and is byte-stable for repeated
runs against the same path and source.
