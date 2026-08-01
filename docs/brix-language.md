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

## 3. Epistemic Grades and Honest Status

Brix categorizes statement outcomes using three epistemic grades:

- **`@Proven`**: Certified end-to-end by the kernel down to discharged tight-generator leaves.
- **`@Audited`**: Certified compositionally given primitive generator leaves whose semantic validity remains open.
- **`@Derived`**: Unverified candidate facts recorded in the settlement hot loop.

### Honest Status of Type Checking

The proof kernel certifies the *composition* theorem — GIVEN the primitive typing-rule leaves as generators, the derivation establishes `e : T`. The honest grade is `@Proven` only when every leaf is discharged tight. Literals, the simply typed λ-calculus core, nonempty records/field access, nonnullary constructors, and explicit-constructor matches now meet that condition. Arithmetic remains `@Audited`; so do zero-field records, nullary constructors, and wildcard/variable catch-all matches, whose kernel rules are not yet available or fully represented.

---

## 4. Type Normalization & Coercion Lattices

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

## 5. Not Yet Supported

The following surface features are not yet in the L2 lowering fragment:

- **Witness composition:** Sequential composition (`then` / $\circ$) and parallel composition (`and` / $\otimes$).
- **Proof & Explanation Keywords:** `prove`, `why`, and `audit`.
- **Regime & Rule Declarations:** Surface `regime`, `gen`, and `rule` checking.
- **Recursive/Custom Sum Payloads:** Constructor payloads are currently limited to `Int`, `Str`, and `Float`; recursive sums remain deferred.
- **Full Structural Discharge:** Empty records and nullary constructors require a kernel unit proposition, while wildcard/variable catch-all matches require explicit repeated-branch premises; these forms type-check but remain `@Audited`.

---

## 6. Roadmap

- **Generator Discharge:** Add unit/nullary and catch-all proof schemas, then discharge arithmetic and numeric coercion semantics when value execution exists.
- **Fragment Expansion:** Add recursive/custom sum payloads and witness composition (`then`/`and`).
- **Fixpoint Execution:** Introduce L3 `brix run` settlement to evaluate programs to fixpoints.
