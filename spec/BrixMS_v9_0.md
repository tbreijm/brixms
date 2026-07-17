# BrixMS Language Specification v9.0

**The Living Model Edition — complete, unified with the execution line**
**Status:** Completed candidate normative specification for the v9 language and professional platform
**Date:** 16 July 2026
**Supersedes:** the Living Model line (v5.3 → Living Model v6.0) and the execution line (v6.0 → v8.1); all earlier specifications

---

## Abstract

Software is a simulation model of reality. BrixMS v6 takes that thesis from language
semantics through the complete application and operational lifecycle.

A BrixMS program is a typed, versioned reactive hypergraph that models a fragment of the
world: what exists, what is observed, what follows, what might happen, what is suggested,
what is authorized, what is presented to people, and how actions cross into external
systems. Every committed change settles deterministically to a fixpoint. Production is a
coupled simulation whose boundary facts come from real systems and people; development is
the same model coupled to scenarios and simulated Drivers.

The paradigm in one breath:

> **Everything durable, observable, explainable, and interactively meaningful is a
> relation. Computation is settlement. Time is versioning. Intelligence is typed by its
> epistemic status. Components are context-independent semantic bricks. The system always
> maintains a model of itself.**

v6.0 retains the v5 semantic kernel and completes the surrounding language. It unifies
formal logic, rigorous mathematics, learned policies, and native language-model boundaries;
makes the reflexive self-model mandatory; adds observation, reconciliation, causality,
decision intelligence, workflow, and model-health semantics; standardizes semantic bricks
that contain both domain and interaction models; and defines a reactive client hypergraph
with React and other renderer adapters. It also closes the professional platform contract
for APIs, security, privacy, deployment, recovery, registries, certification, and domain
libraries. The completed design adds model-validity envelopes, correction and retroactive
truth semantics, canonical graph interchange, explicit consistency and failure contracts,
reproducibility tiers, a complete everyday standard library, executable conformance
artifacts, a threat model, and the professional developer feedback loop. It also makes
data science and machine learning native projections of the living model: cleaning,
features, datasets, experiments, training, predictions, evaluation, and monitoring all
retain snapshot identity, valid time, provenance, authority, and simulation semantics.

The result is not a frontend framework connected to a backend service, nor an AI layer
attached to a database. It is one composable executable model of reality, distributed
across authoritative and client projections, with every inference, interaction, action,
and change retaining identity, evidence, authority, and history.

## Changes for v9.0 (unification of the two lines)

Two specification lines matured in parallel from v5.3: the **Living Model line**
(ecosystem, formalisms, epistemic types, reflexive self-model, reality and decisions,
semantic bricks, reactive clients, professional platform, native data science) and the
**execution line** (compilation model, runtime gap closure, language-model boundary
with frame governance, production contract, real-time transport). v9.0 merges them on
the Living Model base. No semantics change in the merge; where the lines overlapped,
the stricter formulation won:

1. **Part XXVIII — Compilation model and runtime closure**: the two-pass architecture
   (`brixc` → Rust → `rustc`), two execution tiers reconciling AOT with staged
   activation, the oracle, `extern rust`, persistence/recovery, and two normative
   amendments — the logical/physical budget split into §24.5 (logical budgets are
   deterministic semantics; physical pressure acts at admission, never mid-settlement)
   and authorization-blind settlement with `Opaque` provenance truncation into §24.2.
2. **§19.8 — Frame governance**: context-ablation sensitivity as a release-gate
   metric; separation of context authorship from consumption authority
   (`LLM.ContextCuration`); the curation residual stated honestly.
3. **§24.9 — Real-time transport**: the `brix.websocket` contract under one
   `export api` declaration; Rebase-never-skip resume; declaration-driven coalescing;
   deterministic `sim.websocket`.
4. **Conformance I.23–I.25** (tier equivalence, backend and build determinism;
   real-time transport; frame governance) and sealed schemas A.18.
5. **Appendix K — Implementation gates**: the completion checklist as normative
   freeze criteria; v9.0 is feature-complete only when every gate has a testable
   contract, implementable by a two-person kernel team against the oracle.

## Changes from v5.3

v6.0 changes no least-fixed-point settlement principle. It is a major version because it
standardizes the complete language surface and ecosystem contracts that surround that
kernel.

1. **Unified reasoning architecture** (Part XIX): native logic, exact and approximate
   mathematics, probabilistic reasoning, optimization, learned policies, and language
   models share one provenance model while retaining distinct epistemic result types.
2. **Formal logic strengthened** (Part XIX): finite first-order syntax, proof objects,
   refinement types, temporal contracts, paraconsistent profiles, abduction, and
   solver-backed obligations.
3. **Mathematics strengthened** (Part XIX): `Rational`, deterministic floating-point,
   intervals, units, currencies, linear algebra, statistics, probability, numerics,
   symbolic expressions, automatic differentiation, optimization, and graph mathematics.
4. **Native language-model boundaries** (Part XIX): typed language tasks, grounded
   evidence, structured outputs, bounded tools, immutable prompts and models, evaluation,
   replay, and staged generated-code activation.
5. **Mandatory reflexive self-model** (Part XX): every program revision publishes a
   typed immutable semantic model of its declarations, dependencies, phases, boundaries,
   tests, documentation, ownership, quality, and operational evidence.
6. **Reality-observation lifecycle** (Part XXI): observations, candidate claims,
   accepted claims, identity resolution, reconciliation, data contracts, and explicit
   completeness and freshness.
7. **Causal and decision intelligence** (Part XXI): structural causal models,
   interventions, counterfactuals, alternatives, objectives, risk, authorization, and
   complete decision records.
8. **Human work and durable workflows** (Part XXI): tasks, approvals, deadlines,
   escalation, compensation, and process history lower to relations, timers,
   transactions, and protocols.
9. **Semantic bricks** (Part XXII): the primary unit of reuse may contain domain models,
   rules, simulations, workflows, queries, commands, views, forms, and interaction state.
   UI and domain are projections of the same model rather than separate component kinds.
10. **Domain library ecosystem** (Part XXII): context-independent bricks compose through
    typed ports, explicit adapters, ontology alignments, local state ownership, semantic
    versioning, and registry metadata.
11. **Reactive client hypergraph** (Part XXIII): authorized server projections settle in
    a client-side graph and drive a semantic render graph. React, Vue, Svelte, Web
    Components, native, terminal, and conversational renderers are adapters.
12. **Unified live APIs** (Part XXIV): HTTP, WebSocket, SSE, and typed RPC are generated
    from the same queries, commands, and watches with snapshot, revision, authorization,
    resume, and backpressure semantics.
13. **Professional platform contract** (Parts XXIV–XXV): identity, privacy, secrets,
    budgets, observability, deployment artifacts, canary activation, rollback, recovery,
    supply-chain evidence, registry, certification, Model Studio, Brix Control, and Brix
    Lab.
14. **Complete conformance profile** (Appendix I): adds reasoning-status, reflection,
    observation, brick composition, client settlement, rendering, synchronization,
    security, privacy, and release-lifecycle categories.
15. **Language completion contracts** (Part XXVI): model-validity envelopes, bitemporal
    correction, canonical interchange, full standard-library scope, consistency profiles,
    unknown-outcome and cancellation semantics, reproducibility classes, executable formal
    artifacts, professional diagnostics, and explicit trust and threat profiles.
16. **Native data science and machine learning** (Part XXVII): relation frames,
    typed missingness, immutable cleaning recipes, feature-time semantics, snapshot-bound
    datasets, leakage-safe resampling, statistical formulas, estimator workflows, experiment
    tracking, model registry, declarative visualization, drift monitoring, and premium
    Arrow-based Python, R, and ONNX interoperability. Analytical and simulated datasets are
    projections of the same living model rather than an exported second truth.

## Changes from v5.2

v5.3 changes no relation-settlement kernel semantics. It standardizes the first-party
engineering ecosystem and admits ontology and simulation-formalism packages whose
lowerings are defined in terms of the existing kernel.

1. **Complete engineering distribution** (Part XIII): compiler, runtime, REPL,
   simulation runner, package manager, LSP/DAP, test runner, quality engine, provenance
   tools, Wasm Driver host, observability, storage backends, and data interchange.
2. **Rigorous native testing** (Part VIII §§5–7): example, property, scenario, contract,
   migration, statistical, mutation, fuzz, schedule-perturbation, backend-parity, and
   incremental-versus-full-recomputation testing.
3. **Built-in quality governance** (Part VIII §§8–10): graph-native maintainability,
   architecture, capability, boundary, performance, documentation, test-strength, and
   supply-chain metrics with signed activation gates.
4. **First-class ontology layer** (Part XIV): concepts, properties, alignments, shapes,
   model-closed versus open-world reasoning, Horn/RDFS/OWL-RL profiles, RDF/SHACL/SKOS
   interoperability, and explicit inconsistency structure.
5. **Formalism contract** (Part XIV): a formalism defines vocabulary, lowering,
   well-formedness, analysis, visualization, conformance, and composition over the same
   BrixMS kernel.
6. **Native discrete-event simulation** (Part XV): superdense time, future-event
   scheduling, simultaneous-event semantics, cancellation, deterministic random streams,
   replications, event traces, and DEVS mappings.
7. **System dynamics** (Part XVI): typed stocks and flows, dimensional analysis,
   deterministic numerical-integration profiles, delays, algebraic loops, threshold
   events, calibration, sensitivity, and equilibrium analysis.
8. **Agent-based modeling** (Part XVII): agents as graph entities rather than actors,
   explicit perception and activation, propose/arbitrate/commit cycles, neighborhoods,
   communication events, adaptive policies, and micro-to-macro analysis.
9. **Hybrid multimethod simulation** (Part XVIII): settled synchronization among DES,
   system dynamics, ABM, deduction, solvers, and learned policies.
10. **Conformance expanded** (Appendix I): testing, quality, ontology entailment, DES,
    numerical integration, ABM scheduling, and hybrid synchronization categories.

## Changes from v5.1

v5.2 adds one core-language construct and changes no kernel semantics.

1. **`policy` — the learned-policy boundary** (Part XII): a typed, versioned mechanism
   for producing suggestions under uncertainty and learning from settled outcomes. It
   lowers entirely to existing kernel objects — protocols, sealed relations, immutable
   versions, staged activation — and therefore enters as core language under Part II §2.
   No learning algorithm enters the language: inference and training are Drivers;
   bandits, evaluators, and reward shaping are `brix.learn`.
2. **Defining semantic rule added to Part 0 §0.6:** policies suggest under uncertainty;
   rules deduce; transactions authorize; boundaries act; outcomes teach the next
   immutable policy version.
3. **Corrections to the incoming proposal, normative here:** feedback is *deduction*
   (a derived relation over settled outcomes), not an event; snapshot identity is
   stamped by the engine at decision time, never read by a rule during settlement;
   `authority` compiles to generated constraints on the rules that consume suggestions,
   not to a separate enforcement mechanism; candidate rows require `S: Canonical`;
   active-learning `observe` blocks are deferred to `brix.learn`.
4. **Flagship extended** (Part XII §8): the simulated planner becomes a learned policy
   with advisory authority, exercising decisions, feedback, propensity, shadow
   evaluation, and staged version activation in the same scenario.
5. **Housekeeping:** `brix.learn` added to Part IX; policy lifecycle schemas added to
   Appendix A; Edition 6 (Learning) added to Part XI; conformance categories 13–14
   added to Appendix I.

## Changes from v5.0

1. **Flagship repaired** (Part I): all referenced entities are created; `ensure` is
   reserved for entities and `assert` for relations; `riskModel` is defined;
   assignment is protocol-mediated, demonstrating the deduction/external-choice split;
   tariffs are keyed by vehicle class; rates and weights carry dimensional types.
2. **Derived-key conflicts specified** (Part III §11): sealed `KeyConflict` edges, no
   silent winner, per-kind rules for ground, state, event, and derived relations;
   `resolve` merge policies deferred to Edition 2.
3. **`ClaimRef<R>` surfaced** (Part III §3, Part VII §2): `assert` returns an opaque,
   retry-stable claim reference; `retract` consumes one.
4. **Mask phase inference completed** (Part III §6): a normative dependency rule places
   every ordinary live read of a maskable relation strictly above all of its mask
   producers; `mask` binds edge references, not patterns; `history` reads bypass.
5. **Closed world renamed model-closed** (Part III §7): local negation is sound about
   the model; claims about reality require a boundary completeness contract.
6. **Protocol requests versioned** (Part VII §3): RequestKey / RequestVersion /
   AttemptKey identity, supersession lifecycle, satisfaction policy.
7. **Scenario adapters made explicit** (Part VI §2): `sim.capture`, `sim.succeed`,
   `sim.script`, `sim.replay` replace the underspecified `sim.recorder`.
8. **Error edges carry site identity** (Part III §9); `?` semantics fully defined.
9. **Numerical determinism made kernel semantics** (Part V §8): strict IEEE-754 inside
   settlement, canonical reduction order for stock aggregates, parallel execution must
   reproduce sequential results; `Rel<S>` is hashable only when `S: Canonical`.
10. **Aggregates are compiler-visible** (Part IV §4): the `aggregate fn` form carries a
    complete-read obligation; calling one from a rule creates a strict phase dependency.
11. **Path expressions name incidence roles** (Part IV §3): `path R(from -> to)+`.
12. **Reflexive governance bounded** (Part VIII §3): rules may reason about and propose
    changes to program descriptors; executable rule membership changes only through
    staged program activation, effective no earlier than the next ProgramRevision.
13. **Architecture layered** (Part II §2): semantic kernel, core language, standard
    library, toolchain contract — with separate admission rules, replacing the single
    "kernel" bucket.
14. **Normative appendices added**: grammar (D), static semantics (E), phase inference
    (F), canonical encoding (G), protocol lifecycle (H), conformance suite (I).
15. **Wording corrected throughout**: production is coupled, not predetermined; the M6
    milestone is the first reflexive-tooling milestone, not self-hosting; v4 is design
    input that each edition must restate and ratify, not a normative dependency.

---

# Part 0 — Philosophy

## 0.1 Software is a simulation model of reality

Every useful program models something: orders moving through a logistics network, money
moving through accounts, documents moving through an approval chain. Conventional
languages bury the model under execution machinery — objects, threads, queues,
callbacks — until the model is no longer visible in the code and can only be recovered
by archaeology.

BrixMS inverts this. The model is the program. The programmer declares:

1. **What exists** — entities and the relations between them (structure);
2. **What follows** — rules that derive consequences whenever structure changes
   (dynamics);
3. **What is asked** — reads over settled states (observation);
4. **Where the model meets the world** — boundaries that carry facts in and effects out
   (coupling).

Execution is the engine maintaining the model: every committed change to ground
structure is settled to a fixpoint of the rules, producing the next observable state of
the modeled world. This is the detect–execute loop of the original Brix formalism, made
deterministic, incremental, and typed.

## 0.2 Development is simulation; production is coupled simulation

Because the program is a model, the difference between testing and running is only where
boundary facts come from:

- In **simulation**, boundary facts come from scenarios: scripted transactions,
  generated event streams, a simulated clock the program advances itself.
- In **production**, boundary facts come from Drivers coupled to real systems: real
  clocks, real networks, real users.

The program text is identical, and the settlement of any given committed history is
identical and deterministic. Production as a whole is not deterministic — real boundary
outcomes are not predetermined — but every committed production history is exactly as
replayable, forkable, and explainable as a scenario run, because it is one.

## 0.3 Time is versioning

A model of reality must model change without destroying its own past. BrixMS never
mutates in place. The world advances as a sequence of committed **revisions**; each
revision settles deterministically; every observation names the revision it observed.
The past is not a log bolted onto the state — the state is a position in the log.

Three times are distinguished and never conflated: **transaction time** (when the model
learned something), **valid time** (when it holds in the modeled world), and
**simulation time** (the model's own clock, coupled to the real one in production).

## 0.4 The system can read itself

A model that cannot inspect itself cannot govern itself. The program's schema, rules,
derivations, provenance, and performance data are sealed relations in the same graph,
queryable in the same language. The debugger is a query. The profiler is a query. "Why
does this edge exist" is a query. Governance is written as rules over the relations that
describe the rules — within the staged-activation bound of Part VIII §3: the model may
reason about its own program, and may change it only between revisions, never during
one.

## 0.5 Effective, not complete

Three limits constrain any model of reality: observation disturbs (Heisenberg), a system
cannot certify itself from within (Gödel), and exhaustive search is unaffordable (P vs
NP). The design consequence, carried over from the original formalism: **the model does
not need to be complete, it needs to be effective.** v5 chooses pragmatic defaults —
model-closed negation over your own relations, typed error edges over totality proofs,
strict reproducible numerics over exactness theatre — and reserves the heavy machinery
for the boundaries where trust actually breaks. The same limit disciplines the
vocabulary: absence in the model is evidence about the model, and becomes evidence about
reality only where a boundary contract says the model observes reality completely.

## 0.6 Deduction, choice, suggestion

A model of reality contains three kinds of movement, and the language keeps them
separate on purpose. Rules **deduce**: what follows necessarily from settled structure.
Boundaries **choose and act**: what the world decides and does, entering as ground
facts. Policies **suggest**: what a learned, versioned, fallible component believes
under uncertainty — recorded, scored, and inert until a rule with explicit authority
consumes it. The defining semantic rule of the learned boundary (Part XII):

> **Policies suggest under uncertainty. Rules deduce. Transactions authorize.
> Boundaries act. Outcomes teach the next immutable policy version.**

Nothing learned ever mutates the graph directly, and nothing learned ever changes what
deduction means.

---

# Part I — The flagship program

Everything in the semantic kernel and core language is justified by this program and its
scenario. A construct they do not force lives in the standard library, the toolchain, or
an edition. The program models a small vehicle-logistics world: orders arrive,
assignment is requested from an external planner, prices are computed and sometimes
manually overridden, late-delivery risk is watched and escalated, and the model runs
identically as a simulation and as a live service.

The program is complete: every referenced name is defined here or imported, every
claimed kernel feature is exercised, and the scenario drives every rule at least once.

```brix
package demo.logistics @ 1.1.0
module World

use brix.math.{Decimal, clamp}
use brix.math.units.{Mass, kg, Kilometre, km}
use brix.time.{Instant, Duration, hours}
use brix.sim

// ------------------------------------------------------------------
// 1. Structure — what exists
// ------------------------------------------------------------------

entity Location { key code: String }
entity Client   { key code: String; tier: Tier }
entity Vehicle  { key plate: String; class: VehicleClass; capacity: Quantity<Mass> }
entity Tariff   { key class: VehicleClass }        // keyed by class: one tariff per class

enum Tier         { Standard; Key }
enum VehicleClass { Compact; Standard; SUV }
enum Status       { Open; Delivered; Cancelled }

entity Order {
  key ref: String
  client: Client
  from: Location
  to: Location
  weight: Quantity<Mass>
  due: Instant
}

// ground n-ary relation: asserted by transactions or Drivers
rel Distance { from: Location; to: Location; length: Quantity<Kilometre> } key(from, to)

// state relations: at most one live version per key; `set` supersedes
state rel OrderStatus { order: Order; value: Status }                    key(order)
state rel TariffRate  { tariff: Tariff; rate: Money<EUR> / Kilometre }   key(tariff)
state rel ManualPrice { order: Order; amount: Money<EUR> }               key(order)

// event relation: persistent occurrences with identity and time
event rel Delivered { key id: EventId; order: Order; at: Instant } time(at)

// ------------------------------------------------------------------
// 2. Coupling declared early: assignment is an external choice,
//    not a deduction. The model requests; a planner decides.
// ------------------------------------------------------------------

protocol AssignOrder {
  request { order: Order; weight: Quantity<Mass> } key(order)
  outcome Chosen     { vehicle: Vehicle }
  outcome NoCapacity { }
}

protocol NotifyOps {
  // keyed by order: a changed risk supersedes the prior request version (Part VII §3)
  request { order: Order; risk: Probability } key(order)
  outcome Sent   { at: Instant }
  outcome Failed { reason: String; retryable: Bool }
  policy { retry { maxAttempts: 3, backoff: Exponential(base: 1 s, max: 30 s) } }
}

// ------------------------------------------------------------------
// 3. Dynamics — what follows
// ------------------------------------------------------------------

fn surcharge(weight: Quantity<Mass>) -> Money<EUR> =
  if weight > 3500 kg then 150 EUR else 0 EUR

partial fn riskModel(due: Instant, now: Instant)
  -> Result<Probability, ValidationError> {
  let remaining = due - now
  if remaining <= 0 hours { Probability.try(1.0) }
  else { Probability.try(clamp(1.0 - remaining / 24 hours, 0.0, 1.0)) }
}

// Unassigned: model-closed negation over relations this namespace owns.
rel Unassigned { order: Order } key(order)
derive Waiting: Unassigned(order: o) from {
  OrderStatus(order: o, value: Open)
  without { Move(order: o) }
  without { Delivered(order: o) }
}

// The model asks the world to assign; the world answers as ground outcomes;
// deduction resumes from the answer.
derive RequestAssignment: AssignOrder.request(order: o, weight: w) from {
  Unassigned(order: o)
  o: Order { weight: w }
}

rel Move { order: Order; vehicle: Vehicle } key(order)
derive Assign: Move(order: o, vehicle: v) from {
  AssignOrder.Chosen(order: o, vehicle: v)
}

// Pricing: pure deduction over assignments, tariffs, and distances.
rel ComputedPrice { order: Order; amount: Money<EUR> } key(order)
derive PriceOrder: ComputedPrice(order: o, amount) from {
  Move(order: o, vehicle: v)
  v: Vehicle { class: cls }
  t: Tariff { class: cls }
  TariffRate(tariff: t, rate)
  o: Order { from: a, to: b, weight: w }
  Distance(from: a, to: b, length)
  let amount = rate * length + surcharge(w)
}

// Override: `mask` binds edge references — the target and the reason are
// both concrete edges, which is what provenance records (Part III §6).
derive Override: mask(price) by manual from {
  price  @ ComputedPrice(order: o)
  manual @ ManualPrice(order: o)
}

rel EffectivePrice { order: Order; amount: Money<EUR> } key(order)
derive FromManual:   EffectivePrice(order: o, amount) from { ManualPrice(order: o, amount) }
derive FromComputed: EffectivePrice(order: o, amount) from { ComputedPrice(order: o, amount) }
// FromComputed reads ComputedPrice live; because Override can mask it, the
// compiler places FromComputed strictly above Override (Part III §6). The
// two EffectivePrice rules therefore never conflict on a key.

// Risk: a partial function inside a rule; a failing match derives a sealed
// RuleError edge for that site and contributes nothing else (Part III §9).
rel LateRisk { order: Order; risk: Probability } key(order)
derive Risk: LateRisk(order: o, risk) from {
  Unassigned(order: o)
  o: Order { due }
  sim.Now(at: now)
  let risk = riskModel(due, now)?
}

derive Escalate: NotifyOps.request(order: o, risk) from {
  LateRisk(order: o, risk)
  when risk > 0.8
}

constraint Capacity strict {
  Move(order: o, vehicle: v)
  o: Order { weight: w }
  v: Vehicle { capacity: cap }
  when w > cap
}

// ------------------------------------------------------------------
// 4. Observation — what is asked
// ------------------------------------------------------------------

query KeyClientsAtRisk(threshold: Probability)
  -> Rel<{ order: Order; client: Client; risk: Probability }> =
  from {
    LateRisk(order: o, risk)
    o: Order { client: c }
    c: Client { tier: Key }
    when risk > threshold
  }
  yield { order: o, client: c, risk }

// ------------------------------------------------------------------
// 5. Production coupling — Drivers for the two protocols
// ------------------------------------------------------------------

driver PlannerClient for AssignOrder needs Net<"planner.internal"> {
  on request(req, cancel) {
    let resp = http.post("https://planner.internal/assign", encode(req))?
    match decodeAssignment(resp.body) {
      Ok(Some(v)) => succeed Chosen { vehicle: v }
      Ok(None)    => succeed NoCapacity { }
      Err(e)      => fail { error: e, retryable: false }
    }
  }
}

driver OpsNotifier for NotifyOps needs Net<"ops.internal"> {
  on request(req, cancel) {
    let resp = http.post("https://ops.internal/alert", encode(req))?
    if resp.ok { succeed Sent { at: clock.now() } }
    else       { fail Failed { reason: resp.status.text,
                               retryable: resp.status.retryable } }
  }
}

// ------------------------------------------------------------------
// 6. The same model as a simulation
// ------------------------------------------------------------------

scenario RushWeek {
  seed 42
  bind sim.Clock(resolution: 1 hours)

  // Explicit adapters, explicit lifecycle meaning (Part VI §2):
  bind AssignOrder to sim.script {
    when req.weight <= 2000 kg => Chosen { vehicle: Vehicle("V-01") }
    when req.weight <= 3500 kg => Chosen { vehicle: Vehicle("V-02") }
    otherwise                  => NoCapacity { }
  }
  bind NotifyOps to sim.capture     // requests recorded, left unresolved

  setup {
    let ams = ensure Location { code: "AMS" }
    let rtm = ensure Location { code: "RTM" }
    ensure Client { code: "ACME", tier: Key }
    ensure Vehicle { plate: "V-01", class: Standard, capacity: 2000 kg }
    ensure Vehicle { plate: "V-02", class: SUV,      capacity: 3500 kg }
    let std = ensure Tariff { class: Standard }
    let suv = ensure Tariff { class: SUV }
    set TariffRate(tariff: std, rate: 1.20 EUR/km)
    set TariffRate(tariff: suv, rate: 1.65 EUR/km)
    assert Distance(from: ams, to: rtm, length: 78 km)
  }

  step every 1 hours for 168 hours {
    let o = ensure Order { ref: sim.name("ord"), client: Client("ACME"),
                           from: Location("AMS"), to: Location("RTM"),
                           weight: sim.uniform(200 kg, 4000 kg),
                           due: sim.now() + 6 hours }
    set OrderStatus(order: o, value: Open)
  }

  at 24 hours {
    // exercise the event relation and delivery-aware negation
    assert Delivered { id: sim.eventId(), order: Order("ord-1"), at: sim.now() }
    set OrderStatus(order: Order("ord-1"), value: Delivered)
  }

  at 36 hours {
    set ManualPrice(order: Order("ord-2"), amount: 95 EUR)   // exercises the mask
  }

  assert always     { count(from { Violation() }) == 0 }
  assert eventually { count(from { AssignOrder.NoCapacity() }) > 0 }
  assert eventually { count(from { NotifyOps.request() }) > 0 }
  assert at end     { count(from { EffectivePrice() }) > 0 }
}
```

Run it:

```text
brix run world.brix                  # script profile, dev capabilities
brix sim RushWeek                    # deterministic: f(program, scenario, seed)
brix why 'EffectivePrice(order: Order("ord-2"))' --at end
brix serve world.brix --manifest prod.caps    # production coupling
```

The scenario is constructed so every kernel claim is exercised: model-closed negation
(`Unassigned`), the deduction/external-choice boundary (`AssignOrder`), masks and their
phase rule (`ManualPrice` at 36 hours), partial functions becoming error edges
(`riskModel` on a malformed probability would), request versioning (rising `risk`
superseding `NotifyOps` request versions), strict constraints that must hold at every
settled revision (`Capacity` — the scenario script respects capacity, so the `always`
assertion is a real check, not a tautology), state supersession, event relations, and
seeded reproducibility. Orders above 3500 kg receive `NoCapacity`, stay unassigned,
cross the 0.8 risk threshold as the clock advances, and generate escalations — which
`sim.capture` records without resolving, exactly as declared.

---

# Part II — Design contract

## 1. One-sentence definition

**BrixMS is a typed language in which a program is an executable model of a world:
durable state is a versioned typed hypergraph, dynamics are pure derivation rules
settled deterministically to a fixpoint at each revision, and the model couples to
reality only through explicit boundaries — with identical settlement semantics under
scenario-bound and production-bound boundaries.**

## 2. The four layers

v5.1 separates what v5.0 called "the kernel" into four layers with distinct admission
rules. Editions (Part XI) sit outside all four and may extend the semantic kernel.

| Layer | Contents | Admission rule |
|---|---|---|
| **Semantic kernel** | nodes, edges, revisions, settlement, phases, masks, supports, claims, key conflicts, error edges, transactions, protocol lifecycle, canonical identity | forced by the flagship's *meaning*; every element appears in the conformance suite (Appendix I) |
| **Core language** | entities, relations, rules, queries, constraints, scenarios, `policy` declarations (Part XII), the function/value language (ADTs, records, traits, effects, `Rel<S>`, units, path expressions, regions) | needed to *write* the flagship pleasantly; must lower to kernel semantics without extending them |
| **Standard library** | `brix.rel`, `brix.math`, `brix.logic`, `brix.io`, `brix.data`, `brix.time`, `brix.sim`, `brix.learn`, `brix.test`, `brix.meta` | ordinary packages; no private engine hooks except where sealed schemas say so |
| **Toolchain contract** | `brix` CLI, `brixd` LSP/DAP, formatter, profiler, WASM Driver host | observable behavior specified; implementation unconstrained |

Traits, row polymorphism, regions, path expressions, and linear algebra are core
language and library, not kernel machinery. The parser is not semantics. This table is
normative: a construct may not migrate inward without passing the inner layer's
admission rule.

## 3. The three surface categories

| Category | Contains |
|---|---|
| **Relations** | `entity`, `rel`, `state rel`, `event rel`, `open rel`, `derive` rules, `constraint`, `query`, local `Rel<S>` values |
| **Functions** | pure and effectful functions, values, types, traits |
| **Boundaries** | transactions, protocols, Drivers, capabilities, scenarios, policies (Part XII) |

An `entity` is a keyed unary relation whose tuples bear graph identity. A `derive` rule
is a named clause contributing to a derived relation. A `query` is a pure function from
an implicit settled snapshot to a relation value. One type system, one pattern language,
one evaluation story.

## 4. Non-negotiable properties

1. **Hypergraph-native structure.** Relations are first-class n-ary edges with named
   roles.
2. **Pure settlement.** Rules perform no I/O, ambient time reads, unseeded randomness,
   mutation, or blocking. Rules may call `partial` functions; a failed site derives a
   sealed typed error edge and that match contributes nothing else.
3. **Stable reads.** An observable revision is fully settled; no intra-settlement state
   is ever exposed.
4. **Declarative/operational agreement.** Incremental settlement equals whole-world
   recomputation, bit for bit, including error edges and conflicts.
5. **Stratified non-monotonicity.** Negation, absence, aggregation, and mask-affected
   reads read only completed lower phases; phases are inferred (Appendix F).
6. **Model-closed by default, open by declaration.** Locally owned relations are
   complete relative to the committed claims and derived supports of the named snapshot;
   negation over them is sound about the model. `open` relations require a completeness
   witness for absence-sensitive reads. Claims that model-absence equals real-world
   absence require a boundary completeness contract.
7. **No silent winner.** Conflicting tuples under one declared key never resolve by
   rule order, source order, hash order, or recency. Ground conflicts fail the
   transaction; derived conflicts become sealed `KeyConflict` edges and the key has no
   ordinary live value until one candidate remains.
8. **Constructive history.** Supersession, retraction, masking, key conflicts, and
   boundary outcomes are explicit operations with provenance; nothing is silently lost.
9. **Deterministic identity.** Rules derive only deterministically keyed structure;
   fresh identity belongs to transactions and is retry-stable.
10. **No ambient authority.** External effects require capabilities and occur only at
    boundaries.
11. **Reflexive substrate, staged self-modification.** Schema, rules, provenance, and
    profile data are sealed relations queryable in the language; executable rule
    membership changes only through staged program activation.
12. **Simulation equivalence.** Settlement of a committed history is identical under
    scenario-bound and production-bound boundaries.
13. **Implementation freedom without semantic freedom.** Plans, indexes, parallelism,
    and caches may change cost, never any observable value — including floating-point
    results (Part V §8).

## 5. Deliberate non-features

The semantic kernel and core language have no: classes or inheritance; null; unchecked
exceptions; source-order rule priority; implicit last-write-wins; actors, channels, or
effect handlers as semantics; spreadsheet metaphors; graph-level callbacks; defeat
calculus (an Edition 2 library over `mask`); `resolve` merge policies on derived keys
(Edition 2); lattice relations (Edition 2); information-flow labels and redaction
(Edition 3); distributed namespaces (Edition 4); machine-learning algorithms of any kind — inference and training are Drivers, algorithms are `brix.learn` or external, and only the interaction contract (Part XII) is language. Claim bookkeeping is engine-internal
except for the opaque `ClaimRef<R>` needed by retraction.

## 6. The ceremony budget

- hello world is one line in one file with zero manifest;
- the flagship is one self-contained file under 350 lines including its scenario;
- an ordinary program declares no phases, no indexes, no witnesses, and no identities
  beyond `key(...)`;
- `brix fmt` is canonical and non-configurable; `brix why` answers in graph terms;
- moving from `brix run` to `brix serve` changes zero lines of model code.

---

# Part III — Semantic kernel

## 1. Kernel objects

- **Node** — an identity-bearing typed atom.
- **Edge** — a typed n-ary connection with named roles, each role bound to a node or an
  immutable value.

An `entity` tuple is a node. A `rel` tuple is an edge. Nothing else is durable. The
engine represents revisions, rules, provenance, protocol lifecycles, conflicts, and
error edges using the same two objects under sealed schemas (Appendix A).

## 2. Ground and derived structure

**Ground** structure is committed by transactions and Drivers and remains history until
retracted or superseded — and remains *as* history even then. **Derived** structure is
maintained by rules; an edge is derived-live while at least one rule match supports it.
Supports are counted; removing the last support removes the edge from live views.
Derived caches may be discarded and rebuilt; eviction is fixpoint-invariant.

## 3. Surface identity and reference types

The public model exposes exactly:

```text
NodeRef<E>   EdgeRef<R>   ClaimRef<R>
SnapshotId   DataRevision   ProgramRevision
```

`ClaimRef<R>` is opaque and retry-stable: it identifies one ground assertion by one
source, so that retraction is unambiguous when several sources assert the same edge.
`assert` returns it (Part VII §2). SupportRef, MatchDigest, EdgeKey, and all other
identity constructions are engine-internal; their properties are observable only through
sealed provenance relations.

```text
NodeId  = Hash(entity compatibility domain, canonical key encoding)
EdgeId  = Hash(Edge, canon_ident(relation compatibility domain)
                    ++ canonical role tuple)
ClaimId = Hash(transaction intent, operation ordinal, source scope)
```

Unkeyed entities are minted only by transactions with transaction-stable identity. A
rule that derives a node uses `keyed by (...)`: a deterministic Skolem identity over the
rule and its key bindings. Until a declaration supplies an explicit compatibility-domain
token, an entity or relation's stable fully-qualified name is its compatibility domain;
introducing a token for an existing declaration is an explicit identity migration.

`MatchDigest` and `SupportRef` are deterministic engine-internal identities. For a rule
`R` and its bound-variable record `B` (binding names sorted by canonical name bytes),
`MatchDigest = Hash(Value, canon(R) ++ canon(B))`. For a support of `edge` from that
match, `SupportRef = Hash(Value, canon(edge) ++ canon(R) ++ canon(MatchDigest))`.
Only bound variables enter `B`: read-set edges and join order do not. Canonical encodings
are normative (Appendix G); floats are inadmissible in keys. *(Errata 0001: identity
formulas; 0002: compatibility-domain encoding.)*

## 4. Revisions and settlement

Each namespace exposes a totally ordered committed `DataRevision`. A transaction reads
one settled snapshot and commits atomically as a later revision, or fails.

```text
Settled(P, r) = least fixpoint of the rules of program revision P,
                evaluated phase by phase over Base(r)
```

`Base(r)` is ground structure after supersession, retraction, and ground key checking. A
revision is published fully settled or not at all. `SnapshotId = (namespace,
DataRevision, ProgramRevision)`; it appears on every query result, watch delta,
explanation, and export. (v5.0's seed-profile component is gone: numerics are now fixed
kernel semantics, Part V §8, so the snapshot needs no numeric parameter.)

## 5. Phases

The compiler builds the rule dependency graph, condenses strongly connected components,
and inserts strict boundaries at every non-monotone read: negation, absence,
aggregation, and mask-affected live reads. Positive recursion within a phase settles
semi-naively. Cycles through a non-monotone edge are compile-time errors carrying the
minimal offending dependency path. Appendix F is the normative construction. Programs
may name phases for governance; they never need to.

## 6. The mask primitive

`mask(target) by reason` is the single kernel mechanism for defeasibility. Both operands
are **edge references bound in the rule's own pattern**:

```brix
derive Override: mask(price) by manual from {
  price  @ ComputedPrice(order: o)
  manual @ ManualPrice(order: o)
}
```

producing sealed `Masked(target: price, by: manual, atPhase, atRevision)` edges whose
provenance names concrete edges on both sides.

**Normative phase rule.** Let M(R) be the set of rules that can mask relation R. Then:

1. every rule in M(R) is placed strictly above every producer of R;
2. every *ordinary live read* of R — in rules, queries, constraints, and aggregates —
   except the target binding inside a rule of M(R) itself, depends negatively on all of
   M(R) and is placed strictly above it;
3. a `history R(...)` read bypasses masks (and supersession) and creates no such
   dependency;
4. a mask edge may itself be masked; rule 2 applies transitively, and cycles among mask
   producers are compile-time errors.

Conceptually: `R production → R mask production → ordinary live reads of R`. This is
what lets the flagship's `FromComputed` rule read `ComputedPrice` naïvely and still see
the masked view — the dependency is inserted by the compiler because `Override` exists,
not because `FromComputed` mentions it.

Masked edges are absent from live views at and above the masking phase, present in
history always. Masks are ordinary derived edges: they gain and lose support
incrementally, and they are queryable and explainable like everything else. Defeat
calculi and override hierarchies are Edition 2 libraries that derive masks; the kernel
knows only this operation.

## 7. Model-closed and open relations

A locally owned relation is **model-closed**: its live extent is complete relative to
the committed claims and derived supports of the named snapshot. `without { ... }` over
model-closed relations is therefore sound with no ceremony — *as a statement about the
model*. `without { Delivered(order: o) }` means "no live Delivered fact exists in this
settled revision," not "the order was not delivered in the world." The second claim is
valid only under a **boundary completeness contract**: a declaration on the Driver or
import that owns the observation channel, or an explicit sealed
`Complete(relation, partition, through, authority)` witness.

Relations marked `open` (imports, federation, Driver-owned mirrors of external systems)
require such a witness before an absence-sensitive read compiles. The soundness
machinery exists exactly where trust boundaries exist, and nowhere else — and the
documentation obligation runs the other way too: a program that acts on model-absence at
a boundary (e.g., dunning a client for non-payment) should read its own `without`
clauses as claims about its observation channels, because that is what they are.

## 8. Key-conflict semantics

Every relation with `key(...)` obeys a per-kind conflict rule. There is never a silent
winner (Part II §4.7).

**Ground `rel`.** Asserting a tuple whose key matches a live tuple with a different
complete role tuple is a transaction conflict: the transaction fails unless the existing
claim is first retracted or superseded in the same transaction.

**`state rel`.** `set` atomically asserts the new version and supersedes the version the
transaction read. Two concurrent incompatible `set`s conflict under `serializable`;
under `snapshot`, the second commit fails on write-write detection.

**`event rel`.** Event identity is immutable: reasserting an existing `EventId` with
identical content is idempotent; with different content it fails the transaction.

**`entity`.** Every candidate entity row for one entity key participates in the same
`KeyConflict` exposure as a derived relation, whether its candidate came from `ensure`,
`fresh`, or a `keyed by (...)` rule. Repeating `ensure` with the same complete row is
idempotent; a different non-key payload is a distinct candidate. A conflicted entity key
has no ordinary live entity until exactly one candidate remains. This rule also applies
to disagreeing candidates staged by one transaction: the kernel exposes the disagreement
rather than silently choosing a payload. *(Erratum 0001: entity key conflicts.)*

**Derived relations.** Conflicting derived tuples cannot be rejected during settlement —
they may follow validly from valid ground facts. The engine derives a sealed edge
instead:

```text
KeyConflict(relation, key, candidates: Set<EdgeRef>,
            supports: Set<SupportRef>, atRevision)
```

and the conflicted key has **no ordinary live value** until exactly one candidate
remains live (by mask, by retraction of an input, or by program change). `KeyConflict`
is queryable, watchable, and constrainable — `constraint NoPriceConflicts strict {
KeyConflict(relation: ComputedPrice) }` is one line. Deterministic merge policies
(`resolve min by amount`) are Edition 2; the kernel default is to expose the
disagreement, not to arbitrate it.

Aggregates and completeness interact conservatively: a partition containing a live
`KeyConflict` withholds its completeness witness, so absence-sensitive reads over it do
not compile against a lie.

## 9. Error edges

When a rule's local computation fails under `?`, the engine derives:

```text
RuleError(rule: RuleRef, site: SiteId, partialMatch: MatchDigest,
          error: CanonicalValue, atRevision)
```

`SiteId` is a stable compiler-assigned expression-site identity, so two failing sites in
one rule never collide. Semantics of `?` inside a rule body:

- `Err(e)?` yields an error value derived from `e`; `None?` yields the standard
  `MissingValue` error;
- failure stops only the current match's continuation; sibling matches are unaffected;
- the error edge is supported by the inputs evaluated up to the failing site — fix the
  input and it retracts itself;
- error payloads follow canonical encoding; sensitive-payload policy arrives with
  Edition 3 labels.

`brix run` surfaces error edges prominently; `constraint ... strict` may reject
revisions that introduce them. Failure is graph structure, not control flow.

## 10. Time

- **Transaction time** — sealed on every claim; when the model learned it.
- **Valid time** — explicit roles or values; when it holds in the modeled world.
- **Simulation time** — `sim.Now { at: Instant }`, a sealed state relation written only
  by the bound clock driver: wall clock in production, stepper in scenarios. Rules read
  it like any relation; no rule reads an ambient clock.

## 11. Provenance as a relation

```text
Support(edge, rule, match, atRevision)
Claim(edge, source, transaction, atRevision)
```

Pattern-readable when authorized, never assignable by user code, compactable without
changing the answer to any explanation query. `brix why` is a stock query over these
relations; governance rules over them are ordinary BrixMS — subject to Part VIII §3.

---

# Part IV — Relations

## 1. Declaration forms

```brix-example
entity Name { key k: T; a: T2 }                     // keyed unary relation, bears identity
entity Name { a: T }                                 // unkeyed: transaction-mintable only

rel Name { r1: T1; r2: T2 } key(r1)                  // ground n-ary relation
state rel Name { ... } key(...)                      // one live version per key; `set` supersedes
event rel Name { key id: EventId; ...; at: Instant } time(at)
open rel Name { ... }                                // extent not owned here; absence needs witness
```

Role order is not semantic; identity and matching use role names. Roles hold entities,
edge references, or immutable values. Modifiers: `key`, `unique`, `time`, and the
physical hints `index` and `partition`, which never change meaning.

## 2. Derived relations and rules

```brix
rel R { a: A; b: B } key(a)

derive RuleName: R(a: x, b: y) from { ...pattern... }
```

Rule names exist for provenance; they carry no priority. Multiple rules into one
relation are set-union. A rule head is a relation tuple, a `keyed by` derived node, a
`mask(...) by ...` over bound edge references, a protocol request, or a constraint
violation — nothing else. Rules cannot assert ground claims, mint fresh identity,
retract, supersede, invoke Drivers, or forge sealed metadata. Conflicting heads under
one key become `KeyConflict` (Part III §8).

## 3. Pattern language

One pattern language serves rules, queries, constraints, and comprehensions:

```brix-example
R(role: x, other)                    // edge clause; punning: `other` = `other: other`
e @ R(role: x)                       // bind the edge reference
x: Entity { field, f2: v }           // entity attribute clause
let v = pureExpr                     // local binding; `?` allowed (error edges)
when boolExpr                        // guard
any { case {...} case {...} }        // disjunction with compatible bindings
exists { ... }                       // existence test, no exported bindings
without { ... }                      // stratified negation (model-closed or witnessed)
optional { ... }                     // bindings become Option<T>; same completeness rule
history R(role: x)                   // bypass masks and supersession; reads history
path Hop(from -> to)+ from x to y    // traversal with explicit incidence roles
```

Hypergraph traversal must name both incidence roles: `Shipment(origin -> destination)+`
is meaningful where `Shipment.location+` is not. Alternation and composition of steps
are supported with bounded or transitive repetition:

```brix
path ( Hop(from -> to) | Transfer(arrival -> departure) )+ from x to y
```

Evaluation is set-based and order-independent; shared variables are equijoins; a
disconnected conjunction requires explicit `cross { ... }`. Source order feeds
diagnostics and the initial cost model only.

## 4. Aggregation

Aggregates are functions over relation values applied to a completed extent, and the
compiler must be able to see that. The core language therefore has an `aggregate fn`
form:

```brix
aggregate fn count<S>(r: Rel<S>) -> Nat
aggregate fn median<S: Ord>(r: Rel<S>) -> Option<S>
```

Normatively:

- an `aggregate fn` consumes its `Rel` argument as a **complete-read**: calling it from
  a rule on a graph-derived extent creates a strict phase dependency on every relation
  in that extent, exactly like `without`;
- an ordinary (non-`aggregate`) function cannot receive a graph-derived `Rel` inside a
  rule body — this is a compile error naming the fix — so non-monotone reads cannot hide
  inside ordinary expression syntax and defeat stratification;
- outside rules (queries, Drivers, local code) any pure function may consume any `Rel`;
  the restriction protects settlement only;
- stock aggregates (`count`, `sum`, `min`, `max`, `avg`, `all`, `any`, `distinct`,
  `top`, `groupBy`, `collect`) declare incremental maintenance
  (`incremental by CountState` etc.); user aggregates without one are recomputed per
  delta with a cost diagnostic on large extents;
- recursion through an aggregate is a compile-time error in v5 (Edition 2 lifts this via
  lattices);
- stock aggregates over non-commutative-exact domains (floats) reduce in canonical row
  order (Part V §8).

```brix
let n     = count(from { Move(vehicle: v) })
let total = sum(from { Line(order: o, amount) } yield amount)
```

`from { ... } yield ...` is a relation comprehension producing `Rel<S>`.

## 5. Relation values

`Rel<S>` is a first-class immutable value: a typed finite relation with schema row `S`.
Snapshot-relative when captured from the graph; passable to pure functions; comparable
and hashable **only when `S: ValueCanonical`**. Value canonicality admits floats,
including inside `Estimate<T>`, using Appendix G's canonicalized IEEE bit patterns. A
separate `KeyCanonical` judgment governs keys and identity positions and excludes floats
at every nesting depth. Joins, windows, pivots, and matrix views are
`brix.rel` functions, not syntax. A transaction may assert a relation value through an
explicit template expansion; a rule may not. *(Erratum 0001: `Estimate<F64>` value
canonicality.)*

## 6. Queries and watches

```brix-example
query Name(args) -> Rel<Row> = from { ... } yield { ... }
```

A query is a pure function whose implicit first argument is a settled snapshot. Ordered
or paginated results require a deterministic final ordering key and return `Vector<Row>`
or `Page<Row>`; cursors bind the SnapshotId and query hash. `watch Name(args)` yields
one logical delta per published revision — never an intermediate state, never a silent
omission; coalescing across revisions is opt-in.

## 7. Constraints

```brix-example
constraint Name (advisory | strict | audit) { ...pattern... when ... }
```

Matches derive sealed `Violation` edges. `strict` rejects the offending transaction or
program activation, evaluated against the fully settled candidate revision. `audit` adds
retention and provenance depth.

## 8. Reflection and staged activation

Rules, relations, phases, and protocols have sealed descriptor relations in the schema
graph. Reflexive rules may **reason about and propose changes to** program descriptors —
deriving `ActivationProposal`, `DisableProposal`, or violation edges over `meta.Rule`.
Executable rule membership changes only through staged program activation: an authorized
activation transaction consumes proposals; the compiler produces a new
`ProgramRevision`; it is checked and shadow-settled against a chosen base revision; and
activation occurs atomically between data revisions. **No data revision can alter the
program that settles it.** Masking a rule's descriptor edge affects reasoning about the
rule, never the compiled rule's execution in the current program revision (Part VIII §3).

---

# Part V — Functions and values

## 1. Design stance

The function language is a small, modern, immutable-first expression language for
computing values and relation transformations inside the model — not a second place to
build systems. Boundaries own concurrency.

## 2. Types

```text
Unit Bool Char String Bytes
I8..I128  U8..U128  Int Nat  Decimal<P,S>  F32 F64
Instant Duration Date TimeOfDay TimeZone
Option<T> Result<T,E>
List<T> Vector<T> Set<T> Map<K,V> Bag<T>
Rel<S>  NodeRef<E>  EdgeRef<R>  ClaimRef<R>
Quantity<Measure>  Money<Currency>  Probability  EventId
```

Checked fixed-width arithmetic by default; wrapping and saturating forms explicit.
`Int`/`Nat` arbitrary precision. No null; no unchecked exceptions; `panic` is a boundary
effect for invariant failure.

## 3. ADTs, records, traits

`enum`, `record`, `newtype` (with `opaque` and `validated` forms generating checked
`.try` constructors), structural row-typed anonymous records, exhaustive `match` with
nested destructuring and or-patterns. Traits provide constrained polymorphism with
associated types; coherence per package graph; no inheritance. Relation role matching is
row-polymorphic, which is what lets a pattern bind a subset of roles and what makes
generic graph utilities typecheck. All of this is core language, not kernel (Part II
§2).

## 4. Effects

`(A, B) -> C !{effects}`; the empty row is pure; effects inferred and polymorphic.
Kernel effect atoms: `net<S>`, `fs<S>`, `clock`, `random`, `console`, `graph.read<S>`,
`graph.write<S>`, `panic`, `diverge`, `solver<S>`. Effects say what may happen;
capabilities (Part VII §4) say under whose authority.

## 5. Totality, relaxed

Rule bodies, guards, keys, and canonical encoders must be pure and deterministic — but
not total. `partial` functions under `?` in rules turn failure into sealed error edges
(Part III §9). `diverge`-capable functions remain banned in rules; structural recursion
and accepted termination measures infer as non-diverging.

## 6. Local mutation

Scoped mutation where it cannot escape: `region { var b = v.thaw(); ...; b.freeze() }`.
Resources (handles, sockets, capabilities, transactions) are affine; borrows inferred;
no user-visible lifetime calculus on immutable application code.

## 7. Units, money, uncertainty

`measure`/`unit`/`currency` declarations give dimensioned quantities with static
dimension checking. Compound dimensions compose through arithmetic: `Money<EUR> /
Kilometre` multiplied by `Quantity<Kilometre>` yields `Money<EUR>`, as the flagship's
pricing rule demonstrates. Money crosses currencies only through explicit conversion
values. `Estimate<T> = { value, error, confidence, method }` carries honest
approximation; guards over estimates compare conservative bounds by lint-enforced
convention.

## 8. Numerical determinism (kernel semantics)

Determinism inside settlement is not a configuration profile; it is the semantics:

- IEEE-754 operations inside settlement use strict semantics: no reassociation, no
  contraction (no FMA unless written), no fast-math;
- NaNs canonicalize to one bit pattern; comparisons and total-order operations are
  defined (`totalOrder` for sorting, IEEE comparison for guards, both deterministic);
- stock aggregates over floats reduce in **canonical row order** (Appendix G ordering of
  the input extent);
- parallel and incremental execution must reproduce the canonical sequential result
  bit-for-bit — this is a conformance test, not an aspiration;
- floats remain inadmissible in keys and canonical identity; `Rel<S>` hashes only when
  `S: ValueCanonical`;
- high-performance approximate reductions are available as functions returning
  `Estimate<F64>`, or at boundaries, where the effect row says so.

Consequence: "plans may change cost, never meaning" holds without a snapshot-level
numeric parameter, and scenario reproducibility is exact across machines and thread
counts.

---

# Part VI — Time and simulation

## 1. The simulation clock

`sim.Now` is a sealed state relation written only by the bound clock driver: wall clock
at declared resolution in production, stepper in scenarios. Timers are graph structure —
a rule derives `sim.Timer.request(at, key)`; the clock driver commits
`sim.Timer.fired` — identically real or simulated. Nothing in the model blocks on time;
time arrives as facts.

## 2. Scenarios and adapters

```brix-example
scenario Name {
  seed 42
  bind sim.Clock(resolution: 1 hours)
  bind P to <adapter>
  setup { ...transactions... }
  step every D for T { ...transactions per tick... }
  at T { ...one-shot transactions... }
  assert always { boolQuery }        // every settled revision
  assert eventually { boolQuery }    // by scenario end
  assert at end { boolQuery }
}
```

Protocol adapters are explicit about lifecycle meaning:

- `sim.capture` — records requests and attempt admissions, resolves nothing; requests
  remain pending; useful when the assertion is about *asking*, not *answering*;
- `sim.succeed(|req| Outcome {...})` — deterministic success outcomes from a pure
  function of the request version;
- `sim.script { when cond => Outcome {...} ... otherwise => Outcome {...} }` — a
  deterministic decision table over request versions; the flagship's simulated planner;
- `sim.replay(capture)` — replays outcomes previously captured (from an earlier scenario
  or from production history), failing loudly on requests the capture does not cover;
- `sim.fail(|req| Failure {...})` — deterministic failure injection, honoring declared
  retry policy so backoff behavior is itself testable.

Adapter execution is part of the scenario's deterministic schedule: after each settled
revision, pending requests are resolved by the bound adapters in canonical request
order before the next tick. A scenario run is a deterministic function of (program
revision, scenario, seed); its output is a graph, so the entire analysis toolkit works
on simulation output with zero extra machinery. Property-style sweeps: `seed each
1..100`.

## 3. Replay, fork, what-if

`brix replay --from rev:R` re-settles history forward (time-travel debugging). `brix
fork --at rev:R` opens a scratch namespace; alternative transactions settle an
alternative future; `brix diff` compares settled views. REPL `what if { ... }` is fork +
transact + query + discard. Because production history is committed boundary facts plus
deterministic settlement, these operate on production exactly as on scenarios — the
audit story and the debugging story are one story.

## 4. Formal simulation lineage

`brix.sim` ships mappings for classical formalisms: DEVS atomic and coupled models
(states as `state rel`, `ta` as timer requests, transitions as rules over timer-fired
and input events), agent-based models (agents as entities, behaviors as rules, ticks as
clock steps), system dynamics (stocks as state relations, flows as rules over `sim.Now`
deltas), Monte Carlo (seeded `random` + scenario sweeps). A formalism is a library that
compiles its vocabulary onto relations and rules — the knowledge-broker-interface idea
from the original thesis, landed where it belonged.

---

# Part VII — Boundaries

## 1. Boundary principle

The model touches the world in exactly three places: **transactions** carry authorized
ground facts in; **protocols + Drivers** carry derived requests out and committed
outcomes back; **queries/watches** carry settled views out. Inside those walls
everything is pure and deterministic; outside, everything is explicitly
capability-bearing.

## 2. Transactions

```brix
transaction serializable {
  let o = ensure Order { ... }              // return-or-create keyed ENTITY
  fresh Note { ... }                        // mint unkeyed, transaction-stable identity
  let c: ClaimRef<Move> =
    assert Move(order: o, vehicle: v)       // assert RELATION; returns claim reference
  set OrderStatus(order: o, value: Open)    // state rel: assert + supersede
  retract c                                 // consume a ClaimRef; unambiguous
  supersede newEdge over oldEdge            // explicit lineage
}
```

`ensure` is defined for keyed **entities** only: it returns the existing candidate or
creates one. Repeating the same complete entity row is idempotent; competing payloads
under one key are settled as `KeyConflict` candidates (Part III §8). Relations are
asserted, never ensured — obtaining identity and asserting structure are different acts
and read differently. `assert` returns a `ClaimRef<R>`; because claim
identity is derived from transaction intent and operation ordinal, the reference is
retry-stable. `retract` consumes a `ClaimRef` (the affine type system prevents double
retraction); retracting withdraws one source's claim and the edge stays live if other
claims or supports remain.

A transaction reads one settled snapshot and commits atomically or fails.
`serializable` (default) detects read/write and predicate conflicts; `snapshot` admits
write skew for bulk import, explicitly. Retries keep one intent identity: `fresh` nodes,
ClaimRefs, protocol keys, and seeded randomness are retry-stable. Transactions cannot
perform irreversible external effects.

## 3. Protocols: request identity, versioning, lifecycle

A protocol request declaration behaves like a **derived state relation**. Normative
identity:

```text
RequestKey     = Hash(protocol, declared key roles)          // the logical intent
RequestVersion = Hash(RequestKey, canonical request payload) // one desired content
AttemptKey     = Hash(RequestVersion, attempt ordinal)
```

Lifecycle (Appendix H is the normative state machine):

```text
Desired(version)                       // derived support exists for this payload
  -> Leased(version, lease)
  -> Attempted(version, attempt)
  -> Succeeded(version, outcome) | Failed(version, outcome)
Superseded(newVersion, oldVersion)     // payload changed under the same RequestKey
Withdrawn(version)                     // all derived support lost before terminal
Cancelled(version, outcome)            // explicit cancellation, honest outcomes
```

Rules, made explicit because retries and coalescing are undefinable without them:

- changing the payload under a `RequestKey` supersedes the prior desired version; an
  in-flight attempt for a superseded version runs to completion and records its outcome
  against **its own version**;
- attempts and terminal outcomes bind to a RequestVersion, never to the bare key; one
  RequestKey may therefore accumulate multiple terminal outcome histories, one per
  version — the flagship's rising risk does exactly this;
- protocol `policy` declares whether a successful prior version **satisfies** a
  successor (`satisfies: samePayload | sameKey | never`; default `samePayload`) — the
  answer to "we already notified at 0.81; is 0.93 a new notification?" is policy, not
  accident;
- support loss withdraws the current desired version only; completed history is never
  unwound;
- external idempotency keys default to the RequestVersion and may be declared as the
  RequestKey when the external system deduplicates by intent;
- backpressure `Coalesce` is defined as version supersession under the declared key —
  which is why it needed this section to be definable.

Leases prevent double execution by healthy workers; idempotency handles the irreducible
ambiguity of expired leases. Retry policy is declarative and deterministic over
committed facts; backoff timers are clock facts. Overflow policies: `Defer`, `Reject`,
`Coalesce`; `Drop` does not exist — a lossy protocol must name and record its loss.
Every transition is history.

## 4. Capabilities

Unforgeable affine values, host-issued or attenuated: `Net<HostPattern>`,
`Fs<Root, Mode>`, `Clock`, `Random<Alg>`, `GraphReader<Scope>`, `GraphWriter<Scope>`,
`Console`. Effects say what; capabilities say who and over which scope; both are
required at a boundary. Capabilities do not serialize into ordinary roles. (`Solver` and
delegated-authority tokens arrive with their editions.)

## 5. Profiles

- **`brix run file.brix`** — script profile: implicit main, no manifest, ambient dev
  capabilities (console, fs under cwd, clock, seeded random, net with first-use notice).
- **`brix serve pkg --manifest caps.toml`** — production: every capability named,
  scoped, auditable; no ambient anything; identical model code.
- **`brix sim Scenario`** — scenario: no external capabilities exist; every boundary is
  bound to an adapter.

The wall between profiles is the deployment command, never the program text.

---

# Part VIII — Reflexivity and tooling

## 1. The system reads itself

Sealed relations expose the program to itself: `meta.Rule`, `meta.Relation`,
`meta.Phase`, `meta.Protocol`, `meta.Activation`; `Support`, `Claim`; `perf.RuleCost`,
`perf.IndexUse` (populated under profiling). All are ordinary query targets under
`graph.read<Meta>`.

## 2. Tooling as queries

`brix why EDGE [--at rev]` walks Support/Claim to ground facts and renders the
derivation tree. `brix whynot PATTERN [--at rev]` explains the failed joins and guards
nearest a match, including which `without`, witness, or mask blocked it. `brix diff
revA revB` gives the settled-view delta with per-rule attribution. `brix profile`
populates `perf.*`. `brix repl` is snapshot-pinned with `what if`. Each is a thin
renderer over a published query in `brix.meta`; third parties get the same power by
construction.

## 3. Reflexive governance, bounded

Reflexive rules may reason about and propose changes to program descriptors. Executable
rule membership changes only through staged program activation and can affect no
settlement earlier than the next ProgramRevision. The pipeline: rules derive proposals
and violations over `meta.*` → an authorized activation transaction consumes them → the
compiler produces a new ProgramRevision → check, shadow-settle against a selected base
revision, compare → publish atomically between data revisions. Masking `meta.Rule`
edges changes what governance rules conclude; it never changes what the current program
revision executes. The system governs itself; it never modifies the rule set of its own
current settlement.

## 4. Language services

One `brixd` process serves LSP (types, effects, phase info, rule-to-relation
navigation, inline `why` on hover) and DAP (match-level stepping rendered as joins and
deltas, not invented statement order). Canonical `brix fmt`; `--json` on every command;
dry-run plans on every mutating command. This entire part is toolchain contract, not
kernel (Part II §2): its observable behavior is specified, its implementation is free.


## 5. Native testing and verification

Testing is a language-and-toolchain contract, not a third-party framework. A test runs the
same program under an isolated graph fork, commits ordinary transactions and boundary
outcomes, waits for complete settlement, and asserts over published snapshots. It never
calls a rule directly and never observes an intra-settlement state.

The core test forms are:

```text
test             concrete revision examples
property         generated graph and transaction properties
scenario         temporal simulation under seeded boundaries
contract         Protocol, Driver, capability and connector behavior
migration test   schema and program-revision compatibility
benchmark        repeatable cost and resource budgets
statistical test learned-policy and Monte Carlo evidence
```

The assertion vocabulary includes `exists`, `none`, cardinality, relation equality,
revision diffs, history presence, mask presence, provenance, why-not, constraints,
`RuleError`, protocol lifecycle, and explicit approximate numerical comparison.

Every conforming implementation provides an intentionally independent full-recomputation
evaluator. The principal executable correctness condition is:

```text
incrementally settled graph at every published revision
==
whole-world recomputation at that revision
```

The reference runner additionally supports property shrinking, graph-aware mutation
testing, structured fuzzing, schedule and join-plan perturbation, worker-count variation,
cache-eviction invariance, backend parity, fault injection, deterministic failure bundles,
and replay from the exact failed revision.

## 6. Semantic coverage

Coverage is graph-semantic rather than primarily line-based. The runner records:

```text
rules matched and never matched
guards observed true and false
disjunction arms
partial-function success and failure sites
positive-recursion depths
aggregates over empty and non-empty extents
masks introduced and removed
key conflicts created and resolved
constraints activated
protocol outcomes, retries and cancellation branches
scenario temporal states
formalism transitions and event kinds
```

Line coverage remains available for Driver implementations. It is not considered
sufficient evidence for graph logic.

## 7. Test results are relations

Test runs populate sealed schemas such as `test.Run`, `test.Case`, `test.Assertion`,
`test.Failure`, `test.Counterexample`, `test.Coverage`, `test.Mutation`, and
`test.Benchmark`. IDEs, CI systems, dashboards, and release gates render queries over
those relations. Failed property cases and simulation traces are content-addressed,
portable artifacts.

## 8. Built-in quality engine

`brix check` and `brix quality` analyze syntax, types, effects, graph structure,
architecture, boundary behavior, tests, performance risk, documentation, migrations,
and supply-chain evidence. Quality analysis is enabled by default and has three levels:

```text
semantic error      program meaning is invalid; never suppressible
quality violation   valid program rejected by the active quality profile
advisory finding    actionable risk that does not fail unless promoted
```

The standard profiles are `prototype`, `standard`, `production`, and `critical`.
Production activation runs the production profile unless a stricter signed workspace
profile is selected.

The quality vector keeps dimensions separate:

```text
Correctness  Maintainability  Graph clarity  Boundary safety  Test strength
Explainability  Performance risk  Schema stability  Operational readiness
Supply-chain integrity
```

No single average score can hide a critical weakness.

## 9. Graph-native quality metrics

The analyzer measures rule width, join width, dependency and phase depth,
non-monotone depth, recursive-component size, fan-in, fan-out, match cardinality,
derived cardinality, delta amplification, mask complexity, provenance depth, boundary
request volume, and protocol pressure. It detects disconnected joins, weak keys,
low-selectivity joins, likely cardinality explosions, duplicated normalized graph
patterns, mixed-responsibility relations, state/event confusion, Boolean state that
should be an enum, stringly typed identity, missing units or currencies, and domain
packages coupled to concrete Drivers or vendors.

Functions and Drivers also receive conventional checks: cyclomatic and cognitive
complexity, nesting, parameter and effect-row width, partial-operation count,
unreachable patterns, unsafe numerical conversion, ignored `Result` or `Estimate`
information, resource leaks, unbounded retries, overbroad capabilities, missing timeout
and cancellation behavior, and irreversible effects before durable outcome commit.

Suppressions require a diagnostic ID, reason, owner, tracking issue, and expiry date.
Expired suppressions fail the active gate and remain queryable engineering debt.

## 10. Architecture and activation gates

Workspaces may declare allowed package layers and dependency directions. The analyzer
checks imports, relation references, protocol bindings, generated dependencies,
capability changes, schema ownership, and infrastructure leakage. Every production
activation records a signed quality result containing program and toolchain digests,
quality profile, test and mutation evidence, conformance status, capability diff,
schema-compatibility result, performance comparison, suppressions, and artifact
signatures.

The activation record answers not only *what* was deployed, but *what evidence allowed it
to deploy*.

---

# Part IX — Standard library

The standard library is organized into semantic, engineering, modeling, and integration
families. Packages are ordinary versioned BrixMS packages unless explicitly described as
reference-engine components.

## 1. Semantic foundation

```text
brix.core       Option, Result, ADTs, records, collections, text, bytes,
                canonical encoding, hashing and foundational traits
brix.rel        relation comprehensions, joins, grouping, aggregates, windows,
                pivots, top-k and set algebra over Rel<S>
brix.logic      first-order and Horn surfaces lowered to constraints and derive rules
brix.time       instants, durations, calendars, zones, valid time and watermarks
brix.math       exact numerics, Decimal, strict numerical utilities, statistics,
                distributions, estimates, quadrature and root finding
brix.units      measures, quantities, conversion and dimensional analysis
brix.money      typed currencies, exchange values and explicit conversion
```

## 2. Engineering foundation

```text
brix.sim        clocks, timers, scenarios, seeded adapters, replay, forks and sweeps
brix.test       assertions, properties, generators, shrinking, mutation, fuzz,
                semantic coverage, contracts and benchmarks
brix.quality    quality profiles, architecture rules, metrics, gates and debt queries
brix.meta       schema, rules, phases, provenance, activation, quality and perf queries
brix.io         standard file, HTTP, database and protocol families
brix.data       relation frames, typed manipulation, JSON, CSV, Arrow, Parquet,
                schema conversion, profiles, cleaning recipes and quality findings
brix.features   observation-time-safe feature definitions, feature sets, lineage,
                materialization and train-serving consistency
brix.datasets   immutable snapshot-bound datasets, splits, resampling and leakage checks
brix.stats      formula-based statistical models, inference, diagnostics and calibration
brix.ml         estimator, workflow, artifact, prediction, evaluation and registry contracts
brix.experiment reproducible training, tuning, comparison, reports and evidence bundles
brix.viz        declarative statistical, graph, simulation and interactive visualization
brix.learn      policy records, rewards, contextual bandits, logged evaluation,
                calibration, baselines and experience datasets
```

## 3. Ontology and formalism foundation

```text
brix.ontology               ontology descriptors, mappings and entailment profiles
brix.ontology.rdf           RDF import/export and canonical identity mapping
brix.ontology.rdfs          RDFS vocabulary and entailment
brix.ontology.owlrl         incrementally maintainable OWL-RL-compatible rules
brix.ontology.shacl         shape validation lowered to constraints
brix.ontology.skos          concept schemes and controlled vocabularies

brix.formalism.des          discrete-event simulation and event calendar
brix.formalism.devs         atomic and coupled DEVS mappings
brix.formalism.systemdynamics stocks, flows, integration, delays and analysis
brix.formalism.abm          agents, activation, proposals, arbitration and analysis
brix.formalism.spatial      grids, regions, networks and neighborhood queries
brix.formalism.fsm          finite-state machines
brix.formalism.statechart   hierarchical and orthogonal state models
brix.formalism.petri        place/transition and colored Petri nets
brix.formalism.queueing     queues, resources and service disciplines
brix.formalism.hybrid       settled synchronization among supported formalisms
```

## 4. Data, storage, execution and observability

```text
brix.arrow              canonical columnar interchange for Rel<S>
brix.parquet            portable snapshots, histories and analytical datasets
brix.storage.cozo       first-party CozoDB storage and safe query pushdown
brix.postgres           operational PostgreSQL boundary and SQL pushdown
brix.postgres.cdc       logical-replication ingestion and completeness positions
brix.databricks         Databricks transfer and deployment integration
brix.databricks.lakebase Lakebase profile over the PostgreSQL contract
brix.delta              Delta snapshot, history and revision-delta interchange
brix.databricks.unity   Unity Catalog registration, ownership and lineage
brix.duckdb             local analytical companion
brix.datafusion         optional Arrow-native analytical subplan executor
brix.kafka              external event transport, offsets and watermarks
brix.opendal            capability-scoped object and file storage
brix.wasmtime           standard sandboxed Wasm Component Driver host
brix.otel               OpenTelemetry traces, metrics and logs
brix.solver.z3          SAT/SMT and bounded-verification protocol implementation
brix.solver.highs       LP/MIP/QP optimization protocol implementation
brix.policy.onnx        portable learned-policy inference
brix.dbsp.oracle        differential incremental-computation oracle
brix.oci                OCI artifact packaging
brix.sigstore           signatures, attestations and transparency verification
```

Arrow is the shared in-memory interchange layer; Parquet and Delta are durable analytical
forms. CozoDB is the first-party embedded graph-oriented backend. PostgreSQL is the
portable operational-database contract; Databricks Lakebase is its premier
Databricks-native deployment profile. External systems never define BrixMS settlement
semantics.

All of Part IX is standard library or first-party integration contract. The kernel remains
independent from these implementations.

---

# Part X — The Rust reference engine

## 1. Architecture

```text
brixc  (compiler)                    brix-engine (runtime)
  lexer/parser -> AST                  node arena: interned, typed
  name/type/effect check               edge store: columnar, role-indexed per relation
  pattern -> relational algebra        index manager: hash + ordered per key/hint
  phase inference (Appendix F)         settlement: semi-naive deltas per phase
  ontology/formalism lowering          event calendar + simulation coordinators
  plan: join orders, index selection   revision log: append-only, snapshot isolation
  emit: engine IR                      provenance: support counting + claims
                                       conflict detection: per-kind key rules
                                       boundary host: WASM component Drivers
                                       storage: memory + first-party CozoDB
brixd (LSP/DAP)     brix (CLI: run/sim/test/quality/ontology/formalism/...)
```

## 2. Load-bearing choices

- **Canonical bytes first.** One canonical-encoding crate (Appendix G) underpins
  identity, hashing, aggregation order, and serde; it is the first thing built and the
  most tested.
- **Semi-naive by hand, DBSP as the oracle.** The reference settlement is a hand-rolled
  semi-naive evaluator, provably equivalent to full recomputation via the conformance
  suite; DBSP/differential is the differential-testing oracle and a candidate backend,
  not a day-one dependency.
- **Strict floats.** No fast-math anywhere in settlement; canonical-order reductions
  for stock aggregates; the bit-for-bit parallel-equals-sequential test (Appendix I)
  gates every optimizer change.
- **Phases via petgraph condensation** with the mask dependency edges of Part III §6;
  plain cost model; adaptivity later.
- **Drivers as WASM components** (wasmtime); capability imports scoped by manifest;
  native in-process ABI for the stock library.
- **Storage:** memory-first with an append-only revision log on disk; Arrow for
  columnar interchange. Distribution is Edition 4; do not pre-pay for it.

## 3. Milestones

```text
M1  kernel graph + transactions + settlement + key conflicts, REPL           ~8 wks
M2  type/effect checker, model-closed negation, aggregate fns, error edges   ~8 wks
M3  mask primitive + phase rule, state/event rels, provenance, why/whynot    ~6 wks
M4  protocols with request versioning, Drivers (WASM), clock, run/serve      ~6 wks
M5  brix.sim: scenarios, adapters, seeds, replay/fork; flagship end to end   ~6 wks
M6  LSP, fmt, native tests, quality engine, conformance suite v1                 ~8 wks
M7  ontology profiles + RDF/SHACL; DES calendar and DEVS mapping                       ~8 wks
M8  system dynamics, ABM, hybrid synchronization and formalism tooling                 ~10 wks
```

Two people, roughly nine months to the **first reflexive-tooling milestone**: `brix why`
running as a BrixMS query on the engine it explains. (Self-hosting — the compiler in
BrixMS — is a different, later ambition and is not claimed here.) Conformance is the
executable suite of Appendix I; there is no prose-only conformance. The suite grows with
the engine from M1: incremental-equals-full-recompute is the property every milestone is
gated on, not a retrofit.

---

# Part XI — Editions roadmap

Deferred semantics re-enter only through the layer admission rules of Part II §2, each
as an **edition**: additive, feature-gated, semantics-preserving for prior programs.

```text
Edition 2 (Recursion & Choice)   lattice relations with verified merge laws
                                 (recursion through aggregates); `resolve` merge
                                 policies on derived keys; defeat-calculus library
                                 over mask; user effect handlers (one-shot,
                                 boundary-only); solver capabilities and
                                 optimization patterns
Edition 3 (Trust)                information-flow labels; redaction with sealed
                                 tombstones; sensitive error-payload policy;
                                 delegated-authority tokens; privacy-preserving
                                 explanation
Edition 4 (Scale)                distributed namespaces with consensus-backed
                                 revision order; partitioned settlement;
                                 watermarked federation; heterogeneous acceleration
Edition 5 (Proof)                mechanized metatheory for the kernel; verified
                                 migrations; verified lattice/aggregate laws
Edition 6 (Learning)             differentiable graph learning; distributed and
                                 federated training; learned rule proposals feeding
                                 staged activation (the policy suggests program
                                 changes; Part VIII §3 still governs adoption)
```

v4 serves as **design input** for Editions 2–4: its treatments of defeat, lattices,
labels, redaction, and distribution are the starting inventory. Each edition must
restate and ratify its complete semantics against the v5 kernel when opened; nothing is
adopted by reference from a superseded document.

---

# Part XII — The learned-policy boundary

## 1. Rationale and admission

The model has, until now, two write-sources: transactions (human and system decisions,
top-down) and Driver outcomes (the world answering, bottom-up). A learned component is a
third kind of participant — it neither deduces nor decides; it *suggests under
uncertainty*, and it improves by consuming the outcomes of its own suggestions. Left
unstandardized, every project reinvents this contract badly: model versions leak into
mutable state, decisions lose the version that made them, feedback attribution becomes
folklore, and "the model did it" becomes an unanswerable provenance question.

`policy` standardizes the **interaction contract only**. No learning algorithm enters
the language. Admission per Part II §2: `policy` is core language because the flagship
needs it to be written pleasantly (§8 below) and because it lowers *entirely* to
existing kernel objects — protocols, sealed relations, immutable entities, staged
activation. The kernel does not change; conformance categories 13–14 test the lowering,
not new semantics.

## 2. Declaration

```brix
policy AssignVehicle {
  context   { order: Order }
  candidates -> Rel<{ vehicle: Vehicle }> from {
    AssignmentCandidate(order, vehicle)
  }
  suggestion { vehicle: Vehicle; score: Estimate<F64> }
  feedback   { deliveredOnTime: Bool; margin: Money<EUR>;
               emptyDistance: Quantity<Kilometre> }
  objective  AssignmentReward          // a pure fn feedback -> F64, in brix.learn terms
  authority  advisory                  // the default, stated anyway
}
```

Typing rules: the candidate row and suggestion row require `S: Canonical` (the decision
event stores `candidatesDigest`, and undigestible candidates would make off-policy
evaluation unsound); scores are `Estimate<F64>`, honest about approximation by
construction; `context` roles must be bindable from the requesting rule's pattern.

## 3. Lowering

The compiler generates, per policy `Y`, the sealed relations of Appendix A:

- `Y.Version` — an immutable entity `{ key digest: Digest; trainedThrough:
  DataRevision }`. Training produces these; nothing ever mutates one.
- `Y.Active` — a `state rel` mapping the policy to one version, settable **only by
  transaction**. Activation is Part VIII §3 discipline applied to learned artifacts:
  explicit, atomic, between revisions, historically legible.
- `Y.Decision` — an event committed by the policy Driver: version, snapshot,
  candidatesDigest, chosen suggestion, **propensity** (the behavior-policy probability
  of the chosen suggestion, mandatory — without it, off-policy evaluation of candidate
  versions from logged history is impossible), seed, timestamp.
- `Y.Shadow` — decisions by non-active candidate versions evaluated against the same
  requests; structurally identical to `Y.Decision` and **never consumable**: no rule can
  read `Y.Shadow` into a request-deriving pattern (compile-time check). Shadow
  evaluation is therefore safe by construction, not by review.
- `Y.Feedback` — **a derived relation, not an event.** This corrects the incoming
  design: "the order this decision touched was delivered on time, at this margin" is a
  *deduction over settled outcomes*, and deductions are rules. Feedback rules are
  ordinary `derive` clauses keyed by decision:

```brix
derive Outcome: AssignVehicle.Feedback(
  decision: d, deliveredOnTime: onTime, margin, emptyDistance
) from {
  d @ AssignVehicle.Decision(chosen: v)
  DecisionApplied(decision: d, order: o)        // authority trail, §5
  Delivered(order: o, at: t)
  o: Order { due }
  let onTime = t <= due
  Margin(order: o, amount: margin)
  EmptyLeg(order: o, vehicle: v, length: emptyDistance)
}
```

  Feedback thereby inherits everything derived structure has: it retracts and reattaches
  with its inputs, it is explainable by `brix why`, and it can never be fabricated by
  the component it evaluates.

Two engine-side rules complete the lowering. **Snapshot stamping:** a rule never reads
its own snapshot identity — the settling revision does not exist yet, and
`currentSnapshot()` inside settlement would be self-reference. The Driver's lease
carries the SnapshotId it evaluates against, and the engine stamps it into the decision
event. Provenance records what the policy actually saw, exactly. **Inference request
identity:** `Y.request` is an ordinary protocol request under Part VII §3 — RequestKey
from the declared context key, RequestVersion from the canonical context payload plus
candidatesDigest — so changed candidates supersede pending inference, and the whole
retry/coalesce/withdraw machinery applies unchanged.

## 4. Invocation

A rule derives an inference request from settled structure; a policy Driver evaluates
it under the active (or shadow) version and commits a decision plus suggestions as
ordinary graph facts:

```brix
derive RequestAdvice: AssignVehicle.request(order: o) from {
  Unassigned(order: o)
}

rel AssignmentAdvice {
  order: Order; vehicle: Vehicle
  score: Estimate<F64>; decision: AssignVehicle.Decision
} key(order, decision)
```

Suggestions do nothing. They are inert structure until a rule with authority consumes
them — which is the entire point.

## 5. Authority

`authority` declares the ceiling on what may consume suggestions, and it **compiles to
generated constraints, not to a separate mechanism** — a second correction to the
incoming design: an enforcement path that bypassed rules would be a fourth surface
category through the back door.

- `authority advisory` (default): the compiler emits a strict constraint rejecting any
  program revision containing a rule that derives a *command* protocol request from
  this policy's suggestions. Suggestions are visible to queries, dashboards, and
  humans; acting on them is a human transaction.
- `authority gated by G`: consuming rules must include the gate relation `G()` in their
  pattern; the compiler verifies presence, and the gate is ordinary state — flipping it
  is a transaction with provenance, and every derived command retracts the moment the
  gate does.
- `authority autonomous within Scope`: consuming rules may act without a gate, but only
  toward protocols named in `Scope`, and the generated audit relation
  `DecisionApplied(decision, ...)` is derived alongside every command — the authority
  trail that feedback rules join on (§3).

In all three modes, an acting rule remains an ordinary rule: subject to constraints,
phases, capabilities on the target protocol, and `brix why`. The flagship's consuming
rule under `gated`:

```brix
derive AutoAssign: AssignVehicleCommand.request(order: o, vehicle: v) from {
  AssignmentAdvice(order: o, vehicle: v, score, decision: d)
  AutoAssignmentEnabled()
  when score.lowerBound > 0.90
  without { AssignmentSafetyRisk(order: o, vehicle: v) }
}
```

## 6. Training is version production

Training never mutates the active model. The pipeline is graph-shaped end to end:

```text
decision + feedback history through revision R      (queryable, canonical)
        ↓  training Driver (PyTorch, Burn, Candle, ONNX, XGBoost, remote — irrelevant)
Y.Version candidate (digest, trainedThrough: R)     (immutable entity)
        ↓  scenario replay + shadow evaluation      (Y.Shadow events, comparison queries)
        ↓  off-policy evaluation                    (brix.learn: IPS / doubly-robust over
                                                     propensity-carrying history)
approved activation transaction                     (set Y.Active(version: candidate))
```

Every historical decision keeps the version that produced it, under any number of later
activations. "Which model made this call, on what data, seeing what candidates, with
what confidence, and what happened next" is four joins, forever.

## 7. Scenarios

Policies bind like any boundary: `bind AssignVehicle to sim.policy(version)` runs real
inference deterministically (version + snapshot + candidatesDigest + seed reproduce the
decision bit-for-bit — conformance category 14); `sim.script` substitutes a decision
table; `sim.replay(capture)` replays production decisions into counterfactual worlds.
Off-policy evaluation of a candidate against logged history requires no simulation at
all — it is a `brix.learn` query over `Y.Decision ⋈ Y.Feedback`.

## 8. Flagship extension

The flagship's simulated planner becomes the policy above: `AssignmentCandidate` derives
capacity-feasible vehicle–order pairs (the deductive floor under the learned choice —
the policy cannot suggest what deduction has excluded); `AssignVehicle` suggests;
`AutoAssign` consumes under gate + confidence threshold + safety negation; the
delivery events already in the scenario drive the feedback derivation; a `at 96 hours`
step activates a second `Y.Version` and the closing assertions compare regret across
versions via `brix.learn`. The scenario exercises every generated relation, both
authority failure modes (gate off, confidence below threshold), and one staged
activation — which is what admits `policy` under the flagship rule rather than by
enthusiasm.

## 9. What stays out

Neural architectures, gradients, optimizers, tensor runtimes, PPO/DQN/SAC, replay
buffers, GPU execution, feature stores, hyperparameter search, and model serialization
formats are Drivers and libraries, invisible to semantics. Active-learning `observe`
blocks (suggesting information worth acquiring) are deferred to `brix.learn`: an
observation request is expressible today as a second suggestion type consumed by an
ordinary rule, and a dedicated form must earn core-language admission through a flagship
that needs it. Learned *rule proposals* — the policy suggesting program changes — are
Edition 6, and even there they feed staged activation like every other proposal: Part
VIII §3 does not bend for components that learned to write.

---


# Part XIII — First-party ecosystem distribution

## 1. Support levels

The ecosystem has four support levels:

```text
Core distribution      installed with BrixMS and covered by language conformance
First-party package    maintained and versioned by the BrixMS project
Certified integration  independently implemented against a published suite
Community package      uses stable public interfaces without project certification
```

The minimum installation contains `brix`, `brixc`, `brix-engine`,
`brix-storage-memory`, `brixfmt`, `brixd`, `brix.sim`, `brix.test`, and
`brix.quality`. It can compile, run, simulate, test, explain, profile, and enforce quality
without an external database or cloud service.

The standard developer installation adds CozoDB, Arrow, Parquet, Wasmtime,
OpenTelemetry, Tree-sitter, and the official editor extension. Data-platform and
intelligence integrations are separately installable first-party packages.

## 2. Tooling contract

The unified CLI includes:

```text
brix check      static and semantic validation
brix fmt        canonical non-configurable formatting
brix run        development execution
brix serve      production execution under a capability manifest
brix query      snapshot-pinned query execution
brix watch      settled revision deltas
brix why        derivation and claim explanation
brix whynot     failed-match, absence and mask explanation
brix diff       graph, revision and program differences
brix fork       alternative future from a named revision
brix replay     deterministic replay of facts and outcomes
brix sim        scenario and formalism simulation
brix test       native verification suite
brix quality    quality vector and activation gates
brix plan       logical and physical plans
brix profile    runtime and graph-semantic cost
brix ontology   ontology import, validation, entailment and alignment
brix formalism  inspection, lowering, visualization and conformance
brix modelcheck reachability, deadlock and bounded formal analysis
brix package    build, lock, publish and audit packages
brix deploy     shadow-settle and atomically activate
```

`brixd` serves LSP and graph-native DAP. The official Tree-sitter grammar, VS Code
extension, and stable editor protocol ship with the standard installation.

## 3. Backend and integration guarantee

Every backend and connector is behind a BrixMS-owned contract. Unsupported pushdown
falls back safely. Storage encodings never become language identity. A backend may alter
latency, memory, indexing, and physical plans; it may not alter revisions, settlement,
provenance, masks, identity, protocol lifecycle, or explanation.

First-party boundary packages must provide deterministic scenario substitutes and a
connector-conformance suite.

## 4. Supply-chain and activation evidence

Packages, Wasm Drivers, policy models, schemas, snapshots, and deployment bundles are
content-addressed and may be distributed as OCI artifacts. Production profiles support
Sigstore verification, dependency and capability attestations, reproducible lockfiles,
and signed quality records. The complete artifact set is referenced from
`meta.Activation`.

---

# Part XIV — Ontologies and formalism framework

## 1. Ontology is executable model meaning

An ontology defines concepts, properties, subsumption, equivalence, disjointness,
domains, ranges, keys, cardinalities, shapes, external identifiers, axioms, imports,
versions, and alignments. Ontology descriptors are sealed relations in the same graph:

```text
onto.Ontology  onto.Concept  onto.Property  onto.SubClassOf
onto.SubPropertyOf  onto.Domain  onto.Range  onto.Equivalent
onto.Disjoint  onto.Key  onto.Shape  onto.Axiom  onto.Iri
onto.Alignment  onto.Entailment  onto.Inconsistency
```

An ontology declaration attaches formal meaning to existing entities and relations; it
does not duplicate application state. BrixMS identity and semantic equivalence remain
separate: external IRIs and equivalence assertions never silently merge nodes.

## 2. Operational closure and open-world meaning

A model-owned relation is complete relative to the selected settled snapshot. An
ontology imported from an external source is open unless an authority supplies a
completeness contract. Ontological absence, model absence, and real-world absence are
therefore distinct.

Entailment profiles are `Schema`, `Horn`, `RDFS`, `OWL-RL`, `Open`, `Closed`, and
`External`. Profiles expressible as stratified Horn rules are incrementally maintained
inside settlement. More expressive reasoning occurs through a typed reasoner protocol
whose outcome retains the ontology version, question, answer, and certificate or
explanation.

Contradictions derive explicit `onto.Inconsistency` edges; they do not cause logical
explosion. Quality policy determines whether an inconsistency is advisory, strict, or
quarantined.

## 3. Shapes and validation

Ontology shapes lower to ordinary constraints and violations, so the same validation
results are visible in imports, tests, simulations, quality gates, and production. RDF,
RDFS, OWL-RL, SHACL, and SKOS packages preserve source, graph, ontology version, license,
valid time, transaction time, and unsupported-semantics findings.

## 4. Formalism contract

A formalism is a versioned modeling discipline defining:

1. vocabulary and surface forms;
2. well-formedness constraints;
3. lowering to relations, functions, transactions, protocols and scenarios;
4. execution semantics;
5. analysis queries;
6. visualization descriptors;
7. import and export mappings;
8. an executable conformance suite.

A formalism cannot create a second runtime. Its generated declarations are visible
through `meta.Formalism`, `meta.FormalismConstruct`, `meta.FormalismMapping`, and
`meta.ModelInstance`. Explanations can be rendered both in native BrixMS terms and in the
originating formalism vocabulary.

## 5. Composition

Several formalism views may govern one graph only when state ownership, time mapping,
transition authority, random streams, and synchronization are explicit. The compiler
rejects conflicting state writers and produces a formalism-composition report naming
what each formalism owns, advises, observes, and emits.

---

# Part XV — Native discrete-event simulation

## 1. DES correspondence

A DES state is a settled graph snapshot. A discrete event commits an atomic simulation
transaction. The graph then settles completely before any later event can observe it.
The future-event list is represented by scheduled-event relations; the trace is the
revision history.

The operational cycle is:

```text
select the earliest scheduled SimTime batch
advance sim.Now
commit event occurrences and transition facts
settle all phases to a fixpoint
check assertions and observations
schedule resulting future events
repeat
```

## 2. Superdense time

DES uses `SimTime { at: Instant; microstep: Nat }`. Physical time orders first;
microstep orders zero-delay causal chains at one physical instant. Events at the same
`SimTime` are simultaneous by default. A same-time transition that schedules another
same-time event necessarily schedules a later microstep.

The simulator derives `sim.ZenoViolation` when microsteps advance beyond the configured
limit without physical-time progress.

## 3. Simultaneous events and conflicts

A simultaneous batch observes one settled pre-state. Its transitions produce proposed
state changes, which are checked and committed atomically. Incompatible writes to one
state key derive `sim.SimultaneousConflict` and fail under the standard strict profile.
Ordering that affects meaning must be declared as event precedence or separate
microsteps; source order and worker scheduling have no semantic role.

## 4. Scheduling, cancellation and randomness

The standard lifecycle includes `sim.Schedule.request`, `.accepted`, `.cancel`,
`.cancelled`, `sim.Event.fired`, and `.skipped`. The scheduler is a boundary in production
and a deterministic scenario service in simulation.

Random draws are keyed by scenario seed, formalism instance, event identity, draw site,
and draw index. Adding an unrelated draw elsewhere cannot perturb an existing event's
stream. Replications are therefore parallelizable and exactly replayable.

## 5. DEVS and DES analysis

`brix.formalism.devs` maps atomic and coupled DEVS state, inputs, outputs, internal,
external and confluent transitions, time advance, and couplings onto event and timer
relations. Confluent behavior is explicit; host scheduling never chooses it.

Stock DES analyses include event count, event-calendar size, zero-delay depth, resource
utilization, queue length, waiting time, throughput, warm-up bias, replication confidence
intervals, simultaneous conflicts, and Zeno behavior.

---

# Part XVI — System dynamics

## 1. Stocks, flows and auxiliaries

System dynamics is a first-party formalism over typed state relations and rate
expressions. Stocks are keyed state relations. Inflows and outflows are dimensioned rates.
Auxiliaries and parameters are pure functions or derived relations. Feedback is the
ordinary dependency graph.

```brix
system dynamics FleetCapacity {
  stock AvailableVehicles: Quantity<VehicleCount> = 100 vehicles
  inflow Returned into AvailableVehicles rate returnRate
  outflow Dispatched from AvailableVehicles rate dispatchRate
  auxiliary utilization = ActiveVehicles.value / TotalFleet.value
}
```

The surface lowers to explicit stock and rate relations. An integration step commits a
new stock state as a simulation transaction, after which ordinary settlement derives all
logical consequences.

## 2. Numerical integration contract

Integration is not logical deduction. It executes under a recorded deterministic
simulation profile naming the method, implementation version, tolerances, minimum and
maximum step, event-detection policy, reduction order, and numerical digest. First-party
methods include Euler, Heun, RK4, RK45, Backward Euler, and BDF. The v1 minimum is Euler
and RK4 with fixed steps; adaptive and stiff solvers may be separately packaged.

Changing the profile changes the simulation SnapshotId. Solver steps, rejected steps,
error estimates, convergence failures, and event locations are recorded as relations.

## 3. Dimensional and algebraic safety

Units are mandatory for stock and flow equations. The compiler rejects adding a rate to
a stock without integration, incompatible dimensions, and cross-currency arithmetic
without an explicit conversion value.

Instantaneous equation cycles are classified as stock-broken feedback, statically
solvable loops, declared numerical root problems, or inconsistent/underdetermined loops.
Numeric algebraic solutions retain method, tolerance, residual and convergence evidence.

## 4. Delays and discrete crossings

Fixed and exponential delays lower to explicit internal state rather than invisible host
queues. Threshold crossings are located within the declared tolerance, emitted as DES
events, settled, and used to restart integration from the new state. This is the
normative DES/system-dynamics bridge.

## 5. Analysis and verification

The package provides feedback-loop classification, conservation, dimensional checks,
equilibria, local stability, sensitivity, elasticity, parameter sweeps, uncertainty,
overshoot, settling time, oscillation, solver error and step-size convergence. Quality
findings include missing units, hidden algebraic loops, unstable profiles, stiffness,
chattering, uninitialized delays and unbounded stocks.

---

# Part XVII — Agent-based modeling

## 1. Agents are entities, not actors

An agent is an entity participating in behavior and state relations. It does not
implicitly own a thread, mailbox, private mutable heap, execution order, or process
lifecycle. Behaviorally relevant state, perception, communication, decision, and history
are graph structure.

The canonical cycle is:

```text
perceive settled snapshot
choose or suggest
produce action proposals
arbitrate shared conflicts
commit accepted changes atomically
settle
```

## 2. Perception and decision

Perception is a typed query with an explicit visibility scope. Agents do not
implicitly see the whole graph. Decisions may use pure rules, seeded stochastic
functions, learned policies, solvers, humans, or boundaries. Every choice mechanism
retains its version, candidates, seed, propensity where relevant, and outcome.

Agents propose actions as relations. They never mutate shared graph state during
settlement.

## 3. Activation schedules

Supported schedules are `Simultaneous`, `SequentialByKey`, `RandomPermutation`,
`RandomWithReplacement`, `PriorityBy(query)`, `EventDriven`, and `Custom`.
Activation events are explicit and replayable. An ABM whose outcome depends on an
unspecified activation order is invalid.

Under simultaneous activation, all agents perceive one pre-state, proposals are
collected, arbitration resolves conflicts, and accepted effects commit together. Under
sequential activation, each agent observes the settled result of prior accepted actions.
The sequence is explicit and seeded.

## 4. Neighborhoods, space and communication

Graph and spatial neighborhoods define local interaction. The spatial package supplies
points, regions, grids, networks, distance, containment, intersection, nearest-neighbor
queries and indexes. Physical acceleration may use a backend, but neighborhood meaning
is defined in BrixMS.

Communication is modeled by sent and delivered event relations with explicit latency,
loss, duplication, trust, bandwidth and topology policies. Messages are domain events,
not runtime mailboxes.

## 5. Population, adaptation and analysis

Agent creation and departure occur through transactions or formalism transitions and
remain historically queryable. Heterogeneity is expressed through types, ontology
classes, attributes, state, network position, experience and policy version. Learning
updates explicit beliefs or produces immutable policy versions; no hidden model mutates
inside settlement.

Analyses include population turnover, state distributions, network structure, diffusion,
segregation, inequality, mobility, adoption, path dependence, activation-order
sensitivity, seed sensitivity and micro-to-macro attribution. Quality checks detect
global perception, implicit ordering, all-to-all interaction risk, unresolved conflicts,
random-stream coupling, hidden external state and communication without delivery
semantics.

---

# Part XVIII — Hybrid multimethod simulation

## 1. One world, several disciplines

DES models when discrete occurrences happen. System dynamics models continuous
accumulation and feedback. ABM models heterogeneous local actors and interactions.
Deduction establishes necessary consequences and admissible actions. Solvers choose among
feasible alternatives. Learned policies suggest under uncertainty.

A hybrid model declares which component owns each state and how settled information is
exchanged.

```brix
hybrid simulation CityMobility {
  discrete TrafficEvents
  continuous TrafficStocks
  agents DriverPopulation

  synchronize {
    continuousToDiscrete on threshold
    agentsToContinuous every 1 minutes
    continuousToAgents every 1 minutes
    settle after every exchange
  }
}
```

## 2. Synchronization contract

At each synchronization point, the active formalism produces proposed facts or a
transaction; those changes commit atomically; BrixMS settles; only then may another
formalism observe them. No formalism sees another's intermediate state.

DES events may change continuous parameters or stocks. Located continuous threshold
crossings emit DES events. Agents may respond to events and schedule future events.
Agent aggregates may drive continuous rates; macro stocks may influence agent perception.
Each direction declares event-driven, fixed-interval, threshold, or explicit mapping
semantics.

## 3. Conflict, identity and reproducibility

Hybrid composition must define state ownership, time-domain mapping, simultaneous-event
policy, transition authority, numerical profile, seed streams, and external boundary
bindings. The compiler rejects overlapping writers without arbitration. A hybrid run is
reproducible from the program revision, scenario, seed set, formalism versions, numerical
profile and boundary facts.

## 4. Version-one modeling platform

The first stable modeling distribution includes ontology descriptors, Horn/RDFS and
OWL-RL-compatible entailment, SHACL-compatible validation, native DES with superdense
time, DEVS mapping, fixed-step system dynamics with units and threshold events, ABM with
explicit activation and arbitration, graph/spatial neighborhoods, and settled hybrid
synchronization. More advanced continuous, distributed and hardware-in-the-loop
simulation remains additive first-party work.

---


# Part XIX — Unified reasoning and intelligence

## 19.1 The epistemic type system

BrixMS combines several reasoning mechanisms without pretending that they mean the same
thing.

| Mechanism | Meaning | Canonical result |
|---|---|---|
| Deduction | Necessarily follows from settled structure | Derived edge with proof |
| Ontology entailment | Follows under a declared ontology profile | Entailment with axiom support |
| Exact mathematics | Canonical exact calculation | Exact value |
| Validated numerics | Value enclosed by verified bounds | `Interval<T>` or certificate |
| Approximate numerics | Numerical estimate under a method | `Estimate<T>` |
| Probability | Degree of belief under a model | Probability-bearing proposition |
| SAT/SMT proof | Satisfiable, unsatisfiable, or unknown | Model, certificate, counterexample, or `Unknown` |
| Optimization | Best admissible candidate under objectives | Solution, bound, gap, and certificate |
| Learned policy | Fallible preference under uncertainty | `Suggestion<T>` |
| Language model | Grounded interpretation or generative proposal | `Grounded<T>` or candidate artifact |
| Transaction | Authorized change to ground state | Committed revision |
| Protocol | Requested external action and observed outcome | Versioned boundary lifecycle |

No implicit conversion may erase this status. In particular, the following conversions are
invalid without an explicit checking or authorization operation:

```text
Estimate<T>        -> T
Interval<T>        -> T
Probability        -> Bool
Suggestion<T>      -> T
Grounded<T>        -> ground claim
SolverCandidate<T> -> committed state
GeneratedProgram   -> active program
```

The defining rule is:

> **Logic proves. Mathematics calculates. Policies recommend. Language models interpret
> and propose. Transactions authorize. Protocols act.**

## 19.2 Common reasoning records

The standard library defines common status-bearing values:

```brix
record Estimate<T> {
  value: T
  error: ErrorBound<T>
  confidence: Probability
  method: MethodDescriptor
}

record Interval<T> {
  lower: T
  upper: T
}

enum ProofResult<T> {
  Proven { proposition: T, proof: logic.Proof }
  Disproven { counterexample: logic.Counterexample }
  Unknown { reason: logic.UnknownReason }
}

record Grounded<T> {
  value: T
  evidence: Set<EvidenceRef>
  inference: reasoning.RunRef
}

record Suggestion<T> {
  candidate: T
  score: Estimate<Real>
  evidence: Set<EvidenceRef>
  producer: reasoning.ReasonerRef
}
```

Every boundary reasoning run records the input snapshot, program revision, reasoner and
version, method, parameters, seed where relevant, evidence, result, uncertainty or proof
status, resource use, and verification findings.

## 19.3 Logic

The native logical core remains typed stratified relational deduction:

```text
typed facts
Horn-style implication
positive recursion
stratified negation and aggregation
constraints
mask-based defeat
deterministic existential construction
model-closed and open relation extents
proof and provenance relations
```

A finite first-order surface compiles into rules, constraints, existence queries, and
stable-key existential nodes:

```brix
logic {
  forall o: Order {
    HighValue(o) and Open(o)
    implies RequiresReview(o)
  }
}
```

Every native conclusion has an inspectable proof containing the rule, bindings, premises,
guards, aggregate evidence, phase, snapshot, and program revision. `brix why` and
`brix whynot` query this proof graph.

First-party logic packages include:

```text
brix.logic.datalog
brix.logic.constraints
brix.logic.defeasible
brix.logic.paraconsistent
brix.logic.abduction
brix.logic.temporal
brix.logic.ltl
brix.logic.ctl
brix.logic.mtl
brix.logic.deontic
brix.logic.epistemic
brix.logic.probabilistic
brix.logic.contracts
brix.logic.proof
brix.logic.smt
brix.logic.modelcheck
```

Richer profiles either compile to native relations and masks or execute through typed
reasoner boundaries. Solver timeout or incompleteness produces `Unknown`, never falsehood.
Contradictory imported claims may use explicit four-valued paraconsistent relations without
changing ordinary core semantics.

Transactions and Drivers may declare preconditions, postconditions, and preserved
invariants. Refinement newtypes provide constrained values without requiring an unrestricted
dependent type system.

## 19.4 Mathematics

The standard numeric tower is:

```text
Nat, Int
I8 I16 I32 I64 I128
U8 U16 U32 U64 U128
Rational
Decimal<P,S>
F32 F64
Complex<T>
Interval<T>
Estimate<T>
Probability
Quantity<Measure>
Money<Currency>
```

Information-losing conversions are explicit. `Rational`, `Decimal`, exact dimensional
quantities, and exact money values may have canonical identity. Floating values may not be
used in keys.

The settlement numerical profile specifies strict IEEE-754 behavior, canonical NaNs,
signed-zero rules, deterministic reductions, no unsafe reassociation, and a normative
transcendental implementation profile. Parallel execution must reproduce the normative
result.

Units and dimensions are part of type checking:

```brix
measure Length
measure Time
unit kilometre: Length = 1000 metres
unit hour: Time = 3600 seconds

type Speed = Quantity<Length / Time>
```

First-party mathematical packages include:

```text
brix.math            exact and elementary mathematics
brix.rational        arbitrary-precision rational values
brix.complex         complex values and functions
brix.units           dimensions and unit conversion
brix.money           currencies and exchange-value records
brix.linalg          dense and sparse linear algebra
brix.tensor          typed multidimensional arrays
brix.stats           descriptive and inferential statistics
brix.prob            marginal and joint distributions
brix.numerics        roots, quadrature, interpolation, ODE and DAE methods
brix.interval        validated interval numerics
brix.symbolic        symbolic expressions and transformations
brix.autodiff        gradients, Jacobians, Hessians, JVPs, and VJPs
brix.optimize        typed optimization and solver lowering
brix.geometry        typed geometry and coordinate systems
brix.spatial         spatial and network calculations
brix.graph           graph and hypergraph mathematics
```

Numerically sensitive operations return method and convergence information rather than
naked scalars. Large iterative, GPU, optimization, probabilistic, and theorem-proving
operations execute through boundaries and commit versioned results.

## 19.5 Probability, optimization, and choice

BrixMS distinguishes a marginal distribution from a joint model. Independence is explicit;
dependent quantities derive from a common `JointDist<Row>`. Sampling is keyed by scenario,
reasoning site, semantic subject, and draw index rather than global draw order.

Optimization models declare variables, constraints, objectives, and result requirements.
Suitable models lower to HiGHS, Z3, or certified extension solvers. Results include solver
identity, model digest, feasibility, objective, best bound, gap, certificate or unsat core.
Ordinary constraints verify returned solutions where practical.

## 19.6 Native language-model support

A language model is a typed reasoning boundary, never a settlement primitive.

```brix
language task AssessLateOrder {
  input { order: Order }

  context from {
    Order(order)
    OrderStatus(order, status)
    LateRisk(order, risk)
    EffectivePrice(order, amount)
  }

  output OrderAssessment
  evidence required
  mode grounded

  tools { OrderTimeline, RouteHistory, CheckCapacity }

  policy {
    maximumOutputTokens: 800
    timeout: 10 seconds
    onUnsupportedClaim: Reject
  }
}
```

The compiler generates typed request, success, rejection, and failure relations. Outputs
are ordinary BrixMS types with constrained decoding and canonical validation. Grounded
claims cite evidence from the exact authorized snapshot supplied to the model.

Tools are typed queries or protocols. A model receives no ambient graph, filesystem,
network, clock, secret, or code-execution access. Multi-step agentic workflows are explicit
relations with bounded steps, costs, tools, cancellation, and replay.

Prompts, model requirements, provider bindings, output schemas, and evaluation datasets are
immutable versioned artifacts. Replays use committed outcomes. Generated queries, rules,
migrations, ontology alignments, and documentation remain inert until parsed, checked,
tested, shadowed, and activated as a later `ProgramRevision`.

First-party packages include:

```text
brix.llm
brix.llm.gateway
brix.llm.local
brix.llm.retrieve
brix.llm.tools
brix.llm.eval
brix.llm.developer
```

Private chain-of-thought is not part of BrixMS semantics. Auditability comes from inputs,
evidence, tools, typed outputs, declared concise rationale where requested, symbolic
verification, model and prompt versions, cost, and latency.

## 19.7 Combined reasoning workflows

The normal architecture composes mechanisms:

```text
unstructured observation
  -> LLM typed extraction proposal
  -> schema, ontology, and evidence validation
  -> authorized claim transaction
  -> logical settlement
  -> simulation and optimization
  -> decision record
  -> authorized protocol action
```

An LLM may propose a plan; units and types validate it; a solver checks feasibility; a
simulation evaluates futures; policies score alternatives; rules enforce constraints; a
human or governance policy authorizes the action. No mechanism is forced to impersonate
all the others.

## 19.8 Frame governance

The checks of §19.6 close the mechanical attack class — injection, smuggled
instructions, laundered authority, fabricated citations. They do not close
**curation**: a context assembled from legitimate, authorized, individually true
evidence, selected so one conclusion becomes inevitable. Every item-level check
passes such a request, because the attack lives in the selection. The architecture's
honest position: it relocates the frame from ephemeral prompt text into the
versioned, diffable, activation-governed context pattern — the frame becomes a
reviewable artifact — and governs it twice. **Context-ablation sensitivity** is a
first-party evaluation dimension: inference re-runs across context variants with
items and classes removed; conclusion stability is a measured distribution feeding
release gates — a conclusion that flips on one curated item is fragile or framed,
and the number says which. **Separation of authorship and authority** is a quality
gate: the principal who authored a task's context pattern cannot be the sole
approver of rules consuming its output at gated or autonomous authority
(`LLM.ContextCuration` blocks; the two-key requirement is recorded in
`meta.Activation`). Whoever designs the frame does not alone decide what the frame
authorizes. The residual, stated plainly: against a sufficiently patient author who
controls context assembly over time, each in-band check passes individually; the
remaining defense is that the whole construction is visible, versioned, and
queryable after the fact — containment, not immunity, and the specification says so.

# Part XX — The reflexive self-model

## 20.1 Mandatory semantic reflection

Every BrixMS program always contains a complete typed versioned model of itself. Reflection
exposes semantic descriptors, not mutable runtime objects.

The self-model contains six connected projections:

| Projection | Contents |
|---|---|
| Program | packages, modules, declarations, source identity |
| Semantic | types, effects, dependencies, phases, lowerings |
| Runtime | active revisions, bindings, configuration, capabilities |
| Evidence | claims, proofs, tests, quality findings, decisions |
| Operational | plans, indexes, costs, health, budgets |
| Documentation | descriptions, examples, ownership, runbooks |

Descriptors such as `TypeRef`, `RelationRef`, `RuleRef`, `ProtocolRef`, `FormalismRef`,
`ViewRef`, and `BrickRef` are stable semantic references, not pointers to compiler memory.

## 20.2 The sealed meta graph

Every conforming implementation publishes versioned sealed relations including:

```text
meta.Program, meta.ProgramRevision, meta.Package, meta.Module
meta.Type, meta.EntityType, meta.RelationType, meta.RecordType, meta.Role, meta.Key
meta.Function, meta.Effect, meta.CapabilityRequirement
meta.Rule, meta.RuleHead, meta.PatternClause, meta.Dependency, meta.Phase
meta.Query, meta.Command, meta.Watch, meta.Protocol, meta.Driver
meta.LanguageTask, meta.Policy, meta.Ontology, meta.Formalism
meta.Brick, meta.Port, meta.Adapter, meta.View, meta.Action, meta.Renderer
meta.Test, meta.QualityFinding, meta.Owner, meta.Stability
meta.Deployment, meta.Configuration, meta.Migration, meta.CompatibilityResult
```

User code may query these relations under normal authorization. It may not forge them.
The meta schema describes itself through a finite canonical bootstrap snapshot associated
with the language edition.

## 20.3 Authored and resolved models

The self-model preserves both what was written and what it means after resolution:

```text
source declarations and documentation
resolved names and types
inferred effects and phases
normalized patterns
formalism lowerings
ontology compilation
client/server placement
brick dependency closure
```

The resolved semantic model is normative for execution. The authored model supports
editing, explanation, migration, and documentation.

## 20.4 Safe self-change

The current program revision is immutable during settlement. Rules may inspect it and
derive proposals, but semantic change follows:

```text
observation
 -> change proposal
 -> candidate source and self-model
 -> parse, type, effect, phase, security, and compatibility checks
 -> tests and quality gates
 -> shadow settlement
 -> authorization
 -> new ProgramRevision
```

Discovery never grants authority. Reflection cannot expose inaccessible data, manufacture
capabilities, invoke declarations by unchecked strings, forge provenance, or mutate the
active program.

## 20.5 Living documentation

Documentation is typed model structure:

```text
doc.Summary, doc.Description, doc.Rationale, doc.Invariant
doc.Assumption, doc.Warning, doc.Remediation, doc.Example
doc.Owner, doc.Stability, doc.Deprecation, doc.Runbook, doc.Decision
```

Executable examples compile and run as tests. Generated documentation combines authored
prose with inferred schemas, rule dependencies, formalism diagrams, API contracts,
capability inventories, test coverage, quality, ownership, and program history.

Documentation remains distinct from proof. A rationale explains intent; a proof establishes
a logical consequence; a test demonstrates sampled behavior; a quality finding applies a
policy. The self-model connects these without conflating them.

## 20.6 Reflexive tooling

First-party tools are defined as views over the public self-model:

```text
brix doc, why, whynot, inspect, graph, plan, profile
brix quality, coverage, compat, diff, migrate, impact
brix ownership, capabilities, model-health
```

Brix Model Studio and third-party tooling use the same versioned reflection contract.
Semantic program diffs classify schema, dependency, effect, capability, behavior,
documentation, simulation, and interaction changes.

# Part XXI — Observing reality, causality, decisions, and work

## 21.1 The closed intelligence loop

A professional BrixMS system maintains this explicit loop:

```text
sense -> interpret -> reconcile -> commit -> settle -> simulate
      -> decide -> authorize -> act -> observe outcomes -> learn
```

Every transition is represented by graph relations and revisions.

## 21.2 Observations and claims

External inputs enter as observations rather than unquestioned facts:

```brix
event rel ObservationReceived {
  key id: ObservationId
  source: Source
  observedAt: ValidTime
  receivedAt: TransactionTime
  payload: ObservationPayload
  confidence: Option<Probability>
}
```

The platform distinguishes observations, extraction candidates, reconciled claims,
authorized ground claims, derived conclusions, predictions, and simulations.

Data contracts define schemas, units, compatibility, event-time policy, duplicate handling,
late data, source completeness, validation, quarantine, and lineage. First-party ingestion
covers PostgreSQL and Lakebase CDC, Kafka, Arrow, Parquet, Delta, files, object storage,
HTTP, WebSocket, sensors, forms, and document extraction.

## 21.3 Identity and reconciliation

Similarity never silently merges identity. `brix.identity` and `brix.reconcile` provide
source identifiers, deterministic keys, probabilistic match candidates, equivalence
proposals, merge and split operations, conflict handling, survivorship, human review, and
historical identity change.

A candidate match is evidence-bearing and advisory. A governed transaction establishes any
authoritative equivalence or consolidation.

## 21.4 Model health and reality divergence

The platform measures whether its model remains useful, not only whether its processes are
running:

```text
model.ObservationCoverage
model.DataFreshness
model.ReconciliationLag
model.PredictionError
model.Calibration
model.UnexplainedOutcome
model.AssumptionBreach
model.OntologyMismatch
model.PolicyDrift
model.RealityDivergence
```

Runtime health, data quality, model assumptions, environmental change, policy degradation,
and prediction drift are distinct findings.

## 21.5 Causal formalism

`brix.formalism.causal` models causal variables, structural equations, confounders,
mediators, colliders, interventions, treatments, outcomes, identification assumptions, and
counterfactuals.

Observation and intervention are distinct. The model can ask not only what correlates with
an outcome but what the model predicts would change under an explicit `do` intervention.
Causal structures and assumptions are versioned relations; causal estimates carry method,
data, uncertainty, and sensitivity to unmeasured confounding.

LLMs may propose causal structures, but activation requires evidence, tests, and review.

## 21.6 Decision intelligence

A decision is a first-class record of alternatives, objectives, constraints, uncertainty,
stakeholder preferences, simulations, policy suggestions, selected action, rejected
alternatives, authorization, and observed outcome.

```brix
decision FleetAssignment {
  alternatives from CandidateAssignment

  objectives {
    minimize Cost weight 0.4
    minimize DeliveryRisk weight 0.4
    minimize Emissions weight 0.2
  }

  constraints { Capacity, Availability, ContractTerms }
  risk { maximumProbability(CriticalFailure) <= 0.01 }
  approval OperationsManager
}
```

Decision support may combine logic, optimization, simulation, causal inference, learned
policies, and human judgment while preserving each mechanism's status.

## 21.7 Workflows and human tasks

Long-running organizational work is graph structure:

```brix
workflow CapacityException {
  start when { CapacityViolation(order: o) }

  task ReviewException assigned to Dispatcher {
    due within 2 hours
    outcome Approve
    outcome Reassign
    outcome Reject
  }
}
```

Workflows lower to state and event relations, timers, transactions, protocols, and
constraints. They support tasks, approvals, separation of duties, delegation, deadlines,
escalation, cancellation, retries, compensation, and complete history. They are not a
second process runtime.

# Part XXII — Semantic bricks and the domain library ecosystem

## 22.1 The component thesis

The primary unit of reuse is the **brick**: a context-independent semantic component that
may contain both the model of a domain and the model of interacting with that domain.

```brix
brick OrderManagement @ 1.0.0 {
  ontology OrderOntology
  model OrderModel
  behavior OrderRules
  workflow OrderWorkflow
  simulation OrderSimulation
  decisions OrderDecisions

  experience {
    view OrderDetails
    form OrderEditor
    dashboard OrderOperations
  }

  provides { port Orders, port OrderExperience }
  requires { port Inventory, port TransportCapacity }
}
```

Domain, workflow, simulation, decision, interaction, and presentation are projections of one
versioned hypergraph. They are not separate backend and frontend component systems.

## 22.2 Context independence

A reusable brick may not depend implicitly on a particular application shell, renderer,
database, tenant, authentication provider, global singleton, LLM provider, screen size, or
deployment topology. Requirements are expressed through typed ports and configuration.

Ports may expose types, concepts, relations, queries, commands, watches, events,
capabilities, views, workflows, simulation interfaces, and decision interfaces. They
preserve units, ontology meaning, revisions, errors, evidence, and authority.

## 22.3 Ownership and modularity

Every mutable state relation has a declared owning brick. Other bricks may observe it,
derive from it, or propose changes through commands, but may not assign it directly unless
shared ownership is explicitly declared.

Cross-brick dependencies must pass through declared ports under modular production
profiles. The shared graph enables integration; ports, ownership, and capabilities prevent
global coupling.

## 22.4 Adapters and ontology alignment

A semantic adapter is itself a brick. It may map types, concepts, units, queries, commands,
protocols, provenance, and compatibility assumptions. Structural similarity alone never
connects two components. Composition requires direct semantic identity, a declared ontology
relationship, or an explicit adapter.

## 22.5 UI belongs to the brick

A view is a typed query plus semantic presentation and interaction declarations. An action
produces a typed proposal that follows the same authorization and transaction path whether
it originated from a browser, CLI, workflow, LLM, optimizer, or another brick.

Interaction state that affects meaning, authority, collaboration, continuity, or
explanation is graph state. Purely renderer-local details may remain local.

## 22.6 Domain libraries

The registry organizes reusable bricks at three levels:

```text
foundation bricks: Address, Person, Organization, Money, Location, Approval
domain bricks: Order, Shipment, Invoice, Asset, Contract, Appointment
domain assemblies: FleetDispatch, WarehouseOperations, ClaimsProcessing
```

First-party namespaces include:

```text
brix.domain.identity
brix.domain.organization
brix.domain.location
brix.domain.time
brix.domain.money
brix.domain.contracts
brix.domain.orders
brix.domain.inventory
brix.domain.logistics
brix.domain.manufacturing
brix.domain.energy
brix.domain.health
brix.domain.finance
brix.domain.publicsector
```

Assemblies are tested compositions of independently reusable bricks, not monolithic
frameworks. Composition is favored over inheritance.

## 22.7 Brick compatibility

Compatibility is multidimensional:

```text
model, data, port, ontology, behavior, simulation
interaction, presentation, security, and deployment compatibility
```

A brick artifact contains its source, resolved semantic model, ports, ownership, ontology
references, rules, views, renderer assets, tests, examples, migrations, quality record,
SBOM, provenance, and signatures.

# Part XXIII — Reactive client hypergraph and rendering

## 23.1 Client model

A BrixMS client is a reactive local model of the user's authorized world. It contains:

```text
authorized remote graph projection
client-derived relations
interaction and navigation state
drafts and validation
optimistic proposal branches
subscription and revision state
semantic render graph
```

The architecture is:

```text
authoritative hypergraph
 -> revision-aware authorized projection
 -> client hypergraph
 -> incremental client settlement
 -> semantic render graph
 -> renderer adapter
```

The interface is a continuously updated projection, not a static generated website.

## 23.2 Client settlement

The client supports a defined portable subset of the settlement language: typed relations,
pure functions, positive derivation, stratified local negation, aggregation, local state,
validation, and view rules. Heavy global recursion, solvers, LLMs, and large simulations
remain boundary operations unless a deployment explicitly supplies compatible local
engines.

Incoming graph deltas apply atomically from one `SnapshotId` to another. Components never
observe a partially installed server revision.

## 23.3 Server projections and synchronization

Clients request declared server projections. Authorization is applied before data leaves the
server. WebSocket synchronization transports inserted, retracted, masked, and unmasked
edges with revision cursors, sequence numbers, resume tokens, bounded buffering,
backpressure, and explicit resnapshot behavior.

SSE supports read-oriented subscriptions; HTTP supports snapshots and commands; typed RPC
supports service integration. All transports preserve the same semantic contract.

## 23.4 Semantic render graph

Views derive semantic render nodes, bindings, children, actions, accessibility semantics,
and layout constraints. Renderers map this graph to concrete controls.

First-party adapters include:

```text
@brixms/react
@brixms/vue
@brixms/svelte
@brixms/web-components
@brixms/react-native
brix-terminal
```

React is a supported host and renderer adapter, not the BrixMS UI semantics. A declarative
view may be rendered directly, while hooks such as `useBrixQuery`, `useBrixAction`, and
`useBrixDraft` provide controlled framework integration.

## 23.5 State classes

The client distinguishes:

```text
AuthoritativeRemote
AuthoritativeLocal
EphemeralInteraction
Draft
OptimisticProposal
Simulated
Predicted
```

Optimistic UI is a local scenario branch. Acceptance merges a committed server revision;
rejection discards or reconciles the branch. Simulated, predicted, draft, and authoritative
values must remain visibly and structurally distinct.

## 23.6 Forms, navigation, collaboration, and offline use

Forms are draft relations with reactive validation. Navigation is a semantic destination and
workspace graph whose browser URLs or native stacks are renderer encodings. Presence,
annotations, reviews, and shared selections use ordinary scoped relations.

Offline operation is explicit per relation and command. Reconnection reconciles retained
projections, local proposals, server revisions, and conflict relations. Universal transparent
merge is not promised.

## 23.7 Client tooling and performance

The client runtime supports indexed local relations, incremental evaluation, fine-grained
subscriptions, structural sharing, worker settlement, lazy projection loading, semantic
code splitting, binary graph encodings, and renderer notifications only when consumed
semantic results change.

Developer tools expose the current snapshot, local and remote edges, drafts, optimistic
branches, triggered rules, view dependencies, render changes, command lifecycle,
authorization, and subscription state. `why visible` and `why disabled` are queries over the
client self-model.

# Part XXIV — APIs, trust, privacy, and operations

## 24.1 Unified external APIs

A single declaration exports queries, commands, and watches across transports:

```brix
export api LogisticsApi {
  query OrderDetails
  command AssignVehicle
  watch OrderTimeline

  transports { http, websocket, sse, rpc }
}
```

Generated interfaces preserve typed errors, `SnapshotId`, revision cursors, transaction
intent, idempotency, authorization, pagination, compatibility, and observability. Typed SDKs
are generated for TypeScript, Rust, Python, Java, C#, Kotlin, Swift, and Dart according to
release profile.

## 24.2 Identity and authorization

The platform provides user, service, device, and workload principals; role- and
attribute-based policies; capability-based execution; tenant isolation; delegation;
separation of duties; and complete audit history.

Authorization applies independently to descriptor discovery, live data, history,
provenance, explanations, queries, commands, watches, LLM context, and operational metrics.
Client-side checks improve usability but never replace server enforcement.

## 24.3 Privacy and lifecycle

First-party privacy support includes classification, purpose limitation, consent, field and
role protection, retention, archival, legal holds, pseudonymization, redaction,
cryptographic erasure, export restrictions, model-training restrictions, derived-data taint,
and privacy-aware provenance and explanations.

Append-only logical history does not imply indefinite retention of plaintext sensitive data.
The lifecycle contract distinguishes logical retraction, historical tombstones, redaction,
key destruction, and physical compaction.

## 24.4 Configuration and secrets

Configuration, behavior-affecting deployment values, feature activation, and secrets are
typed and distinct from source. Secrets are opaque handles unavailable to settlement and
are resolved only by authorized Drivers. Changes are versioned and included in deployment
identity where they alter behavior.

## 24.5 Resource governance

Budgets cover settlement latency, CPU, memory, storage, query complexity, recursion,
simulation events, solver time, LLM tokens and cost, protocol concurrency, and tenant quotas.
Admission control, workload classes, cancellation, backpressure, and degraded operation are
first-class. Resource failure cannot publish a half-settled revision.

## 24.6 Deployment artifact and release lifecycle

A release is a content-addressed unit containing:

```text
program revision and schema
packages, bricks, ontologies, and formalisms
Drivers and component artifacts
prompts, learned models, policies, and solver profiles
configuration and capability manifest
migrations
quality and test evidence
SBOM, build provenance, licenses, and signatures
```

The lifecycle is:

```text
draft -> validated -> tested -> shadow -> canary -> active -> deprecated -> retired
```

`brix build`, `verify`, `shadow`, `deploy`, `canary`, `promote`, `rollback`, `recover`, and
`doctor` operate on this artifact model.

## 24.7 Recovery and continuity

The platform specifies checkpoints, revision-log backup, point-in-time recovery, replica
validation, corruption detection, protocol lease recovery, Driver crash recovery, program
and schema rollback, model and prompt rollback, cross-backend migration, and recovery
exercises. A backup identifies every artifact required to reconstruct one internally
consistent world.

## 24.8 Supply chain and registry

The official registry stores packages, bricks, Drivers, connectors, ontology packs,
formalisms, prompts, policies, models, UI themes, test fixtures, and assemblies. It supports
content addressing, signed publication, verified maintainers, compatibility metadata,
licenses, SBOMs, vulnerability advisories, yanking, LTS channels, and certification.

Certified runtimes, storage backends, Drivers, connectors, formalisms, and production
profiles pass public conformance suites.

## 24.9 Real-time transport (`brix.websocket`)

The client platform of Part XXIII rides a normative transport: one `export api`
declaration serves HTTP request/response and WebSocket live interaction. The socket
carries settled results, one logical `WatchDelta{from, to, added, removed}` per
published revision with SnapshotId on every graph event, typed transaction commands
(snapshot pins for optimistic concurrency; retries reuse transaction intent
identity), and protocol status — and never intra-settlement state. Resume presents a
token and either replays retained deltas or issues `Rebase(snapshot)`; silent
revision skipping does not exist. Backpressure is bounded and declared: default
`Rebase`; `CoalesceRevisions` only where the watch declared `coalesce` and the
coalesced delta equals the omitted sequence; event subscriptions never coalesce
without a declared loss policy; silent drops never. Authorization is per-operation
under §24.2 scoping and rechecked on resume, role change, activation, and policy
change. Connections negotiate versions and encodings (JSON + Arrow IPC in v1);
incompatible activations close subscriptions with typed errors, never silent
re-decode. `bind Endpoint to sim.websocket` runs the identical logical contract
deterministically inside scenarios; `realtime.*` sealed relations expose operations;
TypeScript and Rust clients generate first-party. Conformance I.24.

# Part XXV — The complete BrixMS platform

## 25.1 Product architecture

The professional ecosystem is delivered as five coherent products over one language model.

### Brix Language

```text
compiler, formatter, package manager, settlement engine
logic, mathematics, ontologies, simulations, policies, LLM boundaries
reflection, protocols, tests, quality, APIs, client runtime
```

### Brix Studio

```text
schema and ontology modeling
rule, proof, and provenance exploration
DES timelines, DEVS diagrams, stock-flow diagrams, ABM views
causal diagrams, workflow design, brick composition
client-view design, tests, quality, decisions, and impact analysis
```

### Brix Control

```text
deployments, environments, revisions, configuration, secrets
capabilities, identity, tenancy, budgets, observability
shadow, canary, promotion, rollback, backup, and recovery
```

### Brix Registry

```text
packages, domain bricks, connectors, ontologies, formalisms
prompts, models, themes, assemblies, certification, and LTS channels
```

### Brix Lab

```text
snapshot-pinned notebooks, relation-frame exploration, cleaning and feature design
statistical modeling, training, tuning, experiments, simulation, and causal analysis
policy and LLM evaluation, model comparison, visualization, and reproducible reports
```

## 25.2 Standard distribution

The v6 distribution contains:

```text
brix, brixc, brixd, brixfmt, brixpkg
brix-engine reference memory backend
brix test, quality, doc, why, whynot, inspect, graph, profile
brix sim, lab, data, profile, clean, features, train, tune, evaluate, models
brix compat, diff, impact, migrate
brix client and semantic render runtime
Brix Model Studio foundation
Brix Control foundation
```

Reference integrations include Arrow, Parquet, CozoDB, PostgreSQL, Lakebase, Delta Lake,
Databricks, DuckDB, DataFusion, Wasmtime Components, OpenTelemetry, Z3, HiGHS, ONNX Runtime,
Kafka, OpenDAL, OCI, and Sigstore. These integrations do not define the language semantics.

## 25.3 Complete modern-language contract

BrixMS v6 is considered complete at the language level because it provides:

```text
a coherent type and effect system
a deterministic reactive hypergraph computation model
versioned state, temporal semantics, and transactions
logic, mathematics, uncertainty, optimization, learning, and native data science
native ontology and multimethod simulation
safe external boundaries and structured Driver concurrency
mandatory reflection and executable documentation
first-class testing, quality, profiling, and debugging
semantic component and package composition
reactive client rendering and live transport contracts
security, privacy, deployment, recovery, and supply-chain contracts
model validity, correction, interchange, consistency, failure, and reproducibility contracts
a complete everyday language, analytical and ML platform, standard library, formal oracle,
and developer feedback loop
```

Future editions may extend algorithms, scale, formalisms, renderers, and integrations without
adding competing core execution paradigms.

## 25.4 Final design laws

1. **The model is the program.** Domain meaning is not hidden behind execution machinery.
2. **Settlement is the computational center.** Alternate formalisms lower to the same
   revisions, relations, transactions, and boundaries.
3. **Production is coupled simulation.** Real and simulated histories differ by boundary
   outcomes, not internal semantics.
4. **Truth status is typed.** Proof, estimate, probability, suggestion, interpretation,
   simulation, and authority are never silently conflated.
5. **The system models itself.** Structure, documentation, tests, quality, ownership, and
   operation are queryable semantic relations.
6. **A component is a semantic brick.** Domain and interaction models compose through one
   port, ontology, ownership, and compatibility system.
7. **The client is a local model.** Rendering frameworks project a reactive authorized
   hypergraph rather than owning a second application truth.
8. **Authority is explicit.** Discovery, intelligence, and recommendation do not grant
   permission to mutate reality.
9. **Every change has identity and history.** Data, programs, models, prompts, policies,
   deployments, and decisions are immutable versions.
10. **The ecosystem preserves semantics.** Backends, Drivers, renderers, and tools may
    optimize execution but cannot redefine meaning.
11. **Every model has a validity boundary.** Applicability, assumptions, coverage, and
    degradation are part of the result rather than prose outside it.
12. **Reality may correct the model.** Corrections create new revisions while preserving
    both current corrected history and what was known then.
13. **Failure is epistemic.** Unknown external outcome, cancellation, timeout, rejection,
    and confirmed failure are never collapsed into one status.
14. **Reproducibility is declared honestly.** Canonical, replayable, statistical,
    artifact-dependent, provider-dependent, and exploratory work are distinct.
15. **Analysis is model execution.** Cleaning, features, datasets, experiments,
    training, predictions, and evaluation are versioned projections and scenario branches of
    the same living model, never a detached analytical truth.

## 25.5 v6 guarantee

BrixMS guarantees that:

1. every observable application state is a settled graph projection;
2. every conclusion identifies whether it was deduced, calculated, estimated, predicted,
   simulated, suggested, interpreted, or authorized;
3. every active system has a complete versioned semantic self-model;
4. every component can package domain, behavior, intelligence, and interaction together;
5. cross-component dependencies are explicit and semantically checked;
6. client interfaces update from atomic revision deltas and preserve authority boundaries;
7. real-world observations retain source, time, quality, and reconciliation history;
8. decisions retain alternatives, assumptions, simulations, objectives, authority, and
   outcomes;
9. deployments are reproducible, governable, observable, and recoverable;
10. models expose validity envelopes, assumption breaches, and reality divergence;
11. corrections preserve bitemporal truth and historical belief;
12. transaction, consistency, failure, cancellation, and reproducibility semantics are explicit;
13. canonical interchange and executable conformance make ecosystem compatibility testable;
14. data preparation, feature computation, training, prediction, and evaluation remain
    snapshot-bound, provenance-bearing operations over the same model used in production and
    simulation;
15. the language remains centered on one idea: software is an executable simulation model
    of reality.

> **BrixMS is a language and ecosystem for assembling living, intelligent, accountable
> models of reality from context-independent semantic bricks.**


# Part XXVI — Completion contracts for a trustworthy living model

## 26.1 Why completion contracts are normative

The preceding parts define what BrixMS can model and compute. This part defines the
conditions under which those models and computations may be trusted, exchanged, corrected,
reproduced, deployed, and maintained.

These contracts are not optional operational advice. They complete the language thesis.
Software that models reality must represent not only its conclusions but also:

```text
where the model applies
which assumptions are currently satisfied
which observations may later be corrected
which historical beliefs were held at each revision
which operations are exact, replayable, statistical, or provider-dependent
which external outcomes are known, failed, cancelled, or uncertain
which consistency guarantees apply to shared state
which trust boundaries apply to code, data, models, and people
```

A production profile may reject activation when any required completion contract is absent.

## 26.2 Model-validity envelopes

Every predictive, causal, simulation, optimization, learned-policy, numerical, or
language-model artifact may declare a validity envelope.

```brix
model contract DeliveryRiskValidity {
  applies to DeliveryRiskModel

  validFor {
    geography in EuropeanUnion
    planningHorizon <= 14 days
    vehicleClass in SupportedVehicleClass
  }

  assumes {
    TrafficObservationCoverage >= 0.95
    RouteNetworkAge <= 24 hours
    HistoricalCalibrationWindow >= 90 days
  }

  requires {
    OrderWeight(order)
    PlannedRoute(order)
    CurrentVehicleState(vehicle)
  }

  outOfDomain when {
    ExtremeWeatherAlert(region)
    UnknownVehicleClass(vehicle)
  }
}
```

Evaluation produces an explicit domain status:

```brix
enum ModelDomainStatus {
  Valid
  Degraded { findings: Set<ModelFinding> }
  OutOfDomain { reasons: Set<ModelFinding> }
  InsufficientObservation { missing: Set<RequirementRef> }
  AssumptionBreached { assumptions: Set<AssumptionRef> }
  Stale { through: ValidTime }
  Uncalibrated { reason: CalibrationFinding }
}
```

A result carries both value and applicability:

```brix
record ApplicableResult<T> {
  result: T
  status: ModelDomainStatus
  envelope: ModelContractRef
  evaluatedAt: SnapshotId
  evidence: Set<EvidenceRef>
}
```

A degraded or out-of-domain result does not satisfy a premise requiring a valid result unless
an explicit rule accepts that status. Confidence alone cannot override an applicability
failure.

Model health evaluates validity envelopes continuously and derives assumption breaches,
coverage gaps, calibration drift, and reality divergence.

## 26.3 Corrections, retroactive truth, and historical belief

Observations and accepted claims may later be corrected. BrixMS preserves two distinct
questions:

1. What does the current model believe was valid at a past real-world time?
2. What did the system believe at each historical transaction revision?

The first is answered through valid time. The second is answered through transaction time
and revision history.

```brix
event rel ObservationCorrected {
  key id: CorrectionId
  original: ObservationId
  replacement: Option<ObservationId>
  reason: CorrectionReason
  correctedAt: TransactionTime
  authority: Principal
}
```

Corrections never mutate an old revision. They commit new claims and supersession or
retraction relations.

A correction policy declares its consequence:

```brix
correction policy OperationalHistory {
  derivedHistory Recompute
  publishedReports MarkSuperseded
  executedProtocols PreserveOutcome
  trainingDatasets RepairLineage
  externalNotifications EmitCorrection
}
```

The standard modes are:

```text
Recompute
  Re-settle affected valid-time projections under the current correction revision.

PreserveAsKnownThen
  Preserve historical conclusions as facts about what was known then.

MarkSuperseded
  Retain an artifact but identify the correcting successor.

NoAutomaticExternalCompensation
  Record that reality was acted upon; require an explicit compensating decision.
```

Protocol outcomes that occurred in the real world are never erased by data correction.
Instead, the graph may derive that an action was based on information later corrected.

Queries and reports must identify whether they use:

```text
current corrected history
knowledge as of a named revision
original source history
```

## 26.4 Canonical Brix interchange

BrixMS defines a vendor-neutral interchange family:

```text
BGIF  — Brix Graph Interchange Format
BRDP  — Brix Revision Delta Protocol
BCMF  — Brix Component Manifest Format
BPEF  — Brix Proof and Evidence Format
BRTF  — Brix Render and Interaction Format
```

Each format has a canonical binary encoding and a canonical JSON diagnostic encoding.
Arrow mappings are normative for columnar relation batches.

Canonical interchange defines:

- type and schema fingerprints;
- canonical role ordering;
- normalized identifiers and strings;
- deterministic collection ordering;
- exact numeric encoding;
- unit, currency, temporal, and ontology identifiers;
- entity, edge, claim, and evidence references;
- snapshot and program revision identities;
- insertion, retraction, mask, and unmask deltas;
- proof and provenance records;
- brick ports, capabilities, and compatibility surfaces;
- semantic render nodes, bindings, and actions;
- extension namespaces and unknown-field preservation.

A digest over a canonical artifact must be identical across conforming implementations.
A noncanonical transport may be used internally, but exported semantic identity is computed
from the canonical representation.

```brix
interchange LogisticsFeed {
  format BRDP
  schema LogisticsProjection@3
  encoding CanonicalBinary
  compression Zstd
  integrity SignedDigest
}
```

## 26.5 Complete everyday language and standard library

The advanced model architecture does not excuse gaps in ordinary programming. The stable
language and standard distribution provide a complete baseline for routine work.

### Core language facilities

```text
algebraic data types and exhaustive pattern matching
generics and constraints
traits with coherent implementation resolution
modules, visibility, imports, exports, and editions
immutable collections and iterators
validated constructors and refinement newtypes
structured typed errors and the ? operator
resource scopes, cleanup, cancellation, and deadlines in Drivers
hygienic compile-time generation with visible lowering
documentation comments and executable examples
```

### Standard library domains

```text
Unicode strings, normalization, segmentation, and collation
regular expressions and parsers
bytes, buffers, binary codecs, and checksums
dates, times, calendars, durations, and IANA time zones
UUIDs, URIs, media types, IP addresses, and network endpoints
persistent maps, sets, sequences, queues, heaps, and ordered collections
JSON, JSON Lines, CSV, XML, MessagePack, CBOR, and Arrow codecs
compression and archive interfaces
cryptographic hashes, signatures, key interfaces, and secure randomness for Drivers
logging, diagnostics, metrics, and tracing
filesystem, process, network, and environment interfaces restricted to Drivers
```

Cryptographic algorithms and platform resources are exposed through versioned capability
interfaces. Application rules never gain ambient filesystem, process, network, secret, or
random access.

## 26.6 Transaction and consistency profiles

The language defines transaction isolation precisely.

```text
Serializable
  The committed history is equivalent to a serial transaction order. Predicate and
  read/write conflicts are detected.

Snapshot
  Reads observe one settled snapshot. Declared write skew may occur and must be guarded by
  strict constraints where prohibited.

ExpectedRevision
  A command commits only if named relations or the whole snapshot match the expected
  revision.
```

A transaction declares or inherits its isolation level:

```brix
transaction AssignVehicle
  isolation Serializable
  intent request.intent
{
  ...
}
```

The specification defines:

- conflict detection;
- retry-stable fresh identity;
- idempotent intent handling;
- strict-constraint evaluation over the fully settled candidate revision;
- nested transaction rejection or explicit flattening;
- command supersession;
- protocol outcome races;
- maximum retry and starvation behavior.

Distributed and offline state uses explicit consistency profiles:

```brix
consistency FleetState {
  ownership Region
  ordering Consensus
  conflict Reject
}

consistency DeviceObservation {
  ownership Device
  ordering Causal
  merge Append
}
```

A profile declares:

```text
ownership: Single | Principal | Partition | Shared
ordering: Local | Causal | Consensus | External
merge: Reject | Append | LastWriterByDeclaredClock | JoinSemilattice | CustomVerified
availability: OnlineRequired | OfflineReadable | OfflineProposable
```

No relation is eventually consistent merely because it is distributed. Recursive global
settlement may require coordination or a declared partition-local approximation whose
status is not presented as canonical truth.

## 26.7 Failure, cancellation, and uncertain external outcomes

All long-running and boundary operations use a common terminal model:

```brix
enum OperationOutcome<T, E> {
  Succeeded(T)
  Failed(E)
  TimedOut(Deadline)
  Cancelled(CancellationRef)
  ResourceExhausted(ResourceFinding)
  ProviderUnavailable(ProviderFinding)
  UnknownOutcome(ReconciliationRef)
}
```

`UnknownOutcome` means that an external effect may have happened but the Driver could not
establish the result. It is neither success nor failure.

Every effectful request carries:

```text
stable request version
retry-stable intent identity
idempotency key where supported
attempt identity
lease
cancellation token
deadline
reconciliation strategy
compensation policy
```

A Driver that crashes after an external success must not cause blind duplication. The
protocol enters `UnknownOutcome` and may execute a declared reconciliation query.

```brix
protocol TransferFunds {
  reconcile using LookupTransferByIdempotencyKey
  compensate using ReverseTransfer
  retry only when DefinitelyNotApplied
}
```

Cancellation is cooperative and recorded. It propagates across workflows, protocol
attempts, numerical solvers, simulations, policy evaluation, language workflows, and client
requests. A cancellation request does not falsely claim that an already completed external
action was undone.

## 26.8 Reproducibility tiers

Every computation that may depend on numerical implementation, external providers,
stochastic execution, or learned artifacts declares a reproducibility tier.

```brix
enum ReproducibilityTier {
  Canonical
  Replayable
  StatisticallyReproducible
  ArtifactDependent
  ProviderDependent
  Exploratory
}
```

### Canonical

Identical semantic and numeric result across conforming implementations under the declared
profile.

### Replayable

The recorded boundary outcomes, seeds, artifacts, and revision history reproduce the run
without recalling live providers.

### Statistically reproducible

Repeated runs satisfy declared distributional, calibration, or confidence bounds rather
than requiring byte identity.

### Artifact dependent

Reproduction requires the exact identified local model, solver, or numerical artifact.

### Provider dependent

The result depends on an identified external provider whose behavior cannot be frozen fully.

### Exploratory

Only inputs, outputs, evidence, and environment are recorded; no stronger guarantee is made.

A result may not claim a stronger tier than every dependency it consumed unless a verifier
converts the result into a stronger independently checked artifact.

## 26.9 Executable normative specification

The prose specification is accompanied by first-party normative artifacts:

```text
complete grammar
name-resolution rules
type, trait, effect, and refinement judgments
rule safety and phase-inference rules
settlement and revision transition semantics
identity and canonical-encoding algorithms
transaction conflict model
protocol lifecycle automata
client-delta application semantics
standard meta schema
reference evaluator
conformance fixtures and randomized generators
```

The reference evaluator is designed for semantic clarity rather than performance. Optimized
runtimes, storage backends, client engines, and formalism packages are checked against it by
differential execution.

A specification feature is not considered stable until it has:

1. normative syntax;
2. static semantics;
3. runtime semantics;
4. canonical encoding where externally observable;
5. diagnostics requirements;
6. conformance tests;
7. migration and compatibility rules.

## 26.10 Professional developer feedback loop

The standard toolchain provides:

```text
brix new
brix build
brix run
brix repl
brix watch
brix test
brix sim
brix explain
brix why
brix whynot
brix inspect
brix graph
brix diff
brix impact
brix doctor
```

Development mode includes:

- hot activation through new program revisions;
- semantic rather than textual diffs;
- synthetic observation generation;
- scenario and Driver switching;
- local protocol simulation;
- client hypergraph and render-graph inspection;
- live React and renderer refresh;
- automatic failing-case minimization;
- proof, phase, capability, and ownership explanations;
- one-command local reference-runtime startup.

Diagnostics must state the violated semantic contract and show the smallest relevant
program path.

```text
Rule EffectivePrice reads the absence of ManualPrice.
ManualPrice is externally open and has no completeness witness for client C-104.
The rule therefore cannot be assigned a valid settlement phase.
```

## 26.11 Trust profiles and threat model

Every source of code, data, inference, and authority may carry a trust profile:

```brix
enum TrustClass {
  CertifiedArtifact
  TrustedLocalCode
  SandboxedPackage
  ExternalService
  LearnedArtifact
  HumanAssertion
  UntrustedContent
  CompromisedOrRevoked
}
```

Trust is not truth. It determines required validation, isolation, evidence, and authority.

The platform threat model covers at minimum:

```text
malicious or compromised packages and Drivers
capability escalation and confused deputies
cross-tenant data and inference leakage
prompt injection and tool manipulation
poisoned observations and training data
malicious ontology alignment or identity merge
provenance, proof, and signature forgery
model extraction and sensitive-context disclosure
expensive-query, solver, simulation, and LLM denial of service
client graph tampering and optimistic-state confusion
supply-chain substitution and rollback attacks
```

A brick manifest declares its trust and sandbox requirements. Revocation produces explicit
operational and model-health findings and may block activation or further protocol use.

## 26.12 Scope closure

With this part, BrixMS is complete in conceptual language scope.

Future work should improve:

```text
algorithms
performance
scale
formalism libraries
domain bricks
renderer adapters
connectors
verification strength
```

Future work should not add a competing execution substrate, hidden mutable component model,
second application truth, or implicit authority mechanism.

The design is complete when the implementation can demonstrate that the same small semantic
foundation supports the full ecosystem without exceptions.

## 26.13 Completion guarantee

BrixMS guarantees that:

1. every model may state where it is valid and why it may be degraded;
2. corrections preserve both current corrected truth and historical belief;
3. exported graphs, revisions, bricks, proofs, and render models have canonical interchange;
4. routine programming is supported by a complete, capability-safe standard library;
5. transaction isolation and distributed consistency are explicit;
6. uncertain external outcomes are never collapsed into ordinary failure;
7. every intelligent or numerical computation declares an honest reproducibility tier;
8. stable features have executable normative semantics and conformance tests;
9. developers receive semantic diagnostics and a complete local feedback loop;
10. trust, isolation, and threat assumptions are represented as part of the living model.

> **A living model is professional only when it knows where it applies, how it may be
> corrected, how its results can be reproduced, how its actions can fail, and which trust
> boundaries surround every observation, component, inference, and effect.**


# Part XXVII — Native data science and machine learning

## 27.1 Data science is model introspection

Data science is not a separate application tier and does not create a second representation
of the organization. It is a disciplined way for the living model to inspect its history,
measure its agreement with reality, construct counterfactual populations, learn reusable
artifacts, and return predictions and proposed improvements to the same graph.

The foundational equivalence is:

```text
production state       = a settled history coupled to real boundary outcomes
simulation dataset     = a settled history coupled to scenario boundary outcomes
analytical dataset     = a typed projection of one or more settled histories
training run           = a governed experiment over those projections
prediction             = a versioned claim about a subject at an observation time
model evaluation       = comparison of that claim with later observations
```

Therefore simulation and analytical modeling share:

- entities and relation identities;
- valid, transaction, simulation, and correction time;
- ontologies and units;
- provenance and evidence;
- model-validity envelopes;
- scenario and random-stream identity;
- authorization and privacy;
- reproducibility profiles;
- the reflexive self-model.

There is no extract-transform-forget semantic path. Physical exports may accelerate work,
but the exported artifact retains the identity of the model projection from which it came.

> **The simulation is the model, and data science is the model examining and improving its
> own correspondence with reality.**

## 27.2 Relation frames

BrixMS provides `Frame<S>` as a finite, typed, columnar projection of `Rel<S>`.

```brix
let orders: Frame<{
  order: Order
  client: Client
  weight: Quantity<Mass>
  due: Instant
  risk: Probability
}> =
  frame from {
    order: Order { client, weight, due }
    LateRisk(order, risk)
  }
```

A frame is not a second mutable dataframe model. It is:

- bound to a `SnapshotId` or explicit revision range;
- typed by a row schema;
- unordered unless an order is declared;
- lazily executable where possible;
- backed by Arrow-compatible columnar buffers where useful;
- provenance-preserving;
- convertible back to relation values when identity and evidence are retained.

The standard manipulation grammar includes:

```text
select, rename, filter, derive, group, summarize, arrange, distinct
join, semijoin, antijoin, asofJoin, union, intersect, difference
pivotLonger, pivotWider, nest, unnest, window, sample
```

Example:

```brix
let clientRisk =
  orders
    |> filter(|row| row.risk >= 0.5)
    |> group(by: [.client])
    |> summarize {
         openOrders: count()
         totalWeight: sum(.weight)
         maximumRisk: max(.risk)
       }
    |> arrange(.maximumRisk descending)
```

Any operation whose result depends on order, time, partitioning, or approximation declares
that dependency explicitly.

## 27.3 Typed missingness and data quality

BrixMS does not use one ambiguous missing-value sentinel.

```brix
enum Missing<T> {
  Present(T)
  NotObserved
  NotApplicable
  Unknown
  Redacted
  Invalid(DataFinding)
  Pending
}
```

These states are semantically distinct. In particular:

- `Redacted` cannot be imputed without an explicit privacy-authorized policy;
- `NotApplicable` is not evidence that a value is unknown;
- `Invalid` retains the rejected observation and validation evidence;
- `Pending` may become available through a protocol outcome;
- `NotObserved` contributes to source-coverage and model-health calculations.

Profiles are versioned model structures:

```text
data.Profile
 data.ColumnProfile
 data.MissingnessProfile
 data.DistributionProfile
 data.CardinalityProfile
 data.OutlierCandidate
 data.DuplicateCandidate
 data.SchemaDrift
 data.ValueDrift
 data.TemporalCoverage
 data.SourceCoverage
```

An approximate profile records its sampling method, error bounds, seed, and execution
engine. Profiling never silently promotes a candidate outlier or duplicate to a correction.

## 27.4 Immutable cleaning and preparation recipes

Data cleaning is represented by immutable recipes:

```brix
data recipe CleanCarrierOrders {
  input RawCarrierOrder

  step parse Timestamp from occurredAt
  step normalize Weight to kilograms
  step trim carrierReference
  step map carrierCode using CarrierCodeMapping
  step validate destination against KnownLocation
  step detectDuplicates by eventId
  step quarantine when severity >= Error

  output CleanCarrierOrder
}
```

A recipe records:

- input dataset and snapshot;
- ordered transformation steps;
- learned preparation parameters;
- reference datasets;
- accepted output;
- quarantined and rejected observations;
- quality findings;
- lineage;
- recipe version.

Common steps include parsing, casting, normalization, standardization, scaling, imputation,
winsorization, clipping, binning, encoding, tokenization, lags, differences, rolling
features, aggregation, deduplication, identity resolution, validation, and quarantine.

Any step that learns parameters has separate `fit` and `apply` phases. A recipe fitted on a
training partition cannot observe assessment, test, or future partitions.

## 27.5 Factors and statistical formulas

Categorical values use explicit factor types:

```brix
factor ServiceLevel {
  Economy
  Standard
  Express
  Critical
}

ordered factor RiskBand {
  Low < Moderate < High < Critical
}
```

A factor declares allowed levels, ordering, unknown-level behavior, ontology mapping,
reference level, labels, and encoding policy. New levels cannot silently change the feature
layout of a trained artifact.

BrixMS provides a typed statistical formula surface:

```brix
statistical model LateDeliveryModel {
  outcome Late

  formula {
    Late ~ Distance
         + Weight
         + ServiceLevel
         + WeatherSeverity
         + Distance:WeatherSeverity
  }

  family Binomial
  link Logit
}
```

Formula terms resolve to semantic roles with units, factor levels, observation time, and
lineage. The standard surface supports main effects, interactions, polynomial and spline
terms, offsets, strata, random effects, nested effects, time-varying effects, and survival
terms.

## 27.6 Feature semantics

A feature is a first-class reflected declaration:

```brix
feature ClientLateRate(client: Client)
  -> Estimate<Probability> {
  observationTime dispatchTime
  window 365 days
  source CompletedDelivery
  leakage SafeBeforeLabel
}
```

Every feature declares:

- subject and semantic definition;
- source relations;
- observation-time policy;
- aggregation window;
- valid-time interpretation;
- missingness behavior;
- unit and factor domain;
- leakage classification;
- training and serving availability;
- freshness and validity envelope;
- owner.

Feature values are explicit records:

```brix
record FeatureValue<T> {
  subject: EntityRef
  feature: FeatureRef
  value: Missing<T>
  asOf: ValidTime
  computedAt: TransactionTime
  snapshot: SnapshotId
}
```

A feature set is a versioned contract shared by offline training, batch prediction, online
inference, simulation, explanation, and monitoring:

```brix
feature set DeliveryRiskFeatures @ 3 {
  entity Order

  include {
    DaysUntilDue
    RouteDistance
    CurrentTrafficSeverity
    ClientLateRate
    VehicleReliability
  }

  target WasDeliveredLate
}
```

Materialized feature stores are caches and indexes. They do not own an independent feature
definition or application truth.

## 27.7 Immutable datasets and temporal correctness

A dataset is an immutable snapshot- or history-bound artifact:

```brix
dataset LateDeliveryTraining {
  source DeliveryRiskFeatures
  observationTime OrderDispatchTime
  labelTime OrderDeliveredTime

  include where { OrderCompleted() }
  exclude where { InvalidTrainingExample() }
}
```

Dataset identity includes:

- program revision;
- source snapshot or revision range;
- query, feature, and recipe versions;
- observation-time and label-time policies;
- sampling and authorization scopes;
- row identities and content digest;
- correction policy;
- reproducibility profile.

A dataset may represent actual history, a scenario population, a counterfactual branch, a
synthetic population, or a mixture. This origin is explicit and cannot be erased by export.

The standard resampling package includes random, stratified, grouped, temporal,
rolling-origin, blocked, leave-one-group-out, k-fold, repeated cross-validation, bootstrap,
and permutation schemes.

The quality engine detects:

```text
future leakage
label leakage
entity and group leakage
duplicate examples across partitions
preprocessing fitted before splitting
target-derived features
correction leakage
simulation-to-production population confusion
```

## 27.8 Estimator and workflow contracts

All statistical and machine-learning engines implement one typed estimator protocol:

```brix
protocol Estimator<Features, Target, Artifact> {
  fit(
    training: TrainingSet<Features, Target>,
    configuration: TrainingConfiguration
  ) -> TrainingOutcome<Artifact>

  predict(
    artifact: Artifact,
    examples: Frame<Features>
  ) -> PredictionSet<Target>
}
```

Optional capabilities include probability and distribution prediction, transformation,
partial fitting, explanation, feature importance, gradients, uncertainty, and portable
export. Capabilities are declared explicitly.

A modeling workflow composes preparation, estimation, calibration, postprocessing, and
evaluation:

```brix
ml workflow DeliveryRiskWorkflow {
  data LateDeliveryTraining

  preprocess CleanAndEncodeDeliveryData
  model GradientBoostedClassifier
  calibrate Isotonic
  threshold CostSensitiveThreshold

  evaluate {
    ROC_AUC
    PR_AUC
    LogLoss
    BrierScore
    CalibrationError
    CostAtThreshold
  }
}
```

The fitted workflow, not only the final estimator, is the immutable `ModelArtifact`. This
ensures training-serving consistency.

## 27.9 Statistical and machine-learning families

The first-party contract covers:

```text
classical statistics
  linear and generalized linear models
  regularized and robust regression
  mixed-effects and longitudinal models
  survival, time-series, state-space, and multivariate models

classical machine learning
  trees, forests, boosting, nearest neighbors, support vectors
  naive Bayes, clustering, dimensionality reduction, anomaly detection
  recommendation and ranking

probabilistic modeling
  Bayesian and hierarchical models, mixtures, Gaussian processes
  latent-variable and probabilistic graphical models

deep and graph learning
  dense, convolutional, sequence, transformer, graph-neural, and multimodal models
```

The standard does not require every algorithm to execute in the settlement engine. It
requires every engine to preserve the estimator, artifact, prediction, evidence, validity,
and deployment contracts.

## 27.10 Tuning and experiments

Hyperparameter and workflow tuning are governed experiments:

```brix
tuning DeliveryRiskTuning {
  workflow DeliveryRiskWorkflow

  vary {
    learningRate in logRange(0.001, 0.3)
    maximumDepth in 2..12
    minimumLeafSize in 10..500
  }

  optimize {
    maximize ROC_AUC
    minimize CalibrationError
    minimize InferenceCost
  }

  budget {
    trials <= 200
    totalCompute <= 500 core hours
  }
}
```

Supported search strategies include grid, random, Bayesian, successive halving,
population-based, and multi-objective search.

An experiment records:

```text
program and package revisions
dataset, split, feature-set, and recipe identities
estimator and hyperparameters
random streams and scenario identities
execution environment and hardware profile
metrics, artifacts, warnings, cost, and duration
```

Experiment results are ordinary graph relations and can be compared, queried, documented,
reviewed, or used by an activation workflow.

## 27.11 Model registry and activation

The self-model includes:

```text
ml.Model
ml.ModelVersion
ml.ModelArtifact
ml.TrainingRun
ml.Evaluation
ml.Alias
ml.Stage
ml.Approval
ml.Deployment
ml.Retirement
```

The lifecycle is:

```text
candidate -> validated -> shadow -> canary -> active -> deprecated -> retired
```

Activation is a transaction. Training never mutates the active model in place. Historical
predictions retain the exact model, feature set, recipe, prompt or policy dependencies, and
validity envelope that produced them.

## 27.12 Predictions, calibration, and decision thresholds

Predictions are typed relation values:

```brix
record Prediction<T> {
  subject: EntityRef
  value: T
  uncertainty: Option<Estimate<T>>
  model: ModelVersionRef
  features: FeatureVectorRef
  asOf: ValidTime
  producedAt: TransactionTime
  applicability: ApplicabilityStatus
}
```

A prediction remains distinct from an observation, accepted claim, derivation, simulation,
or authorized state.

Probability calibration and decision thresholds are independently versioned artifacts:

```brix
decision threshold EscalateLateRisk {
  prediction DeliveryLateProbability
  threshold 0.82

  objective {
    falseNegativeCost: 500 EUR
    falsePositiveCost: 25 EUR
  }
}
```

Thresholds can be simulated, tested, approved, and changed without retraining the estimator.

## 27.13 Evaluation, explainability, and fairness

Evaluation includes classification, regression, ranking, clustering, calibration, forecast,
survival, decision-cost, fairness, robustness, latency, memory, energy, and financial-cost
metrics.

```brix
record MetricResult<T> {
  metric: MetricRef
  value: T
  interval: Option<Interval<T>>
  population: PopulationRef
  sampleSize: Nat
  method: EvaluationMethod
}
```

Critical models require subgroup and validity-envelope evaluation.

Explainability interfaces include global importance, local attribution, partial dependence,
accumulated local effects, counterfactual explanations, exemplars, rule extraction,
uncertainty explanation, and training-data influence. Every explanation records its
algorithm, background population, assumptions, and approximation status. An explanation is
not a causal proof unless produced and verified under a causal formalism.

## 27.14 Monitoring and learning from reality

Active models are evaluated against later observations through the same correction-aware
history used by the rest of BrixMS.

The standard monitor relations include:

```text
ml.FeatureDrift
ml.LabelDrift
ml.PredictionDrift
ml.ConceptDrift
ml.CalibrationDrift
ml.PerformanceDrift
ml.MissingnessDrift
ml.CategoryDrift
ml.LatencyDrift
ml.CostDrift
ml.OutOfDomainRate
ml.RetrainingRecommendation
```

Monitoring is model health. Retraining produces a candidate model version; it never
silently replaces the active artifact.

The complete learning loop is:

```text
observation -> accepted model state -> feature snapshot -> prediction
          -> decision -> action -> later observation -> evaluation
          -> drift or error evidence -> candidate training run -> governed activation
```

## 27.15 Declarative visualization and reports

BrixMS includes a grammar-of-graphics-style visualization model:

```brix
visualization LateRiskByDistance {
  data DeliveryEvaluationFrame

  map {
    x RouteDistance
    y PredictedLateProbability
    color ServiceLevel
  }

  layer points
  layer smooth(method: Logistic)
  facet by Region
}
```

The visualization grammar supports statistical transforms, uncertainty bands, graph and
hypergraph layers, provenance, temporal revisions, simulation comparisons, ontology-aware
labels, and interactive filtering.

Visualizations are semantic bricks and may render through React, SVG, Canvas, WebGL,
notebooks, reports, or terminal summaries.

Brix Lab reports combine narrative, queries, tables, statistics, visualizations, models,
simulation runs, evidence, and citations. Every output cell retains its program revision,
snapshot, dataset, seed, recipe, artifact, numerical profile, and execution environment.

## 27.16 Python and R interoperability

Python and R are premium analytical boundaries, not alternate BrixMS semantics.

First-party Python packages include:

```text
brix.python
brix.python.arrow
brix.python.numpy
brix.python.pandas
brix.python.polars
brix.python.sklearn
brix.python.statsmodels
brix.python.pytorch
brix.python.jax
```

First-party R packages include:

```text
brix.r
brix.r.arrow
brix.r.tidyverse
brix.r.tidymodels
brix.r.statistical
```

The primary data bridge is Arrow-compatible columnar interchange. A Python or R job receives
only the declared projection and capabilities. The returned result includes environment and
package locks, code and artifact digests, warnings, logs, metrics, and lineage.

Language-native opaque serialization may be retained for debugging, but it is not the sole
production contract. A production artifact must expose a typed BrixMS input/output contract
or a supported portable artifact.

Python and R may execute in Brix Lab, isolated Drivers, managed training jobs, Databricks, or
other declared compute environments. They never execute inside settlement and never gain
ambient graph access.

## 27.17 Portable model artifacts

The first-party ecosystem supports ONNX and other certified portable artifact formats behind
`brix.ml.artifact`.

An imported artifact declares:

```text
input feature contract
output contract
operator and format version
artifact digest
training lineage
hardware requirements
numerical and reproducibility profile
validity envelope
```

The portable artifact is an implementation of a BrixMS model contract. It is not the
semantic model itself.

## 27.18 Data-science bricks

Data-science assets are ordinary semantic bricks:

```brix
brick DeliveryRiskIntelligence {
  feature set DeliveryRiskFeatures
  data recipe CleanDeliveryData
  dataset DeliveryRiskTraining
  ml workflow DeliveryRiskWorkflow
  visualization CalibrationDashboard
  decision threshold EscalateLateRisk
  view RiskExplanation
  monitor DeliveryRiskMonitoring
}
```

The brick may be composed with the operational domain, simulation, workflow, UI, and
monitoring bricks through typed ports. There is no separate ML application glued onto the
living model.

## 27.19 First-party packages

```text
brix.data             relation frames and typed manipulation
brix.data.profile     profiling, coverage, distributions, drift and diagnostics
brix.data.clean       preparation recipes, imputation, validation and quarantine
brix.data.quality     expectations, reconciliation and quality evidence
brix.features         features, feature sets, observation time and lineage
brix.datasets         immutable datasets, splits and resampling
brix.stats            formulas, inference, diagnostics and statistical models
brix.ml               estimator, workflow, artifact, prediction and registry contracts
brix.ml.classical     first-party classical machine-learning algorithms
brix.ml.probabilistic probabilistic and Bayesian engine contracts
brix.ml.deep          deep-learning training and inference contracts
brix.ml.tune          workflow and hyperparameter optimization
brix.ml.explain       explanation and attribution methods
brix.ml.monitor       drift, calibration and model-health monitoring
brix.experiment       experiment tracking and evidence bundles
brix.viz              declarative statistical, graph and simulation visualization
brix.report           reproducible analytical reports
brix.python           sandboxed Python interoperability
brix.r                sandboxed R interoperability
brix.onnx             portable model validation, import, export and inference
```

## 27.20 Deliberate non-features

BrixMS does not adopt:

```text
mutable dataframes as the default semantic object
implicit numeric or categorical coercion
one ambiguous missing-value sentinel
hidden row ordering
notebook state as the only source of truth
training preparation outside the model artifact
unsafe pickles as the production interoperability contract
duplicated training and serving feature definitions
metrics without dataset and population identity
models without validity envelopes
automatic retraining or activation without authority
arbitrary Python or R execution inside settlement
```

## 27.21 Version-one premium data-science commitment

The premium v6 distribution includes:

```text
Frame<S> and Arrow interchange
typed data manipulation and missingness
profiling, cleaning recipes, quality findings and quarantine
factors, formulas and statistical diagnostics
feature definitions and versioned feature sets
immutable datasets and leakage-safe temporal splits
cross-validation and bootstrap resampling
linear, logistic, regularized and tree-based models
random forests, gradient boosting, clustering and dimensionality reduction
common evaluation, calibration and fairness metrics
workflow tuning and experiment tracking
model registry, shadow and canary deployment
model drift and validity monitoring
declarative visualization and reports
Python and R analytical Drivers
scikit-learn, statsmodels, tidyverse and tidymodels adapters
ONNX validation and inference
```

Specialized scientific packages, distributed training, advanced Bayesian computation, and
deep-learning training may be external engines while obeying the same contracts.

## 27.22 Data-science guarantee

BrixMS guarantees that:

1. every analytical frame and dataset names the snapshot or history it projects;
2. cleaning and preparation create versioned transformations without destroying evidence;
3. missingness, invalidity, redaction, and non-applicability remain distinct;
4. feature values retain subject, observation time, validity, unit, and lineage;
5. training and simulation populations remain explicitly distinguishable but share the same
   model semantics;
6. preparation and estimation are activated as one immutable workflow artifact;
7. predictions remain distinct from facts, simulations, and authorized state;
8. experiments name their data, code, environment, random streams, and costs;
9. Python and R execute only behind declared analytical boundaries;
10. every deployed model can be compared with later corrected observations of reality.

> **BrixMS does not move data out of the model to perform science elsewhere. It creates
> reproducible analytical and simulated projections of the living model, learns from them,
> and returns versioned improvements to that same model.**

# Part XXVIII — Compilation model and runtime closure

Merged from the execution line (v5.3 Compilation & Gap Closure through v8.1). No
semantics change; this part fixes how every part above *runs*.

## 28.1 Two passes

```text
world.brix ──brixc──► generated Rust workspace ──rustc──► native world binary
(pass 1: BrixMS → Rust)                        (pass 2: Rust → machine code)
```

BrixMS is compiled, not interpreted. Pass 1 emits typed columnar relation stores,
one monomorphized semi-naive delta function per rule per delta source, plain Rust
functions for the value language, and lifecycle/store code for protocols, policies,
language tasks, formalisms, and bricks — all linked against `brix-rt`, which owns the
rule-independent machinery (revisions, scheduler, provenance, lifecycles, event
calendar, WASM host, capabilities, serving). Pass 2 is `rustc`. Normative
consequences: **semantics live entirely in pass 1** — backends (LLVM, Cranelift),
optimization levels, and targets change cost, never any observable value; **the
binary is the model** — one ProgramRevision embedded with its `meta.*` content as
static data, state living outside in the revision log; **no production interpreter**
— `brix-oracle`, the direct naive evaluator of Core IR, is the conformance judge
(differential, bit-compared, runs `extern rust` claims too) and never a deployment
target. ProgramRevision digests cover canonical source + lockfile, never binaries;
`BuildRecord(program, toolchain, target, binaryDigest)` ties artifacts to service
history. Plans are compile-time, recorded in `meta.Plan`, canonical-result-preserving.

## 28.2 Two tiers: activation under ahead-of-time compilation

Staged activation (§20.4) meets immutable binaries through tiers: **tier A** is the
consolidated native binary; **tier B** compiles activated rule deltas in milliseconds
to WASM against the stable **delta ABI** and loads them at the activation boundary;
a supervisor consolidates in the background and swaps binaries at a revision
boundary — activation discipline applied to processes. A rule behaves identically in
either tier (conformance I.23); residency is a `perf.TierResidency` cost fact.
Self-modification pays a temporary performance tax, never a semantic one. The REPL is
tier B with a short attention span; `brix run` uses Cranelift opt-0 with a
content-hash cache; `brix serve --release` uses LLVM + LTO; `brix explain --rust
Rule` prints the generated function.

## 28.3 Runtime closure (normative, condensed)

- **Packages**: 1:1 with generated crates; `pub read` / `pub write` / `pub derive`
  relation visibility; orphan rule for `derive` mirroring trait coherence; link-time
  global phase inference with `brix diff --phases`; identity compatibility domains
  survive semver except through declared migrations.
- **`extern rust`**: the FFI is the target language — declared types, effects, and
  `deterministic pure` claims are trusted, confined to `interop`-marked packages,
  and oracle-liable; effectful externs are boundary-only under capabilities; WASM
  components remain the contract for untrusted or non-Rust code, and native linkage
  is an implementation optimization, not a stable ABI (§26.4 interchange is the
  promise).
- **Persistence**: the revision log (ground facts only, canonical bytes) is truth;
  checkpoints are optimization; recovery = checkpoint + tail replay + settle, exact
  to any retained revision; compaction folds below retention into sealed
  `CompactionRecord`s with `audit` pins; backends sit behind the Part XIII contract.
- **Concurrency**: one settler per namespace (profiles beyond `SingleWriter` per
  §26.6), MVCC snapshot reads, optimistic commit validation, history-invisible
  batching, Drivers on an async executor entering through the same commit path —
  protocol admission is the only backpressure layer.
- **Resource governance, the load-bearing split** (amending §24.5): **logical
  budgets are semantics** — machine-independent units (recursion depth, derived-edge
  counts, extents, workflow steps) rejected deterministically and identically
  everywhere as typed atomic `BudgetExceeded`; **physical budgets are operations** —
  CPU/memory/latency/spend act at admission, never mid-settlement; degraded
  operation is fewer revisions, never different truth.
- **Observation scoping** (amending §24.2): settlement is authorization-blind — one
  truth per revision; every read path (query, watch, export, why, whynot, diff,
  REPL, LLM context, policy candidates, Studio, client projections) evaluates under
  the principal's view; provenance truncates with sealed `Opaque` markers and never
  widens scope.
- **Diagnostics**: `diag.Error/Warning/Note` sealed relations with stable
  `BRX0xxx–BRX8xxx` codes; SARIF/JUnit are projections; cycle errors carry minimal
  paths as structure.
- **Numerics and ordering**: strict IEEE inside settlement, canonical-order
  reductions, `Ord` = Appendix G byte order, static per-package Decimal contexts,
  no bare integer `/`, money divides only through exact allocation.


# Appendix A — Sealed schemas (kernel)

```text
Support(edge, rule, match, atRevision)
Claim(edge, source, transaction, atRevision)
Superseded(newer, older, atRevision)
Retracted(claim, by, atRevision)
Masked(target: EdgeRef, by: EdgeRef, atPhase, atRevision)
KeyConflict(relation, key, candidates: Set<EdgeRef>, supports: Set<SupportRef>, atRevision)
RuleError(rule, site: SiteId, partialMatch: MatchDigest, error, atRevision)
Violation(constraint, match, atRevision)
Complete(relation, partition, through, authority)
ProgramActivated(program, sourceDigest, lockDigest)
ActivationProposal(kind, target, rationale, by)     // governance conclusions; consumed
DisableProposal(rule, rationale, by)                // by activation transactions only

policy lifecycle (per policy Y, Part XII):
Y.Version(digest, trainedThrough)                    // immutable, entity
Y.Active(version)                                    // state, set by transaction only
Y.Decision(id, version, snapshot, candidatesDigest,
           chosen, propensity, seed, at)             // event, Driver-committed
Y.Shadow(id, version, snapshot, candidatesDigest,
         chosen, propensity, seed, at)               // event, never consumable
Y.Feedback(decision, outcome, atRevision)            // DERIVED: deduction over settled
                                                     // outcomes, keyed by decision

protocol lifecycle (per protocol P):
P.Desired(version)          P.Leased(version, lease)
P.Attempted(version, attempt)
P.Succeeded(version, ...)   P.Failed(version, ...)
P.Superseded(new, old)      P.Withdrawn(version)    P.Cancelled(version, outcome)

ontology and formalism descriptors:
onto.Ontology  onto.Concept  onto.Property  onto.Axiom  onto.Entailment
onto.Inconsistency  onto.Alignment  onto.Shape
meta.Formalism  meta.FormalismVersion  meta.FormalismConstruct
meta.FormalismMapping  meta.ModelInstance  meta.FormalismConformance

simulation and modeling:
sim.Now { at }  sim.Schedule.request / .accepted / .cancel / .cancelled
sim.Event.fired / .skipped  sim.SimultaneousConflict  sim.ZenoViolation
sd.IntegrationStep  sd.SolverError  sd.ThresholdCrossing
abm.AgentActivated  abm.Proposal  abm.Arbitration  abm.ActionAccepted

test and quality:
test.Run  test.Case  test.Assertion  test.Failure  test.Counterexample
test.Coverage  test.Mutation  test.Benchmark
quality.Finding  quality.Metric  quality.GateResult  quality.Suppression
quality.ArchitectureDependency  quality.CapabilityChange

meta.Rule  meta.Relation  meta.Phase  meta.Protocol  meta.Activation
perf.RuleCost  perf.IndexUse  perf.RelationSize  perf.DeltaAmplification
perf.ProtocolPressure  perf.SettlementLatency
```

User code may pattern-match these when authorized; no code may assign their sealed
fields.


## A.13 Unified reasoning and intelligence

```text
reasoning.Run
reasoning.Input
reasoning.Result
reasoning.Evidence
reasoning.Verification
reasoning.Certificate
reasoning.Counterexample
reasoning.Unknown
logic.Proof
logic.ProofStep
logic.RuleApplication
logic.Premise
logic.ConstraintProof
llm.Inference
llm.ToolInvocation
llm.Evaluation
math.NumericalRun
math.ErrorBound
opt.SolverRun
```

## A.14 Reflexive program and documentation model

```text
meta.Brick
meta.Port
meta.Adapter
meta.View
meta.Action
meta.Renderer
meta.ClientProjection
meta.ProgramChange
meta.SemanticDiff
doc.Subject
doc.Summary
doc.Description
doc.Rationale
doc.Example
doc.Decision
doc.Runbook
```

## A.15 Reality, decisions, workflows, and client state

```text
model.Observation
model.CandidateClaim
model.Reconciliation
model.RealityDivergence
causal.Model
causal.Intervention
causal.Estimate
decision.Decision
decision.Alternative
decision.Selection
workflow.Instance
workflow.Task
workflow.Outcome
client.GraphProjection
client.GraphDelta
client.Draft
client.OptimisticBranch
render.Node
render.Binding
render.Action
```


## A.16 Completion contracts

```text
model.ValidityEnvelope
model.ValidityRequirement
model.Assumption
model.DomainEvaluation
model.AssumptionBreach
model.Staleness
model.CalibrationStatus
correction.ObservationCorrection
correction.ClaimSupersession
correction.Recomputation
correction.ReportSupersession
interchange.SchemaFingerprint
interchange.CanonicalArtifact
interchange.RevisionDelta
interchange.Signature
transaction.Conflict
transaction.Retry
consistency.Profile
consistency.PartitionOwnership
consistency.MergeEvidence
operation.Cancellation
operation.UnknownOutcome
operation.Reconciliation
operation.Compensation
reproducibility.Profile
reproducibility.Dependency
trust.Profile
trust.Revocation
security.ThreatFinding
```



## A.17 Data science and machine learning

```text
data.Frame
data.FrameColumn
data.Profile
data.ColumnProfile
data.MissingnessProfile
data.OutlierCandidate
data.DuplicateCandidate
data.Recipe
data.RecipeStep
data.PreparedDataset
data.Quarantine
data.QualityFinding
feature.Definition
feature.Set
feature.Value
feature.Materialization
dataset.Dataset
dataset.Partition
dataset.Resampling
dataset.LeakageFinding
stats.Formula
stats.ModelSpecification
stats.FittedModel
ml.Estimator
ml.Workflow
ml.Model
ml.ModelVersion
ml.ModelArtifact
ml.TrainingRun
ml.TuningRun
ml.Prediction
ml.Evaluation
ml.MetricResult
ml.Calibration
ml.DecisionThreshold
ml.Explanation
ml.Drift
ml.RetrainingRecommendation
experiment.Experiment
experiment.Run
viz.Visualization
viz.Layer
report.Report
interop.AnalyticalEnvironment
interop.PortableModelArtifact
```

## A.18 Execution, production, and transport (Part XXVIII, §24.9)

```text
BuildRecord(program, toolchain, target, binaryDigest)
CompactionRecord(horizon, folded, atRevision)
meta.Plan(rule, plan, statsDigest)
perf.TierResidency(rule, tier)
diag.Error / diag.Warning / diag.Note (code, site, message, structure)
BindingRecord(program, binding, digest)
BudgetExceeded(budget, unit, limit, observed, atRevision)
BackupManifest(programRevision, dataRevision, schemaDigest, lockDigest,
               bindingDigest, artifacts)
Opaque(reason)
realtime.Connection / .Subscription / .Delivery / .Backpressure / .Resume / .Failure
```

# Appendix B — Terminology delta from v4

Action → **rule** (`derive`). Model → **entity** (keyed unary relation). Defeat →
**mask** (kernel primitive over edge references; defeat calculus is Edition 2).
Completeness witness → required only for `open` relations and reality-claims at
boundaries. Closed world → **model-closed**. EdgeKey/SupportId/MatchId →
engine-internal; **ClaimRef** is surfaced, opaque. Four surface categories → **three**;
one kernel bucket → **four layers**.

# Appendix C — Migration from v4

Mechanical: `model` → `entity`; `action N { match{P} derive{H} }` → `derive N: H from
{P}`; `Defeats(by, target)` → `mask(target) by reason` with both bound as edge
references; queries gain `= from{...} yield{...}`; drop witness plumbing for owned
relations (they are model-closed); adopt `assert`-returns-`ClaimRef` and
ClaimRef-based `retract`; move lattice/label/redaction code behind edition gates. A
`brix migrate v4` fixture ships with the compiler and is itself a staged program
transformation, verified by shadow settlement before activation.

# Appendix D — Normative surface grammar (EBNF)

Lexical: source is UTF-8; identifiers are Unicode XID normalized to NFC; keywords are
reserved; comments `// line` and `/* block */`; string, numeric, quantity
(`<number> <unit>`), money (`<number> <CUR>`), and duration literals per Appendix G
lexical forms. Newlines terminate clauses inside `{}` blocks; `;` is an explicit
separator where needed.

```ebnf
File        := PackageDecl ModuleDecl Use* Decl* ;
PackageDecl := "package" QualIdent "@" SemVer ;
ModuleDecl  := "module" Ident ;
Use         := "use" QualIdent ( "." "{" Ident ("," Ident)* "}" )? ;

Decl        := EntityDecl | RelDecl | DeriveDecl | ConstraintDecl | QueryDecl
             | ProtocolDecl | DriverDecl | ScenarioDecl | FnDecl | TypeDecl
             | TraitDecl | ImplDecl | MeasureDecl | UnitDecl | CurrencyDecl
             | PhaseDecl | DataRecipeDecl | FeatureDecl | FeatureSetDecl
             | DatasetDecl | StatModelDecl | MlWorkflowDecl | ExperimentDecl
             | VisualizationDecl ;

EntityDecl  := "entity" Ident "{" FieldDecl+ "}" ;
FieldDecl   := "key"? Ident ":" Type ;

RelDecl     := RelKind "rel" Ident "{" RoleDecl+ "}" RelMod* ;
RelKind     := ("state" | "event" | "open")? ;
RoleDecl    := "key"? Ident ":" Type ;
RelMod      := "key" "(" IdentList ")" | "unique" "(" IdentList ")"
             | "time" "(" Ident ")" | "index" "(" IdentList ")"
             | "partition" "(" IdentList ")" ;

DeriveDecl  := "derive" Ident ":" Head "from" Block ;
Head        := TupleHead | NodeHead | MaskHead ;
TupleHead   := QualIdent "(" ArgList ")" ;               (* relation, request, violation *)
NodeHead    := Ident ":" Ident "{" ArgList "}" "keyed" "by" "(" IdentList ")" ;
MaskHead    := "mask" "(" Ident ")" "by" Ident ;         (* both: pattern-bound EdgeRefs *)

Block       := "{" Clause* "}" ;
Clause      := EdgeClause | EntityClause | LetClause | WhenClause | AnyClause
             | ExistsClause | WithoutClause | OptionalClause | HistoryClause
             | PathClause | CrossClause ;
EdgeClause  := (Ident "@")? QualIdent "(" ArgList ")" ;
EntityClause:= Ident ":" Ident "{" FieldPatList "}" ;
LetClause   := "let" Pattern "=" Expr ;                  (* Expr may end in "?" *)
WhenClause  := "when" Expr ;
AnyClause   := "any" "{" ("case" Block)+ "}" ;
ExistsClause:= "exists" Block ;
WithoutClause := "without" Block ;
OptionalClause:= "optional" Block ;
HistoryClause := "history" EdgeClause ;
PathClause  := "path" PathExpr "from" Ident "to" Ident ;
PathExpr    := PathStep ( "|" PathStep )* Repeat? | "(" PathExpr ")" Repeat? ;
PathStep    := QualIdent "(" Ident "->" Ident ")" ;
Repeat      := "+" | "*" | "{" Nat ("," Nat)? "}" ;
CrossClause := "cross" Block ;

ConstraintDecl := "constraint" Ident ("advisory"|"strict"|"audit") Block ;
QueryDecl   := "query" Ident "(" ParamList? ")" "->" Type "="
               "from" Block "yield" Expr OrderClause? ;
OrderClause := "order" "by" Expr ("," Expr)* ("limit" Expr)? ;

ProtocolDecl:= "protocol" Ident "{" RequestDecl OutcomeDecl+ PolicyDecl? "}" ;
RequestDecl := "request" "{" RoleDecl+ "}" "key" "(" IdentList ")" ;
OutcomeDecl := "outcome" Ident "{" RoleDecl* "}" ;
PolicyDecl  := "policy" "{" PolicyItem* "}" ;

DriverDecl  := "driver" Ident "for" Ident "needs" CapList
               "{" "on" "request" "(" Ident "," Ident ")" FnBlock "}" ;

ScenarioDecl:= "scenario" Ident "{" SeedDecl BindDecl* SetupDecl?
               StepDecl* AtDecl* AssertDecl* "}" ;
SeedDecl    := "seed" (Nat | "each" Range) ;
BindDecl    := "bind" QualIdent ("(" ArgList ")")? ("to" AdapterExpr)? ;
SetupDecl   := "setup" TxBlock ;
StepDecl    := "step" "every" Expr "for" Expr TxBlock ;
AtDecl      := "at" Expr TxBlock ;
AssertDecl  := "assert" ("always"|"eventually"|"at" "end") "{" Expr "}" ;

TxBlock     := "{" TxStmt* "}" ;
TxStmt      := "let" Pattern "=" TxExpr | TxExpr ;
TxExpr      := "ensure" Ident "{" ArgList "}"          (* entities only *)
             | "fresh" Ident "{" ArgList "}"
             | "assert" (QualIdent "(" ArgList ")" | Ident "{" ArgList "}")
             | "set" QualIdent "(" ArgList ")"
             | "retract" Expr
             | "supersede" Expr "over" Expr ;



DataRecipeDecl := "data" "recipe" Ident "{" RecipeItem* "}" ;
RecipeItem     := "input" Type | "output" Type | "step" Ident RecipeArgs?
                | "quarantine" Expr ;
FeatureDecl    := "feature" Ident "(" ParamList? ")" "->" Type
                  ("=" Expr | "{" FeatureItem* "}") ;
FeatureItem    := "observationTime" Expr | "window" Expr | "source" QualIdent
                | "leakage" Ident | "missing" Ident ;
FeatureSetDecl := "feature" "set" Ident VersionTag? "{" FeatureSetItem* "}" ;
DatasetDecl    := "dataset" Ident "{" DatasetItem* "}" ;
StatModelDecl  := "statistical" "model" Ident "{" StatModelItem* "}" ;
MlWorkflowDecl := "ml" "workflow" Ident "{" MlItem* "}" ;
ExperimentDecl := ("experiment" | "tuning") Ident "{" ExperimentItem* "}" ;
VisualizationDecl := "visualization" Ident "{" VisualizationItem* "}" ;

FnDecl      := "partial"? "aggregate"? "fn" Ident Generics? "(" ParamList? ")"
               "->" Type EffectRow? ("=" Expr | FnBlock) ;
EffectRow   := "!" "{" (Effect ("," Effect)*)? "}" ;

Comprehension := "from" Block ("yield" Expr)? ;          (* : Rel<S> *)
```

Expression grammar (literals, `if`/`match` expressions, calls, operators with the
precedence table of v4 §XV.3 carried over, `?` postfix) and the type grammar
(generics, rows `{ f: T | r }`, `Rel<Row>`, quantities, compound units `T / U`) follow
the same conventions and are normative as written in the reference parser's grammar
file, which this appendix is generated from. Ambiguities resolve by longest match;
`brix fmt` output is the canonical form.

# Appendix E — Static semantics (judgments)

Environments: Γ (types, traits), Σ (relation schemas), Φ (phase assignment), Κ
(capabilities in scope), Ε (effect row).

```text
Expressions       Γ ⊢ e : T ! ε
Patterns          Γ; Σ ⊢ clause ⇒ Bindings; Reads; Completeness-obligations
Rule              Γ; Σ ⊢ derive N: H from B  ⇒  deps(B) → H
                  side conditions:
                    pure(B, H)            no effect atoms in ε(B ∪ H)
                    det(B, H)             deterministic per Part V §8
                    nondiverge(B, H)      no `diverge` in any called fn
                    keys(H) ⊆ Bindings    heads fully keyed by bound values
                    mask-head: target, reason ∈ EdgeRefs(B)
Aggregate call    Γ ⊢ f : aggregate Rel<S> -> T
                  in-rule use ⇒ strict dep on every relation in extent(S)
Ordinary fn       Γ ⊢ f : Rel<S> -> T   ⇒   in-rule use on graph-derived Rel: ERROR
Without/Optional  Σ ⊢ R model-closed  ∨  witness Complete(R, π) available
Key               Γ ⊢ T : Canonical    for every type in any key position
Query             Γ; Σ ⊢ q : Snapshot -> Rel<Row>, pure
Transaction       Γ; Κ ⊢ tx : requires graph.write<Scope>; ensure targets entities;
                  assert targets relations; retract consumes ClaimRef (affine)
Driver            Γ; Κ ⊢ d : effects(body) ⊆ declared; capabilities(body) ⊆ needs
Scenario          Γ ⊢ s : no external capability atoms reachable; adapters total
                  over declared outcomes
```

Type soundness claim (progress + preservation for the expression language; settlement
safety for rules) is stated here and mechanized under Edition 5.

# Appendix F — Phase inference (normative)

Construct graph D over rules and constraint/query read-sites:

1. **Positive edge** r₁ → r₂ when r₂ reads a relation r₁ derives (live, monotone).
2. **Co-producers.** Before condensation, add a positive cycle in canonical rule-id
   order among all Tuple-head producers of each relation R. Thus tuple production is
   predicate-granular: every producer of R shares one SCC and R becomes complete at one
   phase boundary. Mask-head producers are excluded from this coupling. *(Erratum 0002:
   predicate-level condensation.)*
3. **Strict edge** r₁ ⇒ r₂ when r₂ reads through `without`, `optional`-absence, an
   `aggregate fn`, or a witness over anything r₁ derives.
4. **Mask edges** for each relation R with mask-producer set M(R) ≠ ∅:
   producers(R) ⇒ M(R), and M(R) ⇒ every ordinary live read-site of R (excluding the
   target binding inside each m ∈ M(R); `history` reads excluded).
5. Condense SCCs of positive edges. A strict or mask edge inside one SCC is a
   compile-time error reporting the minimal cycle.
6. Phases are the topological order of the condensation with strict/mask edges as
   ordering constraints; user `phase` declarations add constraints and must be
   consistent, else error with the conflicting path.
7. Within a phase: least fixpoint, semi-naive, order-free.

# Appendix G — Canonical encoding and identity (normative sketch)

One byte-level encoding, versioned as `canon/1`, used for NodeId/EdgeId/ClaimId,
aggregate row ordering, `Canonical` hashing, and serde of key material:

- integers: minimal-length big-endian two's complement with sign byte; Nat unsigned;
- Decimal<P,S>: scale byte + unscaled integer encoding; normalized (no trailing zeros
  beyond declared scale);
- strings: NFC for identifiers; values as raw Unicode scalar sequences, length-prefixed;
- bytes: length-prefixed raw;
- Bool/Unit/Char: single tagged bytes;
- Instant/Duration/Date: epoch-based fixed-width; TimeZone by IANA identifier string;
- enums: variant ordinal (declaration order is ABI; reordering is a compatibility-domain
  change) + payload encodings;
- records/rows: fields sorted by canonical field-name bytes, each name-prefixed;
- collections: Set/Map entries sorted by canonical element/key bytes; List/Vector in
  sequence order; Bag as sorted (element, multiplicity) pairs;
- quantities: normalized to the measure's base unit, value + measure identifier;
- money: currency code + minor-unit integer;
- entity keys: type compatibility domain digest + key fields in declaration order;
- relation tuples: `canon_ident(relation compatibility domain)` + roles sorted by role
  name, then hashed in the `Edge` domain; absent an explicit declared token, the domain
  is the relation's stable fully-qualified name;
- floats: NOT encodable in `canon/1` key or identity positions. They are value-canonical
  outside those positions: canonical row order and value digests use canonicalized bit
  patterns (including one NaN pattern) and full-row `totalOrder` tiebreaks.

Hash algorithm: BLAKE3, versioned with the profile. Identity compatibility is a schema
promise; changing key meaning requires a new compatibility domain and explicit mapping.

# Appendix H — Protocol lifecycle state machine (normative)

Per RequestVersion v under RequestKey k:

```text
            support appears
                 │
             ┌───▼────┐   payload change under k        ┌────────────┐
             │Desired │ ────────────────────────────►   │ Superseded │
             └───┬────┘        (new version created)    └────────────┘
     admission   │                                       in-flight attempts of the
     policy ok   ▼                                       old version run to their own
             ┌────────┐  lease expiry                    terminal outcomes
             │ Leased │ ───────────► re-lease (new lease, same version)
             └───┬────┘
                 ▼
             ┌──────────┐  retryable failure + policy budget
             │Attempted │ ───────────► Leased (attempt ordinal + 1, backoff via clock facts)
             └───┬──────┘
        ┌────────┼──────────┐
        ▼        ▼          ▼
   Succeeded   Failed   Cancelled(outcome ∈ {BeforeStart, Acknowledged,
   (terminal) (terminal)             TooLate(result), Unsupported})

 Desired ──(all support lost before terminal)──► Withdrawn
```

Invariants: attempts and terminal outcomes bind to exactly one version; a version has
at most one terminal outcome; one key may have many versions each with terminal
history; `satisfies` policy (samePayload | sameKey | never) decides whether a
successor version becomes Desired at all when a prior success exists; admission
policies (`Defer`, `Reject`, `Coalesce` = supersession) apply between support and
lease; every transition is a sealed history edge.

# Appendix I — Conformance suite (normative categories)

An implementation conforms when it passes the executable suite. Categories, each with
directional and randomized cases:

1. **Incremental = full recompute.** For randomized programs and transaction streams,
   the incrementally settled view at every revision is bit-identical to whole-world
   recomputation — including error edges, KeyConflicts, masks, and provenance answers.
2. **Deterministic identity.** NodeId/EdgeId/ClaimId stable across runs, machines, and
   transaction retries; `fresh` identity retry-stable under one intent.
3. **Support dynamics.** Removing the last support removes the edge; shared supports
   removed in any order converge; cache eviction is invisible.
4. **Mask dynamics.** Mask appearance/disappearance updates live views at exactly the
   phases Appendix F dictates; `history` unaffected; transitive masking of masks.
5. **Key conflicts.** Per-kind rules of Part III §8; conflicted keys expose no live
   value; conflict resolution by input change restores the survivor; completeness
   withheld over conflicted partitions.
6. **Transaction semantics.** Serializable detects read/write and predicate conflicts;
   snapshot admits declared write skew only; retries stable; strict constraints
   evaluated on fully settled candidates.
7. **Phase inference.** The programs of Appendix F's edge cases compile to the
   specified phase assignments; cyclic cases rejected with minimal paths.
8. **Numerics.** Parallel and incremental float results bit-identical to canonical
   sequential; NaN canonicalization; totalOrder sorting.
9. **Scenario reproducibility.** (program, scenario, seed) yields byte-identical
   revision histories across machines and thread counts; adapter resolution order
   canonical; `sim.replay` exactness including uncovered-request failure.
10. **Protocol lifecycle.** Appendix H invariants under supersession storms, lease
    expiry, retry budgets, cancellation races, and Coalesce; no lost or duplicated
    terminal outcomes per version.
11. **Snapshot isolation.** No API observes intra-settlement state; watches deliver one
    delta per published revision; cursors refuse to cross snapshots.
12. **Reflexive bound.** No data revision can alter its own settlement program;
    activation only between revisions; shadow-settlement equivalence on activation.
13. **Policy lifecycle.** Decisions bind to exactly one immutable version; the active
    version changes only by transaction; historical decisions keep their producing
    version under any later activation; feedback derivations retract and reattach with
    their inputs like any derived structure; advisory suggestions with no consuming
    rule are provably inert (no derived requests reachable).
14. **Policy determinism.** (version, snapshot, candidatesDigest, seed) reproduces the
    identical decision and propensity; shadow decisions never appear in any consuming
    rule's readable extent; scenario-bound policy adapters replay exactly.


15. **Native test reproducibility.** Failure bundles replay byte-identically; structural
    shrinking preserves typing and failure; semantic coverage is stable across physical
    plans; mutation and schedule perturbation cannot alter the unmutated baseline.
16. **Quality determinism.** Given identical source, lockfile, profile, compiler and
    evidence inputs, findings and gate results are canonical; suppressions expire and
    cannot hide semantic errors; architecture and capability diffs are complete.
17. **Ontology entailment.** Horn/RDFS/OWL-RL fixtures produce the specified closure;
    open and model-closed assumptions remain distinct; contradictions derive explicit
    inconsistency without explosion; RDF/shape round trips report unsupported semantics.
18. **DES event semantics.** Event ordering follows SimTime; simultaneous batches share
    one pre-state; conflicting writes are explicit; zero-delay chains use microsteps;
    scheduler cancellation and replay are deterministic.
19. **DES random streams.** Draws are stable under unrelated source and plan changes;
    replications reproduce across worker counts; common-random-number comparisons retain
    corresponding streams.
20. **System dynamics numerics.** Unit errors are rejected; integration fixtures match
    the normative method and profile; threshold crossings are localized within tolerance;
    step-size refinement and solver failures are recorded honestly.
21. **ABM scheduling.** Every activation is explicit; simultaneous propose/arbitrate/
    commit observes one pre-state; sequential schedules are replayable; unspecified
    worker order cannot alter results; communication lifecycle is preserved.
22. **Hybrid synchronization.** Formalisms exchange only published settled snapshots;
    ownership conflicts are rejected; DES/SD threshold bridges and ABM aggregate
    exchanges reproduce under the recorded profile.


## Appendix I.15 — v6 reasoning conformance

A conforming v6 implementation must demonstrate epistemic-status preservation, exact and
approximate numeric separation, deterministic floating-point behavior, proof-object
stability, solver `Unknown` handling, LLM structured-output validation, evidence checks,
record and replay, and generated-program activation safety.

## Appendix I.16 — v6 reflection conformance

It must publish the mandatory versioned `meta` schema, preserve semantic descriptor identity,
provide authored-to-resolved correspondence, prevent current-revision self-modification,
enforce reflection authorization, and support semantic diffs and executable documentation
examples.

## Appendix I.17 — v6 observation and decision conformance

It must distinguish observations, candidates, claims, conclusions, predictions, and
simulations; preserve source and temporal lineage; provide explicit identity reconciliation;
and retain complete decision alternatives, assumptions, authority, and outcome evidence.

## Appendix I.18 — v6 brick and client conformance

It must validate typed port composition, state ownership, adapter semantics, ontology
alignment, component compatibility, authorized server projections, atomic client delta
application, draft and optimistic state separation, renderer independence, and server-side
command enforcement.

## Appendix I.19 — v6 professional-platform conformance

Production profiles must test authentication and authorization, privacy lifecycle,
configuration and secret isolation, resource failure atomicity, deployment reproducibility,
canary and rollback behavior, backup and recovery, supply-chain evidence, API compatibility,
WebSocket resume and backpressure, and certified component manifests.


## Appendix I.20 — v6 completion-contract conformance

A conforming implementation must test:

1. validity-envelope evaluation, including degraded, out-of-domain, stale, insufficient,
   breached-assumption, and uncalibrated outcomes;
2. bitemporal correction, supersession, retraction, recomputation, and knowledge-as-of queries;
3. canonical BGIF, BRDP, BCMF, BPEF, and BRTF encoding, hashing, streaming, and unknown-field
   preservation;
4. serializable, snapshot, and expected-revision transaction behavior under randomized
   conflicts and retries;
5. declared distributed ownership, ordering, merge, and offline profiles without silent
   weakening of canonical truth;
6. unknown external outcome, reconciliation, idempotency, compensation, deadline, and
   cancellation races;
7. reproducibility-tier propagation and rejection of stronger unsupported claims;
8. trust-profile enforcement, revocation, sandboxing, tenant isolation, prompt injection,
   poisoned inputs, and resource-denial defenses.

## Appendix I.21 — v6 language-completeness and tooling conformance

The standard distribution must pass fixtures for the stable everyday language and library
surface, including Unicode, time zones, collections, codecs, resource scopes, Driver
cancellation, capability-safe platform access, exhaustive matching, trait coherence, and
structured errors. The reference evaluator, formatter, compiler, REPL, test runner, client
engine, and semantic diagnostics must agree on canonical examples and rejected programs.

Developer commands required by Part XXVI must be present or explicitly reported as
unsupported by a non-production profile. Production conformance requires the full build,
test, explain, inspect, diff, impact, doctor, replay, and local reference-runtime loop.


## Appendix I.22 — v6 data-science and machine-learning conformance

A conforming premium data-science implementation must demonstrate:

1. `Frame<S>` results are snapshot-bound, unordered by default, and equivalent across native,
   Arrow, and certified backend execution;
2. typed missingness preserves `NotObserved`, `NotApplicable`, `Unknown`, `Redacted`,
   `Invalid`, and `Pending` without implicit coercion;
3. cleaning recipes are immutable, lineage-complete, and fit preparation parameters only on
   permitted partitions;
4. feature values preserve subject, observation time, valid time, units, missingness, and
   program revision, with differential offline/online feature tests;
5. datasets and resampling schemes reproduce exact membership and detect future, label,
   group, duplicate, correction, and simulation-population leakage;
6. estimator workflows include preprocessing, model, calibration, thresholds, and output
   contracts in one immutable artifact identity;
7. experiment runs reproduce according to their declared tier and retain dataset, code,
   environment, seed, artifact, metric, warning, cost, and duration evidence;
8. predictions remain distinct from observations, facts, simulations, policy suggestions,
   and authorized state;
9. model registry promotion, shadowing, canarying, rollback, retirement, and historical
   prediction lineage are transactionally correct;
10. Python and R adapters receive only declared projections, preserve Arrow schema and factor
    semantics, record locked environments, and cannot execute inside settlement;
11. portable artifacts validate feature and output contracts before activation;
12. monitoring compares predictions with later correction-aware observations and produces
    candidate retraining recommendations rather than implicit model replacement.

---

## Appendix I.23 — v9 execution conformance

Tier equivalence: any rule executed in tier B produces settled views, error edges,
conflicts, and provenance bit-identical to tier A; activation → consolidation →
replay yields identical history. Backend independence: Cranelift-dev, Cranelift-WASM,
and LLVM-release builds are observably identical, including float bit patterns;
storage backends alter no observable value. Build determinism: ProgramRevision
digests are machine-independent; BuildRecords accurate; a binary refuses foreign
revision logs (tier-B deltas excepted); `extern rust` purity claims hold under the
oracle. Budget determinism: logical violations reject identically everywhere;
physical stress fixtures alter admission and latency only — no settled value differs
and no partial revision is observable. Scoping: no read path exceeds the principal's
view; explanations truncate opaquely; settlement is identical regardless of any view.

## Appendix I.24 — v9 real-time transport conformance

Socket clients observe only published settled revisions; every graph event names its
SnapshotId; per-subscription sequence and revision order hold; resume replays or
Rebases, never skips; bounded buffers survive slow-consumer fixtures with zero silent
drops; authorization rechecks fire on resume/role/activation/policy change; command
retries preserve intent identity; `sim.websocket` is contract-equivalent to
production across fragmentation, expiry, restart, partition, and heartbeat fixtures.

## Appendix I.25 — v9 frame-governance conformance

Ablation sensitivity computes over declared context classes and feeds release gates;
authorship/authority separation is enforced (single-principal frame-plus-approval at
gated or autonomous authority is activation-blocking); `LLM.ContextCuration` fires on
the fixture catalog of narrow frames feeding high-authority consumers.

# Appendix K — Implementation gates (normative freeze criteria)

v9.0 is declared feature-complete only when every gate below has normative text,
reference behavior, diagnostics, and conformance tests. A box is a contract, not an
intention.

**A. Kernel freeze** — identity algorithms (relation, entity, claim, revision,
support, mask, conflict); settlement, phases, recursion, absence, aggregation, error
edges; transaction isolation, retry, intent identity, strict constraints; protocol
lifecycle including unknown outcomes; no feature requires a second substrate.

**B. Epistemic results** — proof, exact, estimate, interval, probability,
simulation, suggestion, interpretation, authorization, and external outcome are
distinct types; validity envelopes and applicability status attach to model results;
reproducibility tiers propagate; solver `Unknown`, boundary `UnknownOutcome`, and
model `OutOfDomain` never conflate.

**C. Reality and time** — observation, candidate claim, accepted claim, conclusion,
prediction, and action are distinct; four times specified (valid, transaction,
simulation, correction); knowledge-as-of queries; real actions remain historically
visible after their motivating information is corrected; merge/split identity
history explicit.

**D. Reasoning** — executable proof objects with differential tests; stable numeric
tower and solver records; DES/DEVS/SD/ABM/causal/hybrid lowering conformance-tested;
policies and LLM inference advisory until explicit authority; all learned and
reasoning artifacts immutable and provenance-bearing.

**E. Data science** — `Frame<S>` snapshot-bound with canonical Arrow mappings;
explicit ordering/time/units/lineage in manipulation; six-way missingness; immutable
recipes separating fit from apply; reflected factors/formulas/features/datasets;
feature observation-time and offline/online parity; split leakage detection
(temporal, label, group, duplicate, correction, simulation); estimator workflows
with calibration and thresholds; experiments retain data/code/environment/seeds/
metrics/cost; predictions distinct from claims; governed registry promotion; Python
and R only through sandboxed locked boundaries; portable artifacts validate typed
contracts before activation; data-science bricks compose through typed ports.

**F. Self-model** — public `meta` schema for all stable artifacts; authored,
resolved, lowered, runtime, documentation, test, quality, and operational models
align; reflection authorization-aware and unable to mutate the active revision;
semantic diffs and impact analysis across data, behavior, capabilities, bricks, and
APIs; executable documentation gates releases.

**G. Bricks and clients** — bricks package domain, behavior, simulation, workflow,
decision, and interaction; ports, ownership, alignment, and adapters statically
checked; projections expose only authorized fragments; client deltas atomic and
client settlement differentially equal to the reference on the supported subset;
authoritative, draft, optimistic, simulated, predicted, and local state distinct;
render graphs framework-independent with React first-party.

**H. Interchange** — BGIF, BRDP, BCMF, BPEF, BRTF published; canonical binary,
canonical JSON, and Arrow mappings pass cross-runtime vectors; all manifests signed
and content-addressed; registry compatibility spans semantic, data, port,
interaction, ontology, and simulation surfaces; certifications use public suites.

**I. Professional baseline** — the everyday language (ADTs, matching, generics,
traits, modules, errors, resources) is pleasant; Unicode, time, collections, codecs,
compression, crypto interfaces ship; structured Driver concurrency; capability-safe
platform APIs; i18n and accessibility.

**J. Trust and operations** — principal/tenant/capability/row/provenance/reflection
authorization aligned; retention, redaction, legal hold, cryptographic erasure,
derived lineage; trust classes, sandboxes, revocation, published threat model;
atomic budget failure; deployment, shadow, canary, rollback, backup, recovery,
corruption verification tested.

**K. Formal conformance** — grammar, static semantics, operational semantics,
identity, and encodings executable; the oracle is the semantic authority;
incremental/full, backend-parity, schedule-independence, tier-equivalence, and
client/server differentials pass; diagnostics carry minimal dependency paths; the
local loop (`new build run repl watch test sim explain inspect impact doctor`)
coheres.

**L. Scope discipline** — proposals introducing hidden mutable state, ambient
authority, a second application truth, or a competing execution paradigm are
rejected; new algorithms lower to existing semantics or execute behind typed
boundaries.

**Completion statement.** BrixMS is ready for implementation freeze when a
two-person kernel team can implement the semantic oracle, a wider platform team can
build optimized runtimes and tooling without private semantics, and independent
component authors can compose bricks through published formats and conformance
tests — and data scientists can clean, model, simulate, train, evaluate, and deploy
without ever creating a second application truth outside the living hypergraph.


---

**End of BrixMS Language Specification v9.0 — The Living Model Edition**
