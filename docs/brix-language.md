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
- **Structural Records & Field Access:** Record construction `Item { a: 1, b: 2 }` and field access `p.a` (evaluated structurally; the config name is currently ignored).
- **Arithmetic:** Operators `+`, `-`, `*`, `/` over a numeric coercion lattice with witnessed `Int ↪ Float` promotion (division `/` yields the field of fractions, `Int / Int → Float`).

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

#### Composite Expressions (Earn `@Audited`)

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
  p : {a: Int, b: Int} @Audited
  v : Int @Audited
  r : Int @Audited
```

---

## 3. Epistemic Grades and Honest Status

Brix categorizes statement outcomes using three epistemic grades:

- **`@Proven`**: Certified end-to-end by the kernel down to discharged tight-generator leaves.
- **`@Audited`**: Certified compositionally given primitive generator leaves whose semantic validity remains open.
- **`@Derived`**: Unverified candidate facts recorded in the settlement hot loop.

### Honest Status of Type Checking

The proof kernel certifies the *composition* theorem — GIVEN the primitive typing-rule leaves as generators, the derivation establishes `e : T`. It does NOT yet prove the semantic validity of those leaves themselves (the open "tight-generator soundness obligation"). Therefore the honest grade of a typing RESULT is `@Audited` by default. It is `@Proven` ONLY when every generator in its derivation has been discharged to "tight". So far ONLY the literal introduction rules (integer/string/float literals) are discharged (they are definitional — an introduction rule IS the type's definition). Concretely: `let x = 42` → `x : Int @Proven`; `let s = "hi"` → `s : Str @Proven`; but `let c = 1 + 2` → `c : Int @Audited` (arithmetic generator not discharged), records/fields → `@Audited`, functions/application → `@Audited`.

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

- **Named-config resolution:** Record fields are checked structurally; explicit `config` definition matching is deferred.
- **Pattern matching:** `match` expressions.
- **Witness composition:** Sequential composition (`then` / $\circ$) and parallel composition (`and` / $\otimes$).
- **Proof & Explanation Keywords:** `prove`, `why`, and `audit`.
- **Regime & Rule Declarations:** Surface `config`, `rule`, and `regime` checking.

---

## 6. Roadmap

- **Generator Discharge:** Discharge primitive typing-rule leaves to "tight", promoting composite typing results from `@Audited` to `@Proven`.
- **Fragment Expansion:** Add named-config resolution, `match`, and witness composition (`then`/`and`).
- **Fixpoint Execution:** Introduce L3 `brix run` settlement to evaluate programs to fixpoints.
