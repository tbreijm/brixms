# Issue #53: Gemini implementation handoff

Prepared 2026-10-27 for work on this machine. Parent issue:
[#53](https://github.com/tbreijm/brixms/issues/53).

This is an implementation brief, not a new normative ruling or a claim that
all of #53 is ready to close. The requested next feature is closure of the
arithmetic typing bridges. A separate, smaller conformance slice is identified
below so it can proceed without waiting on unresolved design decisions.

## Workspace and starting point

- Repository: `/Users/tonyreijm/Projects/brixms-v3`.
- Review baseline: `origin/main` at `25b130a` (alpha.8, PR #344).
- Read `CONTRIBUTING.md`, `DEPS.md`, relevant crate `OWNER.md` files, and any
  applicable agent instructions before editing.
- Inspect `git status` and the current branch. Use a separate branch/worktree
  from current `origin/main`; preserve the README/handoff PR and other local work.
- No Gemini process has been launched by this handoff. The local `gemini`
  executable is available at `/opt/homebrew/bin/gemini`.

## Current behavior to preserve until justified

`brix check` on `let x = 1 + 2` currently prints:

```text
  x : Int @Audited = 3
    arithmetic typing leaf: kernel-checked
    remaining obligations: g_arith_input, g_arith_result
```

ADR-0015 Stage D already closes supported `g_arith` occurrences using
`PrimRealizes` and independent kernel acceptance. The two bridge leaves remain
open hypotheses. A closed arithmetic leaf is not a fully proved expression,
and a typing proof does not prove the computed value or termination.

Use `crates/brix-cli/tests/arithmetic_typing_leaf.rs` and
`crates/brix-elaborate/tests/arithmetic_leaf_closure.rs` as executable baselines.
Do not globally mark `g_arith` or its bridges tight to force a grade change.

## Read and reconcile the existing design first

Read these documents together; distinguish historical findings from current
source behavior and check each document's status:

- `spec/Type_Realization_Contract.md`, especially §§3.4.2, 4.6, 9–12.
- `spec/adr/ADR-0015_Judgment_Scoped_Tightness.md`, including its Stage D errata.
- `spec/adr/ADR-0023_Primitive_Relation_Identity.md`, especially §4.
- `spec/adr/ADR-0025_Pinned_Endpoint_Identities.md` in full. Its header is still
  **Proposed**, although parts of its machinery have landed. It proposes pinned
  foreign endpoint digests with owner-side re-derivation; it does not authorize
  copying the regime's encoder into the kernel or moving all of `Ty` into it.
- `spec/adr/ADR-0028_Typing_Over_Context_Expression_Pairs.md`: preserve context
  binding when identifying typing endpoints.

Produce a short implementation map of what already exists, what remains, and
which decisions are accepted. Source comments may describe older tree shapes;
verify actual emitted endpoints and current operator-binding tests rather than
treating those comments as proof of a present vulnerability.

If bridge closure requires an unresolved normative choice, draft the precise
erratum/ADR amendment with affected conformance clauses. `CONTRIBUTING.md`
requires Tony's ruling before implementing a spec ambiguity. A proposed ADR's
presence alone is not approval. Continue independent conformance work while
that decision is pending.

## Primary slice: close arithmetic typing bridges

Bound the first implementation to the existing finite numeric typing vocabulary
and supported operator/coercion combinations. Do not add arithmetic evaluation
semantics, numeric types, or general proof search.

### Result bridge

Account for the exact correspondence between `NumericResultTypeV1` and the
regime's canonical type endpoint. Implement only the accepted identity/pinning
mechanism. If the bridge is removed rather than proved, show that the replacement
endpoint and enclosing composition are exactly the intended typing judgement.

### Input bridge

The proof must bind all of the following, not merely admit a valid arithmetic row:

- The original expression's operator, carried through structural decomposition.
- Ordered operand types and their exact contextual endpoints.
- The selected promotion/coercion paths, including exact versus lossy edges.
- The resulting arithmetic input object and the relation/version that admits it.

A shared endpoint identity alone cannot establish these claims. Preserve the
purely structural ground of `g_arith_split`; it must not acquire unchecked
translation, promotion selection, or normalization work.

### Acceptance

- Every claimed closed bridge has an explicit proof accepted by the kernel for
  the exact generator, endpoints, proposition, and context.
- Owner-side re-derivation and independent vectors support any pinned foreign
  identities. Preserve frozen identities unless a separately approved versioned
  change is necessary; never regenerate golden values just to make tests pass.
- Cover every admitted finite operator/type/coercion combination, with expected
  results authored independently of the emitter under test.
- Reject transplanted operators (including Add/Sub with identical operand types),
  wrong operand/result endpoints, altered coercion paths, wrong relation/version,
  wrong context, and mismatched composition middles. Rejection or exhaustion must
  never mint a stronger grade or logical refutation.
- Independent rechecking of the final certificate succeeds without trusting the
  inference engine's claim that its own bridges are correct.
- An expression reaches `@Proven` only when all of its leaf obligations are
  discharged and the exact final typing proposition is accepted. Where any remain,
  preserve `@Audited` and report the actual remaining obligations.
- Human and JSON CLI output continue to distinguish the typing judgement from the
  evaluated value. Check nested arithmetic, helper/lambda contexts, integer
  addition, and division's existing coercion/typing behavior.
- Update the normative ledger and source explanations with named positive,
  negative, and judgement-scope evidence. Do not close the whole of #53.

## Independent smaller slice: missing discharge evidence

The contract's §3.4.2 records `g_then_split`, `g_tensor_split`, and `g_tensor`
as declared tight without all §9.2 faithfulness and negative pins. Reconfirm
that gap against current tests before adding anything.

For each remaining gap, add meaningful checks that the real emitted tree matches
the stated structural/kernel ground, fails when its precondition is violated,
and is scoped to `ClaimKind::Typing`. Include a kernel acceptance/rejection check
where the ground is a kernel rule. Demonstrate sensitivity to a deliberately
wrong endpoint or premise rather than merely testing the whitelist.

This slice must preserve existing grades and canonical identities. Promote a
ledger clause from Partial only when the named checks justify it. If the ground
itself is invalid, report the counterexample and required ruling rather than
silently changing public grades. Keep this as its own reviewable commit/PR.

## Code map

| Area | Files |
|---|---|
| Generators, tree emission, grading, contract pins | `crates/soc-regimes/src/type_realization.rs` |
| Primitive schemas, pinned endpoints, row membership | `crates/brix-kernel/src/prim_schema.rs`, `prim_pinned.rs`, `prim_registry.rs`, `check.rs` |
| Certified tree elaboration and verification | `crates/brix-elaborate/src/tree_proof.rs` |
| Tree identity and structure | `crates/brix-semantic/src/tree.rs` |
| CLI grading regression | `crates/brix-cli/tests/arithmetic_typing_leaf.rs` |
| Independent arithmetic proof tests | `crates/brix-elaborate/tests/arithmetic_leaf_closure.rs` |

Search symbols rather than relying on line numbers. Inspect existing operator
binding, endpoint-vector, and coercion tests before designing replacements.

## Validation and delivery

Start with focused tests for the touched packages:

```sh
cargo test -p soc-regimes type_realization
cargo test -p brix-elaborate --test arithmetic_leaf_closure
cargo test -p brix-cli --test arithmetic_typing_leaf
```

Run kernel/semantic tests when those crates change, plus the relevant frozen
vector and TCB checks. Before delivery, run the repository's contribution gates:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`python3 scripts/test_workspace.py` is the documented parallel alternative for
the workspace tests, including doctests. Avoid concurrent full-suite runs.

Deliver the implementation map, any required ruling, separate commits for the
independent evidence slice and bridge closure, test results, and remaining #53
obligations. General context contents (#59), comparison/recursion/catch-all
discharge, certified refutation, and incremental typing invalidation remain
outside this handoff.
