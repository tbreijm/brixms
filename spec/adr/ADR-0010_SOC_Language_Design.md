# ADR-0010 — The SOC Language: Design & Staged Plan

Status: **Proposed** (2026-07-31) — the design pin for the "make it usable" arc, opened the moment zero-legacy landed (N9, #210).

Date: 2026-07-31.

Foundation: [ADR-0002 SOC Constitution](./ADR-0002_SOC_Constitution.md), [ADR-0003 Proof Kernel](./ADR-0003_Proof_Kernel_Profile.md), [ADR-0005 Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md), [ADR-0009 Native Type Checker](./ADR-0009_Native_Type_Checker_Parity.md). This ADR designs the **surface language** and its lowering onto the SOC machinery that already exists.

---

## 1. Context

The SOC substrate is built and zero-legacy: settle → audit → prove is live on real data (`Derived → Audited → Proven`), types are proofs for the core calculus (Curry–Howard), and a native checker replaced the legacy engine. **But there is no way to author or run a program** — the parser and runtime were deleted with the legacy. The internal representation we have (`NExpr`/`NTy`, the native checker's AST) is a *mirror of the old `brix-ir` `FrontendSource`*, shaped by the parity goal, carrying v1-lowered-subset quirks. It is scaffolding, not a language.

This ADR designs a language that is **SOC-native from the surface down** — and deliberately does *not* cement the legacy mirror as "the language."

## 2. Thesis: three pillars

A clean SOC language is **not** "a typed λ-calculus with SOC bolted on." Its semantics *are* settlement, and three things make it distinctively SOC:

### 2.1 Witnesses (the deepest primitive)
A value is never bare `T`; it is `T` **because `w`**, where `w` is a content-addressed **realization derivation** — the *why*. SOC's thesis "instance-of = carrier of an identity witness" means membership is *always* witnessed. Witnesses are **first-class, composable, checkable values**, not logs or metadata.
- **Inferred by default** ("less is more"): you write the program; the settlement engine builds the witnessed derivation, exactly as type inference already produces a typing derivation. You *touch* a witness only to prove / audit / inspect / compose.
- **Composable**: `w1 then w2` (sequential `∘` = kernel `RealizesComp`), `wf and wx` (parallel `⊗` = kernel `RealizesTensor`). Building a proof = composing witnesses (Curry–Howard).
- **Lax/tight duality = the perf story**: hot loop carries the lax composed handle (cheap, digest-at-boundary); the tight decomposition into logged generators is materialized **lazily**, only at an `audit`/`prove` boundary.

### 2.2 Graded epistemic types (the novel part)
Every fact carries a grade on the outcome lattice `Derived → Audited → Proven` — a **graded modality** `□_g T`. This is genuinely new; no mainstream language has it.
- **Downgrade is free; erasure is checked.** Silently dropping the grade (using `Estimate<T>`/`@Proven` as plain `T`) is a type error — this is literally the `EpistemicErasure` check the native checker already implements.
- **Upgrade needs evidence.** `Derived → Audited` requires a replay-audit; `Audited → Proven` requires a kernel certificate. Upgrading a fact = upgrading its witness's status.

### 2.3 Regimes (has-type is one of many)
The type checker is not privileged — it is **one realization regime**. The language lets you declare others (trust, cost, temporal, authorization) that ride the *same* settlement machinery. Same `(x, y)` can be witnessed under different regimes.

## 3. Execution model: propose → settle → commit

Control flow is not call/return; it is **deliberation plural, commitment singular**:
- `propose` introduces candidates (cheap, may land on `Unknown`, budgeted).
- the calendar **determinizes** what commits (`select_K` — natural, single).
- `commit` yields a `Derived` fact; the audit checker can lift it to `Audited`; the kernel to `Proven`.

A program's execution is a settlement to a committed set of facts, each carrying its witness and grade. "Run" = settle to fixpoint. This reads like declarative rules, executes like an incremental (O(Δ)) settlement engine, and type-checks like a proof assistant.

## 4. Surface syntax sketch (strawman syntax, real semantics)

```
// configurations — content-addressed families (algebraic, like ADTs)
config Nat  = Zero | Succ(Nat)
config Order = { items: Set<Item>, customer: Ref<Customer> }

// a realization regime; typing is the built-in one, you can add others
regime pricing {
  gen base(i: Item)            realizes i ⇒ Money            // a logged generator (𝒢)
  gen taxed(m: Money, r: Rate) realizes (m, r) ⇒ Money
}

// propose (candidate) vs commit (calendar decides) — inference fills the witness
propose total(o: Order) = sum(o.items, base) |> taxed(rate(o))
commit  total when admissible(o)

// grades are first-class and checked; witnesses are inferred but reachable
let quote : Money @Audited  = total(o)              // ok: settled + replayed
let ship  : Decision @Proven = prove policy_ok(o)   // demands a kernel certificate
let bad   : Money           = total(o)              // ERROR: erases @Audited

// witnesses on demand (Curry–Howard): compose proofs with keyword ops
witness w  = why(quote)          // the tight decomposition gₙ∘…∘g₁
audit  w                         // replay-verify → @Audited
let w3 = w1 then w2              // sequential ∘   (endpoints must meet)
let wp = wf and  wx             // parallel   ⊗

// Scala-like matching: arms are candidate realizations; exhaustiveness is PROVABLE
match n {
  Zero    => base_case
  Succ(k) => step(k)            // destructuring = a witnessed decomposition
} proving exhaustive             // optional: demand a kernel coverage certificate
```

Design commitments already agreed:
- **Composition operators are keywords** (`then` sequential, `and` parallel); symbolic fallback `|>` / `&`. `;` and `⊗` are rejected (keyboard-hostile; `;` carries statement-separator baggage).
- **Inference-first / less-is-more**: witnesses and types inferred; annotations (`@grade`, `: T`) are opt-in assertions that the checker must *discharge*.
- **Sugar over a minimal core**: `match`, comprehensions, `|>` desugar onto generators + candidates + witness composition. Small core, rich surface (Scala's template).

## 5. What lowers onto what (we are building syntax, not semantics)

| Surface | Lowers to (exists today) |
|---|---|
| `config` | configurations → `ConfigId` (brix-semantic) |
| `gen … realizes` | generators 𝒢 / `GeneratorId`; `Realizes` proposition |
| `propose` | candidates + `Adm` (soc-core) |
| `commit` | `commit_tick` / calendar `select_K` → `Derived` |
| `@Audited` | audit-factorization checker (soc-core) |
| `prove` / `@Proven` | `brix-elaborate` → `brix-kernel` acceptance → certificate |
| `then` / `and` | `RealizesComp` (∘) / `RealizesTensor` (⊗) — kernel Profiles 1.1/1.2 |
| type-check | the native regime (soc-regimes) as one realization regime |
| `match … proving exhaustive` | a coverage proposition elaborated to the kernel |

**The semantics already exist.** "Make it usable" is: (a) a surface grammar, (b) a lowering pass to configurations + generators + candidates, (c) driving the settlement loop, (d) surfacing witnesses/grades/proofs.

## 6. THE decision this ADR needs: ambitious vs conservative

- **(A) General SOC language (recommended).** Typing is *one regime*; witnesses, grades, and regimes are in the surface DNA. Distinctive, matches the "has-type = one regime" thesis, and is the only version that makes first-class composable witnesses ergonomic. Higher design cost.
- **(B) Typed FP language that *uses* SOC underneath.** Grades/witnesses as a library behind a conventional typed core. Faster to a usable REPL; throws away most of what makes SOC SOC — first-class witnesses quietly become a side-channel.

**Fable's recommendation: (A).** Zero-legacy exists precisely to earn the freedom to build (A); (B) spends that freedom on convention. This is the one call to make with the user before any parser code.

Lineage (honest): Datalog / Datomic (facts + rules + settlement) × dependent types / Lean (proofs as values) × differential dataflow (incremental O(Δ)) × **graded modal types** (the novel core).

## 7. Staged plan

- **L0 — Pin the language (this ADR).** Ratify (A) vs (B); freeze the three pillars, the propose/commit model, and the keyword-operator + inference commitments. Write a small `.soc` example corpus by hand (the "what should typecheck / settle / prove" fixtures — replacing the deleted `brix-conformance` corpus with native ones).
- **L1 — Grammar + parser → a native surface AST.** A *fresh* AST (not the `NExpr` mirror), designed for the surface. New crate `soc-syntax` (or `soc-lang`). Reconcile/retire the two current internal reps (the checker's `NExpr`/`NTy` and `type_realization`'s `Expr`/`Ty`).
- **L2 — Lowering: surface AST → configurations + generators.** The core semantic bridge: parse result becomes `ConfigId`s + `GeneratorId`s + candidate proposals. Type-check is the native regime over this.
- **L3 — Execution: drive the settlement loop.** `run` = `commit_tick` to fixpoint; surface the committed facts with grades. First end-to-end: author a tiny program, settle it, read `Derived`/`Audited` results.
- **L4 — Proof surface.** `prove` / `@Proven` / `match … proving exhaustive` wired to the elaboration→kernel path; grades enforced in the checker (erasure already implemented).
- **L5 — Self-hosting horizon.** Express (parts of) the checker as SOC *data* (generators + configs the engine runs) rather than Rust — the deep form of "written in Brix" (see the earlier self-hosting discussion). Gated on L0–L4.

Each stage gets its own ADR/slice; L0 (this decision) is the blocker for all of it.

## 8. Open questions
- **The arrow-kind in the surface.** Witnesses are morphisms, not objects; the kernel has `Realizes`/`ObjectTerm`, the surface has no notion of "arrow / correspondence" yet. Making it ergonomic is the crux.
- **Witness ergonomics.** Invisible-by-default yet one-keyword-away. Inference + handles is the plan; the exact affordances are unsolved.
- **Regime-polymorphism.** Can code be generic over regimes (a proof reusable for typing *and* cost)? Powerful, unexplored.
- **Dimensions.** Real dimensional analysis with *witnessed unit conversions* (post-parity backlog) — a good first "regime beyond typing."
- **Corpus.** Native `.soc` fixtures replace the deleted `FrontendSource` corpus; what's the golden format (settlement outcomes + optional proofs)?

This ADR is the design pin; §6 is the decision to ratify before L1.
