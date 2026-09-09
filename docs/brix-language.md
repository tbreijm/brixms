# Brix Language Overview

**Brix** (files: `.brix`) is a programming language built on the **SOC paradigm**.

> **Paradigm vs. Language:**  
> Just as Object-Oriented Programming (OOP) is the paradigm and Java is a language realizing it, **SOC is the paradigm and Brix is the language.**

Brix provides witness-first, graded epistemic typing with progressive disclosure: types and epistemic grades (`@Proven`, `@Audited`, `@Derived`) are inferred automatically and stay invisible for everyday code, while remaining fully accessible to power users and proof engineers.

---

## 1. Checking Brix Code: `brix check`

The primary command for inspecting Brix source files is `brix check`:

```bash
brix check <file.brix>
```

`brix check` parses the surface source text, lowers the AST onto native realization expressions, and type-checks each top-level `let` binding. For each binding, it outputs:

```text
  name : Type @Grade
```

For example, checking a file with literal bindings yields:

```text
  x : Int @Proven
  s : Str @Proven
```

For finite-decision modules declaring external inputs (`input <name>: <type>`), `brix check <file.brix>` without `--input` performs a declaration-only contract check (`status: checked-input-contract`), validating syntax, imports, and plan lowering without requiring inputs. Supplying `--input <path>...` performs complete preflight validation against the provided input shards.

---

## 2. What Type-Checks Today (The L2 Fragment)

The current L2 implementation fragment supports:

- **Literals:** Integer (e.g. `42`), String (e.g. `"hi"`), and Float (e.g. `3.14`) literals.
- **Let Bindings:** `let name = expr` top-level declarations.
- **Functions & Application:** `fn` definitions, lambdas, and function calls (`Call`), inlined to application `App(Lam, arg)`.
- **Records & Field Access:** Structural record construction and projection, plus validation of declared record configs for missing and unknown fields.
- **Finite Sums & Matching:** Primitive-payload sum configs, constructor application, exhaustive `match`, and optional kernel-certified `proving exhaustive` coverage.
- **Arithmetic:** Operators `+`, `-`, `*`, `/` over a numeric coercion lattice with witnessed `Int ↪ Float` promotion (division `/` yields the field of fractions, `Int / Int → Float`).
- **Grade Assertions:** `@Proven`, `@Audited`, and `@Derived` assertions checked through the grade lattice; strengthening beyond the earned grade is rejected.

### Runnable `.brix` Snippets and Exact Output Grades

#### Literals (Earn `@Proven`)

```brix
let x = 42
let s = "hi"
let f = 3.14
```

Output of `brix check`:
```text
  x : Int @Proven
  s : Str @Proven
  f : Float @Proven
```

#### Composite Expressions (Earn Their Weakest Leaf Grade)

```brix
let c = 1 + 2

let p = Item { a: 1, b: 2 }
let v = p.a

fn double(x) = x + x
let r = double(2)
```

Output of `brix check`:
```text
  c : Int @Audited
  p : {a: Int, b: Int} @Proven
  v : Int @Proven
  r : Int @Audited
```

---

## 3. External Inputs (`input` Declarations & Schemas)

Finite-decision modules under `brix.l3.finite-decision@1` ([ADR-0031](../spec/adr/ADR-0031_External_Input_Alpha.md)) support declaring external operational parameters at top level:

```brix
input stock: Int
input threshold: Int
input urgent: Bool
```

### Syntax and Declaration Rules

- **Syntax:** `input <name>: <type>` at top level; newline-delimited without trailing semicolons.
- **Scope:** Declared inputs are bound into expression scope for subsequent rule bodies, candidate proposal guards (`when`), proposal values, and `show` expressions.
- **Exact Scalar Types:** Supported input types are strictly limited to scalars:
  - `Int`: signed 64-bit integer (`i64`)
  - `Bool`: boolean (`true` or `false`)
  - `Str`: UTF-8 string scalar sequence
  Non-scalar types (records, sum variants) and floating-point numbers (`Float`) are rejected fail-closed; no lossy coercions are permitted.
- **Identifier Collision Discipline:** Input names participate in top-level collision checks and must not shadow or duplicate other inputs, `let` bindings, rules, or proposals.

### Artifact Schema: `brix.input@1`

External inputs are transported in strict JSON files conforming to schema `brix.input@1`:

```json
{
  "schema": "brix.input@1",
  "values": {
    "stock": {
      "type": "int",
      "value": "12"
    },
    "urgent": {
      "type": "bool",
      "value": true
    }
  }
}
```

- **Strict Envelope:** The root JSON object must contain exactly `"schema": "brix.input@1"` and `"values"`. Any unrecognized key causes fail-closed rejection.
- **Tagged Scalar Values:**
  - `int`: `{"type": "int", "value": "<decimal-string>"}` (string-encoded decimal representation avoiding IEEE 754 precision loss; leading zeros and overflow are rejected).
  - `bool`: `{"type": "bool", "value": true | false}` (native JSON boolean).
  - `string`: `{"type": "string", "value": "<string>"}` (native JSON string).
- **Zero-Tolerance Duplicate Key Rejection:** Duplicate keys anywhere in the JSON artifact (in the root envelope, inside `values`, or within tagged scalar objects) are strictly rejected without applying last-write-wins.
- **Bounded Resource Limits:** File size is bounded to 1 MiB per file, an aggregate limit of 4 MiB across at most 16 shard files, at most 256 inputs, 64-byte identifier length limit, and 64 KiB string value limit.

### CLI Behavior & Epistemic Status

- **Separable Declaration and Completeness Validation:**
  - `brix check <file.brix>` without `--input` validates syntax, imports, and input declarations (`status: checked-input-contract`), establishing a stable `ProgramId` that binds declarations without values.
  - `brix check <file.brix> --input <path>...` performs complete preflight validation, asserting type alignment, total coverage (no missing or unexpected inputs), and dry-run deliberation.
- **Repeatable Disjoint Shards:** Multiple `--input <path>` (or `--input=<path>`) flags supply disjoint input shards. Shards must declare mutually exclusive keys; duplicate keys across shards fail closed. Snapshot canonicalization sorts keys lexicographically by NFC name, ensuring order-independent identity.
- **Snapshot & Context Identity:** `FiniteDecisionProgramId` binds input declarations (name, type, ordinal), never input values. Supplied values are canonicalized into an `InputSnapshotId` (`Domain::Snapshot`). Deliberation `ContextId` incorporates both `ProgramId` and `InputSnapshotId`.
- **Epistemic Grade `@Derived`:** External input values enter deliberation strictly at epistemic grade `@Derived` (unverified external claims). They never ambiently upgrade to `@Audited` or `@Proven`. Deliberated outcomes commit as `@Derived`.
- **Offline Audit Verification:** `brix verify` re-derives the input snapshot and deliberation context directly from caller-supplied input files, refusing to trust unverified snapshot claims. Verification under `--profile l3-v1` rejects `--input`.

---

## 4. Epistemic Grades and Honest Status

Brix categorizes statement outcomes using three epistemic grades:

- **`@Proven`**: Certified end-to-end by the kernel down to discharged tight-generator leaves.
- **`@Audited`**: Certified compositionally given primitive generator leaves whose semantic validity remains open, or verified independently via offline audit input transport bundles (issuing separate `@Audited` audit receipts).
- **`@Derived`**: Unverified candidate facts and runtime settlement commitments (the runtime decision remains `@Derived`; successful independent replay issues and verifies separate `@Audited` receipts).

### Honest Status of Type Checking

The proof kernel certifies the *composition* theorem — GIVEN the primitive typing-rule leaves as generators, the derivation establishes `e : T`. The honest grade is `@Proven` only when every leaf is discharged tight. Literals, the simply typed λ-calculus core, nonempty records/field access, nonnullary constructors, and explicit-constructor matches now meet that condition. Arithmetic remains `@Audited`; so do zero-field records, nullary constructors, and wildcard/variable catch-all matches, whose kernel rules are not yet available or fully represented.

---

## 5. Type Normalization & Coercion Lattices

Type normalization in Brix is governed by `CoercionLattice` — a declared category of witnessed coercions over type sorts, executing on a single unified code path.

Two lattice instances run on this mechanism:

1. **`NUMERIC` Lattice:**
   - Hierarchy: $\text{Nat} \hookrightarrow \text{Int} \hookrightarrow \text{Rat} \hookrightarrow \text{Real} \hookrightarrow \text{Complex}$ (safe widening) plus a lossy $\text{Int} \hookrightarrow \text{Float}$ branch.
   - Note: $\text{Float}$ is incomparable to exact $\text{Rat}/\text{Real}/\text{Complex}$ nodes ($\text{join}(\text{Float}, \text{Rat}) = \text{None}$); attempting to mix float and exact rational/real types results in a type error.
2. **`GRADE` Lattice:**
   - Hierarchy: $\text{Proven} \hookrightarrow \text{Audited} \hookrightarrow \text{Derived}$ (safe weakening of certainty).
   - The forbidden strengthening $\text{Derived} \to \text{Proven}$ has no upward path and is rejected as **epistemic erasure**.

### Mixed Arithmetic & Division Examples

```brix
// Division yields the field of fractions (Int / Int -> Float)
let ratio = 7 / 2

// Mixed integer and float addition (Int safely coerces to Float)
let mixed = 1 + 2.5
```

Output of `brix check`:
```text
  ratio : Float @Audited
  mixed : Float @Audited
```

---

## 6. Not Yet Supported

The following surface features are not yet in the L2 lowering fragment:

- **Witness composition:** Sequential composition (`then` / $\circ$) and parallel composition (`and` / $\otimes$) are parsed in syntax but not supported in lowering.
- **Surface Expression Keywords:** keywords like `why` and `audit` inside expressions (these are CLI driver subcommands over `.brix` files, not surface expression operators).
- **Surface Regime & Rule Declarations:** Surface `regime` and `gen` syntax; `rule` declarations are evaluated by L3 execution profiles, not L2 type-realization bindings.
- **Directly Recursive Functions:** Recursive `fn` definitions are refused because functions are currently inlined.
- **Recursive/Custom Sum Payloads:** Constructor payloads are currently limited to `Int`, `Str`, and `Float`; recursive sums remain deferred.
- **Full Structural Discharge:** Empty records and nullary constructors require a kernel unit proposition, while wildcard/variable catch-all matches require explicit repeated-branch premises; these forms type-check but remain `@Audited`.

---

## 7. Roadmap & Execution Profiles

- **Finite-Decision Alpha (`0.1.0-alpha.3`):** The `brix.l3.finite-decision@1` candidate deliberation profile ([ADR-0030](../spec/adr/ADR-0030_Finite_Decision_Alpha.md)) is implemented across `brix-syntax`, `soc-regimes`, `brix-lower`, and `brix-cli`. In `0.1.0-alpha.3`, external operational inputs ([ADR-0031](../spec/adr/ADR-0031_External_Input_Alpha.md)) are integrated via `input <name>: <type>` declarations, strict `brix.input@1` schema decoding, bounded disjoint shards, and deterministic context identity binding. It evaluates complete candidate frontiers, structured rejection reasons, and deterministic calendar selection at phase zero. Decisions are committed as `@Derived` at runtime; the runtime decision remains `@Derived`, while successful independent replay issues and verifies separate `@Audited` audit receipts via `brix verify` ([ADR-0026](../spec/adr/ADR-0026_Audit_Input_Transport_Bundle.md)).
- **CLI Driver:** The live toolchain provides six file-oriented subcommands: `check`, `run`, `audit`, `verify`, `why`, and `whynot`. All six subcommands support repeatable `--input <path>` (or `--input=<path>`) flags for finite-decision workflows; `verify --profile l3-v1` rejects `--input`.
- **L3 v2 Derivation:** Stages A–C ([ADR-0027](../spec/adr/ADR-0027_L3_V2_Derivation.md)) are landed in `brix-lower`, defining the derivation evaluator and eligibility rules on committed dependencies.
- **Generator Discharge:** Add unit/nullary and catch-all proof schemas, then discharge arithmetic and numeric coercion semantics when value execution exists.
- **Fragment Expansion:** Add recursive/custom sum payloads and witness composition (`then`/`and`).
