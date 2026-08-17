# ADR-0027 — L3 v2: A Derivation-Capable Executable Profile

Status: **Proposed** (2026-08-16). Defines the successor to
[ADR-0012](./ADR-0012_L3_Executable_Settlement.md)'s `brix.l3.rule-agenda-saturated@1`, which
cannot express a computation. Governs issue #178 and the "run a real program" goal.

Date: 2026-08-16.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail
closed, §8 the behavior signature, §9.1 the O(Δ) invariant),
[ADR-0012: L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (⟨D-STATUS⟩,
⟨D-CAND⟩, ⟨D-TAUZERO⟩, ⟨D-RESIDUE⟩, ⟨D-PROFILE⟩, §10),
[ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md),
[ADR-0014: Divergence-Sensitive Saturation](./ADR-0014_Divergence_Sensitive_Saturation.md),
[ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-JUDGE⟩),
[ADR-0010: SOC Language Design](./ADR-0010_SOC_Language_Design.md) (§7a ⟨D-OPARROW⟩).

This ADR rests on two independent design reviews commissioned for it. Where they agreed the
agreement is recorded as such; where they disagreed §3 and §7 say so and rule.

---

## 1. What v1 cannot do

Measured against the running binary, not inferred. `brix run` accepts `config` declarations,
`let` bindings of nullary constructors and record literals, and **zero-parameter rules** whose
bodies are those same closed forms. It rejects, by name:

```text
rule referencing a rule   UnclosedReference / CallNotAllowed
parameterized rule        ParameterizedRule
payload constructor       PayloadBearingConstructor
field access              FieldAccessNotAllowed
match                     MatchNotAllowed
arithmetic                ArithmeticNotAllowed
comparison                ComparisonNotAllowed
generic config            UnknownTypeName
```

The first line is the one that matters. **A rule cannot depend on another rule's fact**, so every
rule is an independent constant, the agenda commits each once, and the run quiesces. ADR-0012
⟨D-TAUZERO⟩ already says as much — "saturation over L3 v1 is the identity" — so this is a
deliberately minimal first profile rather than a defect, and both reviews confirmed that reading
against the code.

The consequence is worth stating plainly rather than as a feature list: **there is no computation
in the executable fragment.** v1 settles a set of constants. A duel engine, or any program, needs
facts derived from facts.

## 2. Decision — ⟨D-V2PROFILE⟩ a new profile, never a widened v1

> L3 v2 SHALL mint a new execution-profile marker and a new plan format version. It SHALL NOT
> accept any formerly rejected form under `brix.l3.rule-agenda-saturated@1`.

Not a judgement call — ADR-0012 §10 forecloses it:

> Adding source-level transition guards, parameters, payload-bearing constructors, general
> regimes, effects, administrative steps, or candidate-work limits requires a new ADR slice and a
> new execution-profile marker. It cannot broaden this profile by accepting formerly rejected
> forms under the same `brix.l3.rule-agenda-saturated@1` identity.

Versioning obligations, all additive: new artifact types (`L3PlanV2`, `ProgramIdV2`, `FactV2`,
`L3WorldV2`, `SettlementRunV2`); new vectors, never an edit to `vectors/l3_plan_v1.json`;
exact-version decoding with unknown plan or profile versions failing closed; every v1 program,
world, fact, rule, run, journal and certificate identity preserved unchanged; the v2 profile
marker and the evaluator-semantics version bound into `ProgramIdV2`; `CommittedStep`, the outcome
lattice, the authority table and the certificate envelope untouched; and a differential suite
proving the v1 fragment still produces byte-identical results under v1.

**⟨D-TAUZERO⟩ and the O(Δ) gate's verbatim applicability are forfeited together** by any profile
that moves a generator into `𝒢_τ`. ADR-0012 §13 is explicit that such a profile must re-derive
both conveniences rather than inherit them.

> **Erratum (2026-08-17, found while implementing Stage B).** This section originally asserted
> that v2 *does* forfeit them. **It does not, and the correction is a simplification rather than a
> repair.**
>
> A v2 rule body evaluates **atomically inside one commit**, so every committed step publishes a
> fact and is *realizing*. No generator moves into `𝒢_τ`, `𝒢_τ = ∅` is preserved, and ⟨D-TAUZERO⟩
> and the O(Δ) gate carry over unchanged. v1's fixture asserting no step is ever `Administrative`
> is **carried forward, not replaced**.
>
> The same correction retires ⟨D-MEASURE⟩ for v2 — see §4.

## 3. Decision — ⟨D-DERIVE⟩ derivation is an acyclic rule-fact dependency

> A rule SHALL **declare** the facts it reads as its parameters. Each parameter names an earlier
> rule and binds that rule's already-committed fact. A body SHALL read only its declared
> parameters. It SHALL NOT re-evaluate or invoke the referenced rule. Dependencies SHALL be
> acyclic, and a rule commits at most once.
>
> ```brix
> rule base()        = 1500
> rule boosted(base) = base + 500
> ```

**Erratum (2026-08-17, maintainer ruling during Stage C).** This originally had a rule remain
zero-parameter, with dependencies **extracted** by scanning the body for bare references. The
parameter list is strictly better and replaces it:

- The dependency is **declared**, so the plan's dependency list cannot drift from what the body
  actually reads. Under extraction the two were the same only because one was computed from the
  other; under declaration the body can read *only* what the signature names, and an undeclared
  read is `UndeclaredFactRead` rather than a silent new edge.
- It removes a surprising asymmetry. Under extraction a bare name meant "read a fact" inside a
  rule and "unresolved" inside a `let`, with nothing in the source marking the difference.
- The dependency is visible at the declaration rather than only by reading the whole body.

**This does not reopen §7's deferral of schemas, and the distinction is the load-bearing one.** A
v2 parameter names exactly **one rule** and binds exactly **one fact**, so there is no
quantification and no grounding domain to supply. §7 defers parameters that range over *values* —
a different construct that v2 still does not have.

**The two reviews disagreed here, and the disagreement resolves rather than persists.** One
argued for exactly the above. The other argued that rules should *observe the world* through
guards — `rule b when Fact(a, v) => …` — and that `UnclosedReference` is therefore correct and
should stay, because in a settlement substrate rules do not call each other.

The second is the better long-run shape and it is not available yet: a guard quantifies,
quantification is a rule *schema*, and schemas need a grounding discipline this AST cannot express
(§7). So the two are not rival designs but consecutive ones. **v2 is the acyclic fact dependency;
v3 is guards and schemas.** Recording this so the v3 author does not read v2's bare reference as
the intended end state — it is a stepping stone, and a rule reference is a narrower thing than a
guard precisely because it names one fact rather than quantifying over many.

Two dependencies hide inside "b depends on a", and they must not be conflated:

- **Enablement** — `b` is a candidate only once `a`'s fact exists. Settlement-shaped; lives in
  `Regime::candidates`.
- **Value** — `b`'s payload is computed from `a`'s. Evaluation; a metafunction over values, not a
  settlement step.

v2 admits both, and §5 keeps the second from laundering its grade through the first.

### ⟨D-CANDTOTAL⟩ `Regime::candidates` stays total and unbounded

> v2 SHALL NOT make `candidates` bounded or fallible. It therefore requires **no v2 quiescence
> certificate**.

ADR-0012 §10 requires a v2 certificate only for a bounded or fallible `candidates`; ADR-0012 §4.2
already anticipates a profile with a large candidate set and says it "will pay by minting a v2
certificate". v2 declines to pay, because it does not need to: enumeration stays exhaustive and
total, and the single terminal re-enumeration `check_quiescence_certificate` performs is a
once-per-run cost. Both reviews independently reached this, and it is the single largest saving
available — protect it.

## 4. Decision — ⟨D-TERMINATES⟩ v2 terminates structurally; the measure is a v3 concern

> A v2 run SHALL terminate because each rule commits **at most once** and the dependency graph is
> **acyclic** (⟨D-DERIVE⟩). A plan of `N` rules therefore admits at most `N` commits. No progress
> measure, and no per-step measure check, is required.

**Erratum (2026-08-17), same origin as §2's.** This section originally ruled ⟨D-MEASURE⟩ — a
well-founded ordinal carried in the world, with an O(1) per-step decrease check. That was designed
against a more ambitious v2 in which derivation could loop. ⟨D-DERIVE⟩ removed the possibility:
commit-at-most-once plus acyclicity makes termination *structural*, and a measure would be
machinery guarding a case that cannot arise.

**The measure returns in v3**, and the reason is precise: a rule *schema* instantiated over a
growing fact base can commit repeatedly, so "at most once" no longer bounds the run. That is the
profile that needs a well-founded measure, and it should adopt this section's original design.

What does **not** change is the ruling against divergence certificates, which stands for v2 and
for every successor:

**Free recursion relying on ADR-0014's divergence certificates is rejected**, and both reviews
killed it for complementary reasons worth recording together, because it looks like a safety net
and is not one:

1. ADR-0014 certifies only **administrative τ-divergence** — a repeating *projected* state with
   every step in the cycle administrative. A recursion that emits facts is realizing, so no
   certificate applies.
2. Divergence detection is lasso detection on `ObservableState = (world, policy)`. The L3 world
   contains a strictly-appending fact chain and a monotonically incrementing count, so **the world
   identity is fresh at every step and a lasso can never close.** In a monotone fact-accumulating
   model, divergence is structurally uncertifiable.

Either way a non-terminating run reports budget exhaustion — `Unknown`, which establishes nothing
(ADR-0014 §5). Calling that "certified divergence" would be a false record.

### The original ⟨D-MEASURE⟩ design, retained for v3

Retained rather than deleted, because v3 needs it and rediscovering it would be waste. ADR-0012
§3.3 is explicit that v1's termination comes from the pending suffix shrinking — a degenerate
well-founded measure. A profile whose rules can commit repeatedly keeps the shape of that argument
and enriches the measure: the measure lives **in the world**, hence in the journal, hence is
independently re-derivable by a checker, with each generator declaring whether it strictly
decreases the measure or advances it and the adapter checking the decrease per step, in O(1),
before committing. A violation is `Unknown`, never a commit.

Stratified evaluation survives as an *implementation technique* within a measure level, but it is
not the top-level model: stratification buys nothing about termination once arithmetic can create
values.

**Stated honestly for that future profile:** termination would not be a static property. It would
be a per-run, per-step, checkable one. For v2 it *is* static, which is the whole gain of keeping
the fragment this narrow.

**Free recursion relying on ADR-0014's divergence certificates is rejected, and both reviews
killed it for complementary reasons that are worth recording together, because it looks like a
safety net and is not one:**

1. ADR-0014 certifies only **administrative τ-divergence** — a repeating *projected* state with
   every step in the cycle administrative. A recursion that emits facts is realizing, not τ, so no
   certificate applies.
2. Divergence detection is lasso detection on `ObservableState = (world, policy)`. The L3 world
   contains a strictly-appending fact chain and a monotonically incrementing count, so **the world
   identity is fresh at every step and a lasso can never close.** In a monotone fact-accumulating
   model, divergence is structurally uncertifiable.

Either way a non-terminating run reports budget exhaustion — `Unknown`, which establishes nothing
(ADR-0014 §5). Calling that "certified divergence" would be a false record.

So termination is *decided* rather than assumed, by generalizing what v1 already does. ADR-0012
§3.3 is explicit that v1's termination comes from the pending suffix shrinking — a degenerate
well-founded measure. v2 keeps the shape of that argument and enriches the measure. The measure
lives **in the world**, hence in the journal, hence is independently re-derivable by a checker —
which is the property this whole substrate is organized around, and the cheapest checkable
termination artifact available.

Stratified evaluation survives as an *implementation technique* within a measure level — inside a
fixed ordinal the closure is a monotone fixpoint over a finite fact set — but it is not the
top-level model, because stratification buys nothing about termination once arithmetic can create
values.

**Stated honestly:** termination of a v2 program is not a static property. It is a per-run,
per-step, checkable one. An ADR that promised otherwise would be promising a decision procedure it
does not have.

## 5. Decision — the four remaining rejections

**Field access — admitted.** The nominal record and field ordinal resolve statically; runtime
projection is total on a well-typed canonical record. Never resolved by map iteration or textual
coincidence.

**`match` — admitted, exhaustive only.** Compiled to a versioned decision tree preserving source
arm order. Non-exhaustive matches, duplicate bindings, wrong constructor arity, and a default arm
silently swallowing malformed input are all rejected. `proving exhaustive` may request stronger
coverage evidence, but that evidence grades **coverage** and never upgrades the settlement
observation — ADR-0012 already separates the two.

**Payload-bearing constructors — admitted**, at exact nominal constructor, arity and payload
types. Note this needs recursive configs, which have since landed (μ-types), and generic configs,
which v2 must therefore also admit — v1 rejects `List<T>` as `UnknownTypeName`.

**Arithmetic and comparison — admitted, partially, and the grade is the interesting part.**
Checked `i64` addition, subtraction and multiplication with left-to-right operand evaluation, no
wrapping, no saturation, canonical result encoding, and overflow as `Unknown(EvaluationFault)` —
never quiescence and never refutation. Division is deferred until quotient type, rounding,
division-by-zero and `MIN / -1` are separately pinned.

### ⟨D-ARITHGRADE⟩ a run may not claim more than its weakest authority

> A settlement whose evaluation used arithmetic or comparison SHALL NOT be rendered above
> `Derived` on the strength of the quiescence certificate alone.

The certificate grades **frontier emptiness**, not numeric correctness. `g_arith` and `g_cmp` are
undischarged (ADR-0015; the arithmetic bridges are blocked on ADR-0025), so a typing derivation
using them caps at `@Audited` — and a typing discharge would establish typing only, never
evaluation (⟨D-JUDGE⟩). A journal audit may reach `Audited` only if an independent v2 evaluator
rechecks operands, operation, overflow behaviour and result. `Proven` value arithmetic needs the
kernel value relation ADR-0015 §6 describes; a typing proof cannot be reused for it.

This is the clause most likely to be quietly violated by a rendering change, so it is normative
rather than advisory.

## 6. Decision — ⟨D-STATE⟩ state is successive configurations

> A state transition SHALL emit a new content-addressed revision. "Current state" is a view of the
> latest revision, never a mutable cell outside the world identity.

**The reviews disagreed, and this one is a deliberate deferral rather than a resolution.** One
proposed a `StateRevision` chain — predecessor, event, state, ordinal — extending the existing
world model additively. The other argued the world should become a **map of facts**, with mutation
as remove-plus-add, citing `soc-core/src/delta.rs`'s own doc: *"editing a configuration is modelled
as removing its old handle and adding the new one — there is no third 'modified' case."* On that
reading v1's monolithic world is the outlier and the delta protocol is currently inert.

The second is very likely right as the eventual model, and it has a consequence the first cannot
offer: with a mutable-map world, **world identities can recur**, which makes ADR-0014's divergence
certificates reachable and gives "this duel is a draw by infinite loop" a checkable artifact. That
is a genuinely attractive target.

v2 takes the first anyway, for one reason: it needs no `soc-core` change, and v2 is already
forfeiting ⟨D-TAUZERO⟩ and introducing an evaluator. Changing the world's granularity in the same
step would make the differential suite prove two things at once. **The fact-map world is named
here as the expected successor** so it is not rediscovered, and §9 records what it would unlock.

External choices, shuffled decks and randomness enter as canonical initial inputs or a seeded
event stream. Host RNG, wall clock, input arrival order and hidden process state can never
influence a content-addressed run.

## 7. What v2 deliberately does not do

**Rule schemas — deferred to v3.** Note this is narrower than it first reads, after §3's erratum:
a v2 rule *does* take parameters, and each names one rule and binds one fact. What is deferred is
a parameter that **ranges over values** — a schema. Such a parameter supplies no quantification
domain, and the AST has none to offer. Admitting them now forces an arbitrary choice among "all values
of the type", "all initial facts", "all currently committed facts", or an incrementally expanding
Herbrand universe — choices with different semantics and different termination properties, several
of them infinite once arithmetic can create values. A schema profile must require an explicit
finite domain or a function-free, range-restricted grounding discipline, and if grounding cannot
stay unbounded and total it needs `QuiescenceCertificateV2` under ⟨D-CAND⟩.

**Consequence, stated rather than buried: v2 can express a finite, closed simulation, and not yet
a reusable open-ended engine.** That belongs in the acceptance criteria, not in a footnote.

**Three things a duel engine wants that are not expressible under ratified decisions**, scoped out
deliberately rather than discovered in Stage C:

- **Hidden information.** The world is a single global content-addressed configuration with no
  per-agent visibility. Put a hidden hand in it and a certificate for "this player had no legal
  play" was computed against full information — a *wrong* certificate, not merely a limited one.
  Per-player epistemic views are a new structural layer.
- **Randomness.** `F_O` is ratified partial-deterministic (ADR-0002 §8.2, §11); a subprobabilistic
  signature requires a new behavior-signature version tag and never a silent widening. A shuffled
  deck must be a predetermined order fixed in the initial world. Simulating over a distribution
  means N independent runs.
- **Interactive play.** Commitment is singular; a player choice is several admissible candidates
  plus a keyer that picks one. A deterministic keyer gives a fully replayable duel — an AI *is* a
  keyer — but an interactive human keyer is not a pure function, so replay identity dies unless
  every choice is materialized as an input fact.

The honest statement of the target is therefore: **SOC as ratified can adjudicate and replay a
duel; it cannot interactively play one without materializing every choice.** That is the correct
scope, and it is still a strong one.

## 8. Determinism and identity

Two runs of the same plan and initial artifacts must produce identical plan, fact, step, journal,
run and certificate bytes. v2 must freeze: expression and evaluator-instruction ordinals; rule
dependency order; the administrative-versus-realizing generator partition; left-to-right
evaluation; source-order rule priority; declaration-order record fields and variants; source-order
match arms; fact dependency order; arithmetic semantics and fault ordering; state-revision and
event encoding; and first-error selection where several errors are statically present.

Named nondeterminism risks: `HashMap`/`HashSet` iteration; unstable topological sort; raw interner
handles; concurrent evaluation or journal append; unordered fact lookup with duplicate producers;
optimizer-dependent constant folding; host overflow or division behaviour; locale or Unicode
handling; match-arm reordering; record-literal source order versus declaration order; and budgets,
cost measurements or diagnostic text leaking into semantic identity.

Naive and incremental v2 evaluators must be checked with **symmetric weak bisimulation**, not
directional refinement — ADR-0014 requires symmetric parity for naive versus incremental
execution.

## 9. Hard boundaries

A v2 settlement SHALL NEVER:

1. claim a grade above its weakest participating authority (⟨D-ARITHGRADE⟩);
2. present budget exhaustion as divergence, or divergence as refutation;
3. mint a quiescence certificate over an incomplete enumeration;
4. treat a typing discharge as an evaluation fact (⟨D-JUDGE⟩);
5. accept a formerly rejected v1 form under the v1 marker (§2);
6. let a runtime evaluation fault appear as an empty frontier — it is
   `Unknown(EvaluationFault)` through a distinguished trap path;
7. wrap, saturate, or silently truncate an arithmetic result;
8. derive its expected program, context, or policy from the plan it is checking;
9. consult history where the profile promises to consult only `(world, policy)` — ADR-0014's P1.
   Concretely for a duel: "once per turn", "if this was destroyed this turn", "if you did not
   Normal Summon this turn" **must be facts in the world**, asserted when they happen and cleared
   by an end-phase rule, never journal queries. This is the boundary most likely to be violated by
   someone implementing a single card, and the failure is silent: certificates keep minting and
   are all unsound.

## 10. Staged implementation

No public `@2` run is emitted until the whole chosen fragment lands.

- **Stage A — profile, IR and identities.** `L3PlanV2`/`ProgramIdV2`, the expression IR, dependency
  encoding, value/fact/world encoders, the observation-profile preimage with its non-empty `𝒢_τ`,
  and frozen vectors under the two-consumer discipline. v1 tests byte-identical.
- **Stage B — the evaluator.** Field access, `match`, payload constructors, checked arithmetic and
  comparison, as a deterministic small-step machine whose steps are administrative generators.
- **Stage C — derivation.** Static dependency extraction, acyclicity, and eligibility on committed
  dependencies. **First stage where a genuinely derived fact is reachable.**
- **Stage D — the measure.** The progress ordinal in the world, per-generator decrease
  declarations, and the O(1) per-step check.
- **Stage E — state revisions** (§6) and the generic/parameterized config vocabulary v1 rejects.
- **Stage F — audit and CLI.** v2 audit semantics, `brix run`/`brix audit` over the v2 profile, and
  the differential suite against v1.
- **Stage G — adversarial gates.** Non-termination, evaluation faults, overflow, dependency cycles,
  determinism and reproducibility. Release gates, not deferred polish.

## 11. Open decisions

- Whether the fact-map world (§6) lands as v2.1 or waits for v3 alongside schemas. It is the
  change that makes divergence certificates reachable, so it should not wait indefinitely.
- Whether `Regime::candidates` can stay total once schemas arrive (§7). If not, v3 pays the
  ⟨D-CAND⟩ price v2 avoids.
- The exact profile marker string, to be frozen by this ADR before implementation begins.
