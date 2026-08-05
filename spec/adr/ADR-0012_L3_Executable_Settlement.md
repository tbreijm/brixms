# ADR-0012 — Brix L3: Executable Settlement, Saturated Rule-Agenda Profile

Status: **Accepted** (2026-08-05; Proposed 2026-08-02 as an unsaturated v1
profile, re-pinned against [ADR-0014](./ADR-0014_Divergence_Sensitive_Saturation.md)
before any implementation existed). This is the implementation pin for the
first executable Brix settlement path. It extends
[ADR-0010](./ADR-0010_SOC_Language_Design.md)'s L3 goal, under the authority
and failure rules of the [SOC Constitution](./ADR-0002_SOC_Constitution.md).
It is intentionally narrower than a general Brix runtime: it turns a parsed
module's finite, zero-argument `rule` declarations into a deterministic,
auditable settlement agenda. It does not assign general evaluation semantics
to rule bodies.

Date: 2026-08-02; re-pinned 2026-08-05.

Foundation: [ADR-0002](./ADR-0002_SOC_Constitution.md) §§4–5, §8–9;
[ADR-0010](./ADR-0010_SOC_Language_Design.md) §§3, 5, 7;
[ADR-0014](./ADR-0014_Divergence_Sensitive_Saturation.md) §§3, 5–6 (the total
settlement interface this profile executes through); the current
`soc-core::{commit, calendar, journal, audit, saturate}` APIs; and SOC-LAW-08,
SOC-LAW-10, and SOC-LAW-11 in
[`spec/SOC_Semantic_Laws.md`](../SOC_Semantic_Laws.md). This ADR is a bounded
runtime bridge in the #178 area.

## 0. What the 2026-08-05 re-pin changed, and why

The 2026-08-02 draft was written while #61 was open. It therefore specified a
deliberately saturation-blind profile whose stop vocabulary
(`PlanComplete`/`FrontierStalled`/`CommitBudgetExhausted`) was forbidden from
claiming quiescence, and its §11.5 reserved the stronger interface for "a
later profile … under a new execution-profile marker."

**#61 is now closed.** ADR-0014 is Accepted and its four stages landed in
`crates/soc-core/src/saturate/`: `StepLabel`/`ObservationProfile`,
`SaturatedStep`, `sat_step`, `run_saturated`, the quiescence and divergence
certificates and their total fail-closed checkers, weak bisimulation, and the
CJ-1 adequacy interface. The interface L3 was told to wait for exists.

No L3 code was ever written, so nothing is being migrated and no identity is
being reinterpreted. Rather than build the blind profile and retrofit it, this
re-pin has L3 consume the saturated interface from its first commit. Three
consequences dominate the rest of this document:

1. `PlanComplete` — a bookkeeping claim that L3's own pending list was
   empty — is replaced by `Quiescent`, carrying a `QuiescenceCertificateV1`
   that a checker re-derives independently from the presentation and journal
   (§5, ⟨D-STATUS⟩). This is the entire epistemic payoff of the re-pin: the
   successful terminal status stops being L3's word for it.
2. `FrontierStalled` disappears as a status. Certified quiescence is relative
   to a policy and regime set — the certificate says so — so a denied agenda
   *is* genuine quiescence under that policy, reported as `Quiescent` plus a
   non-semantic **agenda residue** diagnostic (§5, ⟨D-RESIDUE⟩).
3. Saturation over *this* profile is the identity: the profile declares
   `𝒢_τ = ∅`, so `sat_step` degenerates to `commit_tick` exactly as ADR-0014
   §3.2 describes (§4.1, ⟨D-TAUZERO⟩). The re-pin buys the certified `1`
   summand and a driver that later profiles can add administrative steps to
   without a rewrite. It does not buy L3 v1 depth it does not have, and this
   document does not pretend otherwise.

Two decisions had to overturn text in the 2026-08-02 draft; both are marked
and argued where they occur: ⟨D-LIM⟩ (§3.3, the run budget leaves
`RunContextV1`'s identity) and ⟨D-CAND⟩ (§4.2, a bounded/fallible
`Regime::candidates` is reclassified from "later compatible extension" to
profile-incompatible).

---

## 1. Decision

L3 SHALL compile a supported `.brix` module into one finite settlement plan,
execute that plan through `soc-core`'s keyed committed coalgebra **via
ADR-0014's saturated settlement interface**, and return an explicit,
replay-addressable run result. Its total stop vocabulary is ADR-0014's:
certified quiescence, certified administrative divergence, and an explicit
`Unknown` — plus a pre-run `Rejected`. Only certified quiescence is
successful, and it is successful because a checker can re-derive it, not
because the runner asserts it.

The execution-profile marker is `brix.l3.rule-agenda-saturated@1`. The marker
`brix.l3.rule-agenda@1` from the 2026-08-02 draft is **retired unimplemented**:
it names the blind profile, no build ever emitted it, and it MUST NOT be minted
by any future implementation. §10's rule that saturation requires a new
execution-profile marker is thereby satisfied rather than waived ⟨D-PROFILE⟩.

The supported execution fragment is deliberately small:

1. A module may contain `config`, immutable `let`, and zero-argument `rule`
   declarations. L3 reuses L2's primitive type/grade rules but is independently
   responsible for the module-wide checks in §3.1. It normalizes every selected
   rule body to the closed static `L3ValueV1` fragment in §3.2 and validates
   its inferred type against any declared rule return type.
2. Each supported `rule name() = body` is a **one-shot settlement proposal**.
   It becomes eligible exactly once, when its rule identifier is at the head
   of the module-ordered pending agenda. Committing it removes that identifier
   and records the canonical identity of `body` as that rule's emitted fact.
3. A rule body is a *canonical static consequence payload*, not an evaluator
   call in v1. Arithmetic, ordinary calls, field access, matching, recursion,
   parameters, and free variables are rejected. A prior immutable `let` may
   be referenced only after it has normalized to a closed `L3ValueV1`.
4. `fn`, `regime`, `gen`, parameters on `rule`, `show`, `witness`, `why`,
   `audit`, `prove`, and `then`/`and` are rejected by the L3-v1 compiler when
   they are relevant to execution. They remain parsed surface constructs, not
   silently ignored declarations.

This profile is executable and finite: a successful run commits at most one
step per selected rule. `Quiescent` means the admissible frontier at the
terminal world is empty under a complete enumeration, in this context, under
this policy and regime set, at this presentation revision — exactly what
ADR-0014 §6.1's certificate asserts and nothing more. It is **not** a general
Brix fixpoint theorem, and it is not a claim about any other policy. The
profile exercises the real calendar, commit, journal, replay, audit,
incremental candidate, and saturation boundaries without pretending that the
present AST already has a full transition/effect language.

Module order is the entire v1 rule policy: the frontier contains at most the
head rule's candidate. Multi-rule deliberation, guards, and user-authored
priorities require a later profile. This restriction is what lets a committed
world delta remove one candidate and add at most one successor candidate
instead of rebuilding an agenda-sized frontier.

## 2. Current substrate and required additions

The following existing APIs are normative building blocks, not sketches:

| Need | Existing implementation | L3-v1 use |
|---|---|---|
| Parse a module | `brix_syntax::parse` → `ast::Module` | The only source input. |
| Static value validation | `brix_syntax::ast` plus `brix_lower`'s declared config environment | Normalize closed `L3ValueV1` values; L3 does not substitute a second evaluator or mint an audit result. |
| Hot execution state | `soc_core::ExecConfig { world, policy, history }` and `Interner` | Intern canonical v1 world/policy identities at the boundary. |
| Reference candidate relation | `Regime`, `SettlementRegime`, `Candidate`, `AdmRegimeAllowlist` | The compiler-owned v1 regime proposes pending rules; policy admits only that regime. |
| Incremental candidate view | `IncrementalRegime`, `IncrementalEngine`, `CandidateDelta`, `Delta`, `Footprint` | Required public execution substrate; current engine needs a calendar/decomposition adapter. |
| Deterministic commitment primitives | `Key`, `Frontier`, `CommittedStep` | The adapter maintains and selects from a keyed incremental frontier. |
| Durability and replay | `CommittedStep`, `Journal`, `Journal::replay_chain` | Append-only committed trace and canonical history-chain identity. |
| Audit boundary | `audit_step` / `audit_journal` | Optional, separate `Derived → Audited` upgrade only. |
| Observation boundary | `saturate::{ObservationProfile, GeneratorPartitionProfile, StepLabel}` | The plan declares an all-realizing profile over exactly its own generators (§4.1). |
| Saturated stepping | `saturate::{PresentationV1, sat_step, run_saturated, SaturationBudget}` | The public driver. L3 supplies the presentation; it does not write its own stepping loop. |
| Certified outcomes | `saturate::{SaturatedStep, SaturatedStop, QuiescenceCertificateV1, check_quiescence_certificate}` | The total stop vocabulary and the independently checkable success claim. |

The following additions are REQUIRED before an L3 command is exposed. They
are additions around these APIs, not replacements for them:

1. **`L3PlanV1` lowering in `brix-lower`.** It MUST validate the fragment in
   §1, canonicalize the selected rules in module order, normalize `L3ValueV1`,
   and produce enough owned data to implement one `Regime`, one
   `IncrementalRegime`, and a fallible decomposition path. `check_module` MUST
   NOT be reused as a runtime plan merely because it returns `CheckResult`: it
   does not currently lower or validate `rule` declarations.
2. **Canonical L3 artifacts.** The surface AST has no `Canonical` encoder, so
   the compiler MUST add versioned encoders for the *lowered* plan, its initial
   world, policy, rule identifiers, fact payloads, and run result. Hashing
   source text directly is forbidden: whitespace/comments must not change a
   plan identity. These encoders belong at the Brix lowering/runtime boundary,
   not in `soc-core`.
3. **A compiler-owned dual regime.** It MUST implement both `Regime` (the
   retained naive/reference relation) and `IncrementalRegime` (the public
   runner's delta path). It MUST derive a `Candidate` only from the head of the
   pending v1 agenda, and fallibly decompose a committed candidate to a
   one-generator recorded `Decomposition` whose endpoints are the exact
   pre/post L3 world identities. The generator identity MUST bind the plan
   identity and rule identity; a single anonymous `g_rule` is insufficient
   for audit replay. Because `IncrementalEngine` owns its mutable regime, the
   naive differential oracle uses a separate regime instance over the same
   immutable precomputed transition table.
4. **An incremental settlement adapter driving `run_saturated`.** The public
   runner MUST integrate `IncrementalEngine`, an incrementally maintained
   keyed `Frontier`, calendar selection, fallible decomposition, journal
   append, and a final execution state/stop reason. It MUST compare the
   incremental admissible candidate view and selected key against the naive
   `Regime` relation filtered through the exact `Adm` policy after every
   committed step. `soc_core::run`, `run_reason`, and `commit_tick` stay
   reference-only; they cannot back the public path. The adapter MUST convert
   source-derived malformed-plan/key/decomposition conditions to a failure
   result rather than expose a panic.

   **The stop vocabulary is not the adapter's to invent.** The adapter builds
   the `PresentationV1` and the keyer and hands stepping to
   `saturate::run_saturated`; the returned `SaturatedStop` is the authority on
   why the run ended. An adapter-local integrity failure (a differential
   mismatch, a staged-update failure) is reported as its own `Unknown` reason
   and MUST NOT be dressed as a `SaturatedStop::Quiescent`.
5. **One settlement-authority path.** ✅ *Landed in PR #235.* `soc-core`
   factors a fallible `try_commit_selected` primitive out of `commit_tick`.
   Given an already selected key/candidate and its `SettlementRegime`, this
   primitive alone validates/decomposes the candidate, derives the
   generator-chain witness, constructs the `Derived` judgement/`Observation`
   and `CommittedStep`, and advances `ExecConfig`. Naive `commit_tick`,
   `sat_step`, and the incremental adapter all reach commitment through it.
   The Brix-owned adapter selects and schedules; it never mints a settlement
   judgement. The shared pure `prospective_successor` operation exposes the
   oracle successor fold without constructing an observation.
6. **Fallible seams.** ✅ *Landed in PR #235 and PR #249 (#244).*
   `Interner::try_resolve`, the non-mutating `Frontier::peek_least`, **and
   transactional exact candidate-delta application** all landed in #235:
   `Frontier::apply_delta` stages a private copy, performs checked
   `(key, expected)` removal (`RemoveMismatch`/`RemoveMissing`), inserts under
   the B^uk unique-key discipline (`InsertConflict`), and publishes only if
   every operation succeeds — leaving the frontier byte-identically unchanged
   on any error.

   > **Erratum (2026-08-06).** The 2026-08-05 re-pin stated that transactional
   > delta application was still outstanding and tracked it under #244. That was
   > wrong: #235 had already delivered it. The claim came from reading
   > `apply_delta`'s signature rather than its body, and it propagated into
   > #244's scope before being caught. #235 delivered more of this item than the
   > re-pin credited it with.

   PR #249 closed the genuine remainder: `SettlementRegime::decompose` is now
   the fallible `try_decompose(&self, e, c) -> Result<Decomposition,
   CommitError>`, with `CommitError` extended by `ChainLengthMismatch` and
   `EndpointMismatch` beside the existing `UnresolvedHandle` and
   `EmptyDecomposition`. `try_commit_selected` propagates a rejection instead of
   unwinding, so no `CommittedStep` is appended and no `Derived` judgement is
   minted on a malformed decomposition.

   A selected candidate remains in the frontier until its commit succeeds; its
   following candidate delta performs the removal. The adapter MUST NOT call the
   remaining panic paths in `Interner::resolve` or `commit_tick` for untrusted
   source-derived state.

   The `BTreeMap<Candidate, Key>` reverse index lives **Brix-side on the L3
   adapter**, not in `soc-core`: `Frontier<V>` stays generic, and
   `apply_delta`'s `&[(Key, V)]` removals already make the checked-removal
   contract expressible for a Brix-side index. The adapter stages copies of both
   maps before publishing an update.

   Making `SettlementRegime::decompose` fallible did **not** touch the
   quiescence certificate's enumeration-completeness field, which is a property
   of `Regime::candidates` alone (§4.2) — that signature is unchanged, so v1
   certificates remain sound and no v2 was needed.
7. **Plan-specific audit semantics.** The L3 runtime MUST construct the
   `GeneratorRegistry` and `GeneratorSemantics` supplied to `audit_journal`.
   The semantics checks the exact plan, rule, source world, destination world,
   and fact identity. The settlement loop itself MUST NOT publish `Audited`.
8. **A declared presentation and observation profile.** The runner MUST build
   a `saturate::PresentationV1` whose `id` is `PresentationIdV1` derived from
   `ProgramIdV1` (§3.1), whose `regime_set`/`adm_id` are the canonical
   identities of §3.4's regime and policy, and whose `profile` is the
   all-realizing `GeneratorPartitionProfile` of §4.1. `PresentationIdV1` is
   opaque to `soc-core`, which cannot compute or validate it (ADR-0014 §11);
   deriving it from anything but canonical artifacts — a source path, a file
   mtime, an interner handle — would silently corrupt every certificate taken
   against it, and is forbidden.

## 3. Canonical module-to-settlement mapping

### 3.1 Plan identity and admissible source

`L3PlanV1` is built from a parsed `ast::Module` after these validations. Its
canonical identity is `ProgramIdV1`, the exact program-revision identity for
this profile; it is not a source path or an in-memory AST allocation. More
precisely, it identifies the executable normalized plan. A satisfied source
type/grade annotation is checked but erased and does not change
`ProgramIdV1`; a changed normalized config, binding, rule, or order does.

- every selected `rule` has zero parameters;
- duplicate top-level config/`let`/rule names and cross-kind collisions are
  rejected;
- duplicate field/variant names within a config are rejected rather than
  normalized or last-write-wins;
- config type references must resolve in the complete module environment;
  the only primitive payload types are `Int` and `Str`, variant arity is
  preserved exactly, and direct or mutually recursive nominal configs are
  rejected in v1;
- every immutable `let`, whether referenced or not, normalizes in source order
  to a closed `L3ValueV1`; forward references, free variables, and recursive
  references are rejected;
- each selected rule body normalizes to a closed `L3ValueV1` in the declared
  nominal configuration environment, type-checks through the L2-compatible
  rules, and satisfies its declared return type when one is present;
- the profile marker is exactly `brix.l3.rule-agenda-saturated@1`; an unknown
  or mismatched profile is rejected before any engine state exists, and the
  retired `brix.l3.rule-agenda@1` is rejected like any other unknown marker;
- no unsupported execution item from §1 is silently skipped;
- names and ordering are preserved as written in `Module.items`.

L3 performs these payload checks itself; current `check_module` is not
sufficient. Every declared `let` type and rule return type is split into its
payload and optional grade. The payload MUST equal the normalized
`L3ValueV1`'s inferred primitive or nominal type. Unknown type names, wrong
constructor arity, and missing/extra/duplicate record fields reject the plan.
Any grade assertion is checked through the existing L2 grade rules but remains
irrelevant to settlement authority.

Canonical environment ordering is source declaration order. Within a record
config, fields use field declaration order; within a sum config, variants use
variant declaration order and payload types use parameter order. Names are
not hash-map iteration order or implicitly sorted.

The canonical plan preimage MUST include, in this order:

1. the fixed marker `brix.l3.plan`;
2. format version `1`;
3. the fixed execution-profile marker `brix.l3.rule-agenda-saturated@1`;
4. one normalized item stream in exact `Module.items` order, with an item tag
   followed by: config name and canonical ordered body; `let` name and
   `L3ValueId`; or rule ordinal, name, and `L3ValueId`.

Checked source annotations are absent from this executable stream. Keeping a
single tagged item stream means moving declarations across item kinds changes
the plan revision; the encoder does not silently regroup configs, bindings,
and rules.

The implementation MAY choose a concrete encoder shape, but it MUST freeze it
with independent vectors before making the identity durable. Existing
`ConfigId`, `GeneratorId`, `WitnessId`, and `ContextId` encoders are reused;
this ADR does not repurpose their domains.

`PresentationIdV1` is `PresentationIdV1(ProgramIdV1.digest())` — the same
revision identity under ADR-0014's opaque wrapper. It is deliberately not a
second, independently derived number: two identities for one revision is a
divergence waiting to happen, and ADR-0014 asks only that the value come from
canonical artifacts.

`L3ValueId` is the domain-separated canonical identity of `L3ValueV1`.
For rule ordinal `i`, `RuleId` is the domain-separated identity of
`("brix.l3.rule@1", ProgramIdV1, i, name)`. Generator and witness identities
are pinned without changing the existing semantic ID types: `GeneratorId` is
derived from the domain-separated preimage `("brix.l3.generator@1",
ProgramIdV1, RuleId, src, dst)`, and the candidate's witness handle interns
that generator's primitive `WitnessId`. The one-generator decomposition
therefore commits the same witness identity the candidate proposed. No
identifier is derived from a source path or raw interner handle.

### 3.2 Closed static values

`L3ValueV1` is the complete executable consequence language in this profile:

```text
L3ValueV1 ::= Int(i64) | Str(String)
            | Record { nominal_config,
                       fields: declaration-ordered [name, L3ValueV1] }
            | NullaryVariant { nominal_sum, variant }
```

Integer and string literals are decoded to their semantic values before
canonical encoding, so spelling/escaping differences do not create distinct
facts. Float literals are rejected in this v1 profile: `brix-canon`
deliberately excludes floating-point values from `Canonical`, so preserving a
float token's source spelling would be an invalid substitute for durable value
identity.

Record identity includes the declared nominal record configuration, not merely
its structural fields. The normalizer first verifies exact field-set equality,
then emits fields in config declaration order regardless of record-literal
source order. `NullaryVariant` includes both the declared nominal sum and
variant identity. Thus values such as two identically shaped records from
different declarations, or identically named variants from different sums,
cannot collapse.

The normalizer accepts only literal source forms, a record literal whose fields
are recursively closed static values, a validated nullary constructor, or a
reference to an earlier immutable `let` already normalized to `L3ValueV1`.
It rejects float literals, `Expr::Bin` (including arithmetic), `Expr::Call`
(including payload-bearing constructors), `Expr::Field`, `Expr::Match`,
`Expr::Prove`, `Expr::Why`, `Expr::Audit`, any function/rule call, parameters,
and unbound/forward variables. The resulting payload is canonical
`L3ValueV1` data; L3 v1 does not invoke the L2 evaluator or reuse a
typing/coverage grade as execution evidence.

### 3.3 World, facts, and transition

The world uses fixed-size canonical chain identities, not arrays re-encoded on
every commit:

```text
PendingV1    ::= Empty | Cons { rule: RuleId, tail: PendingIdV1 }
FactV1       ::= { rule: RuleId, payload: L3ValueId }
FactChainV1  ::= Genesis | Append { prior: FactChainIdV1, fact: FactV1 }
L3WorldV1    ::= { program: ProgramIdV1,
                   pending: PendingIdV1,
                   facts: FactChainIdV1,
                   fact_count: u64 }
```

The runner builds the duplicate-free pending suffix chain in reverse module
order and starts the fact chain at its versioned genesis digest. Every node
and world uses its own domain/version marker. A fact binds the publishing
`RuleId` and the canonical identity of the static normalized body; it is not
an untyped print string or a claim of general evaluation. Two rules MAY
publish the same payload because their facts remain distinct by `RuleId`.

For head node `Cons(r, tail)` and current chain `h`, the unique transition is:

```text
World(program, Cons(r, tail), h, n)
  -- g(program, r) -->
World(program, tail, Append(h, Fact(r, value(r))), n + 1)
```

The fixed-size successor preimage makes transition identity O(1); journal
replay reconstructs the ordered facts. During bounded setup, the runner
precomputes and interns the plan's `N + 1` deterministic prefix worlds and its
`N` head candidate triples; `Candidate` itself has no canonical identity, so
its regime, witness, and successor constituents are what get interned. The hot
regime therefore maps each nonterminal world handle directly to one
preconstructed candidate without scanning the pending agenda or hashing
source-sized data. Its footprint is exactly those `N + 1` world handles. The
initial input is `Delta::of_added([W0])`; transition `i` uses
`Delta::between_worlds(Wi, W(i+1))`. This bounded O(N) setup cost is distinct
from, and may be reported separately from, per-committed-step work as a
non-semantic setup diagnostic; it is never a committed `CostRecord`.

No non-head rule is eligible, and no rule is eligible once removed from the
pending suffix. This is the sole source of v1 termination; it is an
operational property of this profile, not a general-language termination
theorem.

`ExecConfig.world` interns the canonical `ConfigId` of this world.
`ExecConfig.history` starts at `History::empty().digest()`.

**Two limit sets, and only one of them is semantic ⟨D-LIM⟩.**

`PlanLimitsV1` contains the exact fields `max_selected_rules`,
`max_config_nodes`, `max_total_value_nodes`, `max_total_value_bytes`, and
`max_value_depth`, all canonically encoded as `u64`. A config node is one
declaration, field, variant, or variant payload type. A value node is one
constructor in §3.2; root depth is one. Total nodes/decoded string bytes count
every visit while normalizing each `let` and rule, including each substituted
occurrence, rather than deduplicating equal identities. These limits are
checked during plan validation and before the engine or journal exists;
exceeding them is `Rejected`, never truncated normalization or partial
candidate enumeration.

The execution budget is `saturate::SaturationBudget` —
`max_visible_steps`, `max_hidden_steps`, `max_administrative_states` — supplied
directly to `run_saturated`. It replaces the draft's `max_commits`, which was
the same bound under a pre-saturation name. It has no `Default`: a caller must
state all three (ADR-0014 §5.2's honesty discipline), including the two that
this profile's `𝒢_τ = ∅` renders unreachable. Declaring an unused bound is
cheap; inheriting an invisible one is not.

`RunContextV1` is a required canonical envelope with, in order: format
version, `ProgramIdV1` (the program revision), initial-world identity, exact
policy identity, exact profile marker, and `PlanLimitsV1`. The `ContextId`
passed to commit/audit is derived from this complete envelope;
`ContextId::root()` is not a valid public L3-run substitute.

**The `SaturationBudget` is excluded from `RunContextV1` and from every
canonical identity.** This overturns the 2026-08-02 draft, which folded
`max_commits` into the context on the argument that "a budgeted run [is]
unambiguously different from an otherwise-identical run with a different
resource contract." That argument holds for `PlanLimitsV1`, which decides
whether a plan is an admissible executable artifact at all — a plan rejected
for exceeding `max_value_depth` is a different question about a different
artifact. It does not hold for the step budget, and keeping it there would
break ADR-0014. `ContextId` is a field of `QuiescenceCertificateV1` (§6.2), so
budget-in-context would give two runs under different *sufficient* budgets two
different certificates for the same fact — contradicting ADR-0014 §6.2's
ratified rule that "two runs under different sufficient budgets identify the
*same* certificate," which rests on ADR-0013 §4: identity is a property of the
artifacts, not of the effort spent. ADR-0014 is Accepted; this profile yields.

The budget is reported in `SettlementRunV1` as a non-semantic diagnostic, so
an operator can still see the contract a run executed under.

### 3.4 Policy and calendar

`L3PolicyV1` is a canonical immutable envelope containing the plan identity,
the fixed profile marker, and the one compiler-owned regime identity. It
compiles to `AdmRegimeAllowlist` containing precisely
`RegimeId::named("brix.l3.rule-agenda-saturated@1")`'s interned handle.
There is no surface policy language in v1, and `AdmAll` is not an acceptable
public default.

`PresentationV1.adm_id` is the canonical digest of this envelope and
`PresentationV1.regime_set` the canonical digest of the ordered one-element
regime set. Both are carried into every quiescence certificate, which is what
makes a certificate's claim explicitly relative to *this* policy and *this*
regime set rather than absolute (§5).

For an eligible candidate `c` at phase `n`, the key is:

```text
Key {
  phase: n,
  priority: rule's module-order ordinal,
  tiebreak: H("brix.l3.key@1", plan, resolved(c.regime),
              resolved(c.witness), resolved(c.successor))
}
```

`resolved` means the canonical digest recovered from the same `Interner` at
the commit boundary. Raw `Handle` indices MUST NOT be used in a durable L3 key
or replay identity; they are local implementation details. The keyer MUST
detect/report a `Frontier` conflict, never let map insertion silently choose a
candidate. This maps directly to `Key::new` and `commit_tick`'s existing
unique-key enforcement.

## 4. Deterministic saturated loop and budgets

### 4.1 The observation profile is all-realizing ⟨D-TAUZERO⟩

ADR-0014 §3.2 makes τ-ness **declared**, not intrinsic: an observation profile
classifies each committed step, and `Administrative` means only that the
declared boundary does not export that step's observation.

This profile declares `𝒢_τ = ∅`. Its `GeneratorPartitionProfile` is built with
`all_realizing(G)` where `G` is exactly the plan's `N` generators
`g(program, rᵢ)` from §3.1 — no more, so that an unregistered generator is a
fail-closed `ProfileError::UnregisteredGenerator` rather than a silent label.

The consequences must be stated plainly rather than left to look like depth:

- **Saturation over this profile is the identity.** Every committed step is
  realizing, so every `sat_step` hides a τ-prefix of length zero and returns
  `Realizing { hidden_steps: 0, .. }`. This is precisely ADR-0014 §3.2's
  degenerate case, "under which `sat_step` behaves exactly like `commit_tick`."
- **`SaturatedStep::Divergent` is structurally unreachable.** A divergence
  certificate certifies an administrative lasso; with no administrative steps
  there is no τ-lasso to close. §5 retains the summand for totality, and treats
  its appearance as an integrity failure rather than suppressing it.
- **`max_hidden_steps` and `max_administrative_states` are unreachable bounds**
  for this profile, and are still declared (§3.3).

What the re-pin buys, then, is not hidden depth. It is (a) the certified `1`
summand — `Quiescent` is re-derivable by a checker, where `PlanComplete` was
L3's own bookkeeping — and (b) a driver on the interface that later profiles
grow into. A profile that adds guards, multi-rule deliberation, or retraction
introduces genuine administrative steps by moving generators into `𝒢_τ` and
minting a new profile marker; it does not rewrite the driver.

### 4.2 Enumeration completeness binds `Regime::candidates` ⟨D-CAND⟩

ADR-0014 §6.2 makes enumeration completeness "the load-bearing honesty field"
of the quiescence certificate: an empty frontier is a decided negative *only
if* the enumeration was exhaustive. That holds in v1 solely because
`Regime::candidates -> Vec<Candidate>` is unbounded and total, and the v1
reader accepts only the `Complete` ordinal.

The 2026-08-02 draft called a fallible bounded regime API "a later compatible
extension." **It is not compatible with this profile, and that classification
is withdrawn.** Normatively:

> While this profile emits v1 quiescence certificates, `Regime::candidates`
> MUST remain unbounded and total. Introducing a bounded or fallible
> candidate-enumeration API requires both a v2 quiescence certificate in
> `soc-core` and a new L3 execution-profile marker. It is a new profile, not a
> widening of this one.

`crates/soc-core/OWNER.md` records the same constraint from the other side, and
#244 honored it: PR #249 made decomposition fallible while leaving
`Regime::candidates`'s signature untouched, so v1 certificates remain sound.

**What this costs L3: nothing, and the reason is structural.** The head-only
regime of §3.3 emits **at most one candidate per world** by construction, from a
precomputed transition-table lookup. There is no enumeration to bound. The plan's
`max_selected_rules` bounds `N` during validation, before the engine exists, so
the total candidate work of a whole run is fixed before a single step is taken.
An unbounded-total `candidates` is free here precisely because this profile
never enumerates. A future profile with a genuinely large or
dynamically-generated candidate set is the one that will have to pay, and it
will pay by minting a v2 certificate.

L3 MUST NOT claim a candidate-enumeration, wall-clock, or audit-work budget was
enforced; `SaturationBudget` bounds steps, not enumeration. Kernel budgets apply
only if a caller separately requests proof elaboration; L3 does not invoke the
proof kernel automatically.

### 4.3 The loop

For a validated plan, a `SaturationBudget`, and the presentation of §2 item 8,
L3 executes:

1. construct a fresh `Interner`, intern plan/world/policy/regime/witness
   identities, and construct `ExecConfig`;
2. initialize `IncrementalEngine` with the compiler-owned dual regime, apply
   the initial world delta, filter candidate additions through the exact `Adm`
   policy, and materialize only admitted candidates into a keyed `Frontier`;
   this initialization is not a committed step;
3. drive `saturate::run_saturated(&presentation, e0, &mut keyer, budget)`. The
   adapter supplies the keyer of §3.4 and maintains its incremental frontier
   and reverse index alongside; it does not re-implement stepping, selection
   semantics, or the stop vocabulary;
4. before every selection, compare the incremental admissible view (and its
   keyed least candidate) with the naive `Regime::candidates` relation filtered
   through the same `Adm` policy for the presented world. A mismatch is an
   adapter integrity failure — an `Unknown`, never a commit, and never a
   certificate;
5. for each step, peek at the least key without mutation and obtain its pure
   `prospective_successor` from `soc-core`. Derive `step_world_delta` and apply
   it to the private incremental engine. On staged frontier/reverse-index
   copies, remove the old candidate and insert additions with phase `n + 1`.
   Compare the resulting admitted view/key with the naive prospective-state
   relation. If those checks pass, commitment proceeds through
   `try_commit_selected`; require its successor to equal the prospect, then
   append/publish its returned step and swap in the staged frontier as the new
   public state. Any failed update, comparison, or commit validation aborts as
   an `Unknown` with only the earlier `ExecConfig`/journal prefix exposed; the
   mutated private engine and staged maps are discarded;
6. when the frontier is empty, `sat_step` re-enumerates and mints a
   `QuiescenceCertificateV1`; `run_saturated` returns
   `SaturatedStop::Quiescent`. L3 does not decide this by inspecting its own
   pending list. That the two coincide — the agenda is empty exactly when the
   frontier is — is a *derived property of this profile's transition relation*
   (§3.3: no non-head rule is eligible, and none is eligible once removed), and
   §9 Stage C fixture 12 asserts the coincidence rather than assuming it.

**A zero visible budget establishes nothing, even for an empty agenda.**
`run_saturated` checks `max_visible_steps` at the top of its loop, before
calling `sat_step`, so `max_visible_steps = 0` returns
`Unknown(VisibleBudgetExhausted)` regardless of the plan. This is deliberate
and is retained rather than special-cased: certified quiescence requires that
the frontier actually be enumerated, and under a zero budget it never was.
The draft's contrary rule — an empty plan at zero is `PlanComplete` — was a
bookkeeping answer available only to a status that certified nothing. Under
ADR-0002 §5.3, a search that has not run has proved nothing.

The budget is an operational safety bound, not a proof of nontermination. It
MUST be reported in the run result and does not alter the plan identity.

The current `soc_core::run`/`commit_tick` are deliberately naive reference
paths and MUST NOT be the public L3 driver. `IncrementalEngine` currently
maintains only a candidate view, so the required adapter supplies the missing
calendar/decomposition/journal integration. The public path MUST meet the
per-committed-step cost bound `O(|Delta| × fanout)` and include inert-world and
inert-regime ballast gates proving that unrelated population does not increase
step work. The head-only rule regime MUST emit exactly one candidate removal
and at most one addition for a committed world delta, independent of the
number of remaining rules. It MUST NOT scan or re-emit the whole pending
agenda. Differential equality against the naive relation is required after
every step, not merely at the end of a fixture.

`IncrementalEngine::StepReport` counts produced entries but cannot observe
work hidden inside `IncrementalRegime::apply`. The L3 regime therefore MUST
implement one shared `apply_counted(delta) -> (CandidateDelta, probe_count)`;
its trait `apply` delegates to that function. The deterministic count is one
precomputed world-transition-table lookup per touched handle and no
agenda-element probes. Conformance tests inspect this L3-local count in
addition to the core `CostRecord`; optional production reporting is a
non-semantic diagnostic.

## 5. Result vocabulary, authority, and replay

The post-plan status **is** ADR-0014's `SaturatedStop`, carried through
unchanged rather than translated into a parallel L3 vocabulary ⟨D-STATUS⟩:

| Status | Meaning | Grade claim |
|---|---|---|
| `Quiescent(cert)` | The admissible frontier at the terminal world is empty under a complete enumeration, in this context, under this policy and regime set, at this presentation revision. | The **only** decided negative. `Derived`-grade in the certificate's exact context/profile/revision — never a theorem, never `Audited` or `Proven`, never a fixpoint claim about Brix. Logged steps are `Derived`. |
| `Divergent(cert)` | A certified administrative lasso. **Structurally unreachable in this profile** (§4.1). | `Unknown` for completion/quiescence; never `Refuted`, never the `1` summand. If ever observed, see below. |
| `Unknown(reason)` | Every `SaturationUnknown` variant, plus adapter-local integrity failures. | `Unknown`; establishes nothing; never an invented commit. Earlier logged steps remain `Derived`. |
| `Rejected` | Parse, profile, closure, static-value, plan-limit, or plan-validation failure. | Pre-run; no settlement fact and no run envelope at all. |

`PlanComplete`, `FrontierStalled`, and `CommitBudgetExhausted` are **retired
unimplemented**. No build ever emitted them, so no identity is reinterpreted
and ADR-0014 §5's disjointness requirement is met vacuously. They MUST NOT be
reintroduced as aliases of the statuses above.

Their replacements:

- `PlanComplete` → `Quiescent(cert)`. Strictly stronger: a checker re-derives
  the claim from the presentation and journal (§9 Stage C fixture 12) instead
  of trusting the runner's pending list.
- `CommitBudgetExhausted` → `Unknown(VisibleBudgetExhausted { .. })`, which
  already carries the visible-step count and the bound.
- `FrontierStalled` → `Quiescent(cert)` **plus an agenda-residue diagnostic**
  ⟨D-RESIDUE⟩, argued next.

**Why a denied agenda is genuine quiescence.** The draft treated "pending rules
remain but the frontier is empty" as its own inconclusive status, on the
reasoning that the system had not really finished. Under ADR-0014 §6.1 that
reasoning does not survive contact with what the certificate actually asserts:
emptiness of the admissible frontier *under this policy and this regime set*,
asserting "nothing about any other context, profile, revision, policy, or
regime set." A policy that denies a rule makes that rule inadmissible, and a
world with no admissible candidate is quiescent — under that policy. The
certificate is valid, checkable, and correctly scoped. Inventing a separate
inconclusive status would understate a claim the substrate can actually
certify.

What must not be lost is the operational fact that rules were never admitted.
`SettlementRunV1` therefore carries `agenda_residue: u64`, the count of rules
still pending at the terminal world. It is a **non-semantic diagnostic**: it is
excluded from the certificate and from the run's canonical identity, because
it is a statement about L3's bookkeeping and not about the coalgebra. A
`Quiescent` result with `agenda_residue > 0` MUST be surfaced as quiescent
*under this policy, with N rules never admitted* — never as an unqualified
success, and never as "complete."

**If `Divergent` is ever observed**, the adapter MUST report an adapter
integrity failure with a distinguished reason code and MUST NOT return it as a
settlement result: with `𝒢_τ = ∅` a τ-lasso certificate is a contradiction
between the profile and the engine, and the honest response to a contradiction
is `Unknown`. The adapter MUST NOT discard or suppress the certificate; it is
retained as a diagnostic for the bug report it represents.

On every committed step, only `soc_core::try_commit_selected`, shared with
`commit_tick`, constructs the `Derived` judgement/`Observation`. The public
adapter may publish, display, or return that identity, but MUST NOT construct
an alternative settlement judgement or relabel it `Audited` or `Proven`.

The settlement `Derived` judgement is distinct from any type-checking result or
`match … proving exhaustive` coverage certificate used while compiling a rule
body. A body may already have a `Proven` typing/coverage claim, but that never
upgrades the later committed settlement observation; the two propositions,
evidence routes, and authorities remain separate.

An optional explicit audit action calls `audit_journal` with the plan-specific
registry/semantics. Only `AuditResult::Audited` produces a separate `Audited`
judgement and dependency; `AuditResult::Unknown` is returned as unknown audit
status. A request for `Proven` is a separate proof-kernel elaboration flow over
audited support. These are the sole authority routes from ADR-0002 §4.1.

`SettlementRunV1` is a required canonical result envelope containing:

- program, `RunContextV1`, `PresentationIdV1`, `ObservationProfileId`, initial
  world, and policy identities;
- the exact `PlanLimitsV1`, final resolved world/policy state, and total stop
  reason;
- on `Quiescent`, the `QuiescenceCertificateId`;
- ordered committed step digests and `Journal::chain_digest()`;
- no raw-handle `ExecConfig.history`, `SaturationBudget`, `agenda_residue`,
  cost records, or audit reports.

`Rejected` is a pre-run compiler outcome and therefore has no fabricated
`ProgramIdV1`, context, journal, or `SettlementRunV1`. Every post-plan status
is carried by `SettlementRunV1`; `Unknown` additionally carries a stable
versioned reason code, while human diagnostic text remains outside the
semantic identity.

The certificate's *identity* is in the envelope; the certificate itself is
carried in the operational result. Two runs of the same plan under different
sufficient budgets MUST produce the same `SettlementRunV1` identity and the
same `QuiescenceCertificateId` (ADR-0014 §6.2), which is what ⟨D-LIM⟩ in §3.3
exists to protect.

Its replay identity is valid only when a fresh interpreter reconstructs the
same plan, rebuilds the same journal in order, and obtains byte-identical
`Journal::replay_chain(steps)` and final chain digest. A matching digest alone
does not certify rule semantics; audit remains the tight boundary.

The operational result MAY carry the final `ExecConfig`, per-step
`CostRecord`s, the `SaturationBudget`, `agenda_residue`, the certificate
itself, and an optional audit report as non-semantic diagnostics. Those fields
are excluded from `SettlementRunV1`'s semantic identity: current
`ExecConfig.history` folds raw interner handles and is therefore not stable
across interner allocation orders; costs and budgets are observational;
residue is L3 bookkeeping; audits are separate judgements with their own
evidence identities. A mandatory cross-interner-order fixture MUST rebuild the
same plan with distinct intern insertion orders and obtain identical
`ProgramIdV1`, `RunContextV1`, `PresentationIdV1`, `ObservationProfileId`, step
identities, journal chain, `QuiescenceCertificateId`, and `SettlementRunV1`
identity.

`SaturatedRun::chain_digest()` MUST NOT be used to decide behavioral agreement
between two runs — ADR-0014 risk 3: weakly bisimilar systems have different
journals by design. L3 uses it only to check that one run replays
deterministically.

## 6. Fail-closed and security invariants

1. **No silent omission.** Unsupported declarations or rule forms reject the
   plan. L3 never executes a module after dropping `regime`, `show`, or a
   parameterized rule from its meaning.
2. **Canonical boundaries only.** Handles are fresh-run implementation state;
   all public plan, world, generator, witness, key tie-break, and run
   identities use resolved canonical artifacts. Raw-handle history and
   diagnostics are excluded from semantic replay identity.
3. **One candidate, one decomposition.** A committed v1 rule has a nonempty,
   plan/rule-bound generator decomposition and exact world endpoints. An empty
   decomposition, missing interner entry, or malformed candidate is a
   fallible `Unknown`/rejection, not a panic exposed as a successful
   run. The L3 regime's `try_decompose` MUST require all of:
   `candidate == transition_table[current_world]`, the resolved candidate
   witness equals the expected generator's primitive `WitnessId`,
   `decomposition.generators == [expected_generator]`, and
   `decomposition.configs == [expected_src, expected_dst]`.
4. **Deterministic incremental selection.** Module-order priority plus
   canonical tie-break must make `select_K` total. Candidate-delta removals and
   additions update the maintained frontier through fallible operations; a
   `KeyConflict` or naive/incremental differential mismatch is an integrity
   failure.
5. **No grade laundering.** `Derived` is the only grade from the hot loop;
   audit and kernel proof retain their separate authorities and evidence.
6. **Only a verified certificate reports settlement.** `Quiescent` is the sole
   decided negative, and only via a `QuiescenceCertificateV1` that
   `check_quiescence_certificate` re-derives. Budget exhaustion, a profile
   error, a commit failure, a key conflict, and every adapter integrity
   failure are `Unknown` — never `Refuted`, never `Audited`, never a
   certificate. A certificate that fails its checker is `Unknown`, not a
   downgraded pass, and the run MUST NOT report quiescence on the strength of
   the runner having minted one.
7. **A certificate is scoped, and MUST be reported scoped.** Its claim is
   relative to a context, observation profile, presentation revision, policy,
   and regime set. It MUST NOT be presented as a fixpoint of the program, as a
   property of the language, or as holding under any other policy — including
   when `agenda_residue > 0` (§5).
8. **No source-text identity.** Comments, formatting, and path names cannot
   affect canonical plan or replay identity. Explicit version fields protect
   encoder evolution.

## 7. What this profile consumes from ADR-0014, and what it does not

#61 is closed; ADR-0014 is Accepted and implemented. This section replaces the
draft's boundary-against-an-open-issue with the actual consumption contract.

**Consumed.** L3 is a *client* of the saturated interface, not a
reimplementation of it:

- `PresentationV1` — L3 supplies the presentation (§2 item 8); `soc-core` owns
  stepping.
- `run_saturated`/`sat_step` — the driver. L3 does not write its own stepping
  loop, its own quiescence test, or its own stop vocabulary.
- `SaturatedStop` — carried through verbatim as the post-plan status (§5).
- `QuiescenceCertificateV1` and `check_quiescence_certificate` — the success
  claim and its independent checker.
- `GeneratorPartitionProfile` — the declared observation boundary (§4.1).
- `SaturationBudget` — the execution bound, excluded from identity (§3.3).

**Not consumed, and not implied.** Saturation supplies a settlement interface.
It does not supply meaning, and this profile gains none of the following from
adopting it:

- **rule-body evaluation semantics.** A rule body remains a canonical static
  consequence payload (§3.2). Saturation does not make `1 + 2` reducible.
- **any authority upgrade.** The hot loop still mints only `Derived`, only
  through `try_commit_selected`. A quiescence certificate is itself
  `Derived`-graded; `Audited` still requires `audit_journal`, and `Proven`
  still requires kernel elaboration (ADR-0002 §4.1). Certification and grading
  are different axes, and holding a verified certificate upgrades nothing.
- **a fixpoint theorem, or any general-language termination claim.** v1
  termination remains the operational property of §3.3: the pending suffix
  strictly shrinks.
- **administrative depth.** `𝒢_τ = ∅` (§4.1).
- **behavioral comparison.** ADR-0014's weak bisimulation, refinement, and
  counterexamples exist and are available, but this profile neither runs nor
  needs them: it compares nothing. A later profile comparing two revisions is
  a separate slice.
- **correction/retraction or cross-revision invalidation semantics.** Still
  unaddressed by either ADR; still #59/#178 integration work.

**Direction of the boundary.** ADR-0014 §7 is the other side of this one.
Where the two could conflict, ADR-0014 is Accepted and governs: §3.3's ⟨D-LIM⟩
resolves a budget-identity conflict in ADR-0014's favor, and §4.2's ⟨D-CAND⟩
withdraws a draft extension that would have violated ADR-0014 §6.2's
enumeration-completeness rule.

Accordingly, #178's older "drive `commit_tick` to a fixpoint" wording is
planning shorthand superseded by this pin: the public path is the incremental
adapter above driving `run_saturated`, and it reports certified quiescence at a
world under a policy — not a fixpoint.

## 8. Non-goals

L3 v1 does not:

- define general evaluation, effects, recursion, or a value store for Brix;
- execute surface `regime`/`gen` bodies or provide a user-authored policy
  language;
- make every `let` a settlement fact or change L2's type-checking grades;
- replace the naive reference oracle; it instead requires the incremental
  engine's O(Δ) gate and step-by-step differential comparison;
- auto-audit, auto-prove, or establish theoremhood from a completed run — a
  verified quiescence certificate is `Derived`, and upgrading it is a separate
  authority route;
- introduce administrative steps, weak transitions, or behavioral comparison
  (§4.1, §7);
- provide distributed replication or scheduling parallelism.

## 9. Staged implementation and acceptance fixtures

### Stage A — plan and artifact vectors

Implement plan validation, canonical encoders, and frozen independent vectors
for one plan, rule, initial world, policy, generator, witness, and fact. Tests
MUST prove whitespace/comment-insensitive source mapping by parsing equivalent
modules to the same lowered plan identity. They MUST also reject duplicate
rules/configuration members, unclosed/forward/recursive `let` references,
recursive/mutually recursive configs, unknown or unsupported type names,
profile mismatch, calls/arithmetic/matches/fields, and payload-bearing
constructors. They MUST reject float literals and integer overflow. They MUST
prove that equivalent integer/string spellings and escapes normalize to the
same semantic value identity, and that reordered record-literal fields yield
the same declaration-ordered `L3ValueId`. Missing, extra, and duplicate record
fields MUST reject. Plan-limit failures MUST occur before engine or journal
construction.

Vectors additionally freeze `PresentationIdV1` and the `ObservationProfileId`
of the plan's all-realizing profile, and a fixture MUST assert
`PresentationIdV1 == PresentationIdV1(ProgramIdV1.digest())` so the two
revision identities cannot drift apart.

**Fragment validation may land before the encoders** — it depends only on §1,
§3.1, and §3.2, none of which the re-pin touched. An encoder-free slice must
assert structural plan equality where this stage asserts identity equality,
and must not mint any identity type.

### Stage B — one-shot dual compiler regime and the observation profile

Implement `Regime` + `IncrementalRegime`, the plan's footprint, fallible
decomposition (over `SettlementRegime::try_decompose`, landed in #249), and
exact `Decomposition` construction. Fixture: two zero-argument rules in reverse lexical name order.
The module-order rule MUST commit first, then the other; each decomposed
endpoint MUST equal the pre/post canonical L3 worlds.

Also construct the `GeneratorPartitionProfile`. Fixtures: its realizing
partition is exactly the plan's `N` generators and its administrative
partition is empty; its `ObservationProfileId` is stable across interner
orders; every committed step labels `Realizing`; and a step carrying a
generator outside the plan yields `ProfileError::UnregisteredGenerator` rather
than any label.

### Stage C — incremental calendar adapter, saturated driver, and replay

Implement incremental candidate-delta → keyed-frontier maintenance, the
`run_saturated` integration, and the semantic/diagnostic result split.
Fixtures:

1. an empty-rule module returns `Quiescent` with an empty journal and a
   certificate that verifies;
2. two rules with a sufficient budget return `Quiescent`, two `Derived`
   observations, two entries in `SaturatedRun::visible`, a two-step journal,
   and a reproducible replay chain;
3. the same two rules with `max_visible_steps = 1` return
   `Unknown(VisibleBudgetExhausted)`, one committed record, **and no
   certificate of any kind** — the sharpened no-false-quiescence fixture;
4. the same plan with `max_visible_steps = 0` returns
   `Unknown(VisibleBudgetExhausted)` with an empty journal, **and so does an
   empty plan at zero** (§4.3): a search that never ran certifies nothing;
5. a deliberately denying policy returns `Quiescent` with a verifying
   certificate, `agenda_residue > 0`, and a surfaced qualification — never an
   unqualified success and never the word "complete";
6. unsupported/parameterized/non-static rules reject before any journal is
   created;
7. deliberately colliding calendar keys or malformed decompositions fail
   closed as `Unknown(KeyConflict)` / `Unknown(CommitFailed)` rather than
   selecting or minting a result;
8. a candidate-witness/generator mismatch, wrong transition-table candidate,
   or wrong decomposition endpoint fails before `Derived` publication;
9. after every step, incremental candidates and selected key equal the naive
   relation; inert configuration/regime ballast leaves the measured per-step
   cost within the `O(|Delta| × fanout)` gate. Per ADR-0014 §5.2 that
   invariant is quantified over committed γ-steps; this profile's saturated
   and γ-steps coincide 1:1 (`hidden_steps == 0` on every step, asserted), so
   the existing gate applies verbatim;
10. increasing trailing agenda ballast leaves one head commit's candidate
    delta, core work, and L3-local apply probe count unchanged: one removal,
    at most one addition, and one lookup per touched world handle;
11. two fresh interners initialized in different insertion orders produce the
    same semantic run/replay identities — including `PresentationIdV1`,
    `ObservationProfileId`, and `QuiescenceCertificateId` — while their raw
    engine histories may differ;
12. **the certificate is independently checkable, and the agenda/frontier
    coincidence is asserted rather than assumed.**
    `check_quiescence_certificate` re-derives the claim from the presentation
    and journal prefix and returns `Verified`; the terminal world's pending
    list is independently observed empty; and tampering with any of the
    journal prefix, terminal world, regime set, `Adm` identity, context,
    profile, or presentation revision yields `Unknown`, never a pass;
13. **no run of any valid plan returns `SaturatedStop::Divergent`**, and no
    committed step ever labels `Administrative` — the structural consequence
    of `𝒢_τ = ∅` (§4.1), asserted so that a future profile change that
    introduces τ steps cannot do so silently;
14. two runs of the same plan under different *sufficient* budgets produce the
    same `SettlementRunV1` identity and the same `QuiescenceCertificateId`
    (⟨D-LIM⟩, ADR-0014 §6.2).

For a post-plan outcome, the first CLI surface added after these fixtures MUST
print the program, context, presentation revision, observation profile,
semantic run, journal-chain, ordered `Derived` step identities, and one exact
status name from §5. `Rejected` instead prints deterministic plan diagnostics
and no invented semantic identifiers. The CLI MUST use a nonzero exit status
for `Rejected` and every `Unknown`. It MAY print "quiescent" **only** when it
holds a `Quiescent` whose certificate its own checker verified, MUST qualify
that output when `agenda_residue > 0`, and MUST NOT print "fixpoint",
"settled", `Audited`, or `Proven` for any settlement outcome.

### Stage D — audit boundary

Use the plan-specific registry/semantics with `audit_journal`. Fixture: the
unchanged journal yields distinct linked `Derived` and `Audited` judgements;
tampering with a rule/world/fact/decomposition yields `AuditResult::Unknown`.

One further fixture keeps the two axes apart: auditing the journal of a
`Quiescent` run MUST NOT change the quiescence certificate, its identity, or
its `Derived` grade. Certification and grading are orthogonal (§7); an audited
journal supporting a quiescence claim is not an audited quiescence claim.

No CLI `brix run` command is added before Stages A–C have the above fixtures.
No `@Audited` display or proof command is added before Stage D's authority
fixtures pass.

## 10. Compatibility and evolution

- New L3 artifact fields, statuses, and encoders are append-only and versioned.
  Existing v1 identities never change in place.
- The existing `soc-core` `CommittedStep` ABI remains authoritative; the L3
  result envelope wraps it and does not copy/re-encode it as an alternate log.
- Adding source-level transition guards, parameters, payload-bearing
  constructors, general regimes, effects, administrative steps, or
  candidate-work limits requires a new ADR slice and a new execution-profile
  marker. It cannot broaden this profile by accepting formerly rejected forms
  under the same `brix.l3.rule-agenda-saturated@1` identity.
- `brix.l3.rule-agenda@1` is permanently retired and MUST NOT be minted.
- A bounded or fallible `Regime::candidates` additionally requires a v2
  quiescence certificate in `soc-core` before any profile may use it (§4.2,
  ⟨D-CAND⟩; ADR-0014 §6.2).
- A future implementation may replace the compiler-owned one-shot regime with
  a richer lowering only after differential fixtures establish equivalent
  behavior for the v1 fragment and audit replay remains byte-identical.

## 11. Decisions ratified here

| Mark | Decision | §|
|---|---|---|
| ⟨D-PROFILE⟩ | The profile marker is `brix.l3.rule-agenda-saturated@1`; the blind `brix.l3.rule-agenda@1` is retired unimplemented and never minted. | §1 |
| ⟨D-LIM⟩ | `RunLimitsV1` splits into semantic `PlanLimitsV1` (in `RunContextV1`'s identity) and non-semantic `SaturationBudget` (excluded). Overturns the draft's §3.3. | §3.3 |
| ⟨D-TAUZERO⟩ | The observation profile is all-realizing over exactly the plan's generators: `𝒢_τ = ∅`. Saturation over this profile is the identity, and this document says so rather than implying depth. | §4.1 |
| ⟨D-CAND⟩ | While this profile emits v1 quiescence certificates, `Regime::candidates` MUST stay unbounded and total. A bounded/fallible API is reclassified from "later compatible extension" to a new profile requiring a v2 certificate. | §4.2 |
| ⟨D-STATUS⟩ | The post-plan status is ADR-0014's `SaturatedStop` carried through verbatim. `PlanComplete`/`FrontierStalled`/`CommitBudgetExhausted` are retired unimplemented. | §5 |
| ⟨D-RESIDUE⟩ | A denied agenda is genuine policy-relative quiescence, reported as `Quiescent` plus a non-semantic `agenda_residue` diagnostic that MUST qualify the output. | §5 |

## 12. Open blockers

1. `brix-lower` needs a public/module-level checked lowering path for rules;
   today its public `check_module` processes `let` declarations only. Unchanged
   by the re-pin — the fragment of §1/§3.1/§3.2 is exactly what it was — so
   this may proceed in parallel with the encoder work.
2. The exact canonical schemas and frozen vectors for the new L3 artifacts
   need a separate encoder review; this ADR fixes their required inputs and
   versioning, not their byte tags. The re-pin adds `PresentationIdV1`, the
   `ObservationProfileId`, and the certificate identity to what must be frozen,
   and removes the budget from `RunContextV1` (⟨D-LIM⟩).
3. ✅ **Discharged in full.** `try_commit_selected`, `prospective_successor`,
   `Interner::try_resolve`, `Frontier::peek_least`, **and** transactional
   candidate-delta application landed in PR #235; the fallible
   `SettlementRegime::try_decompose` landed in PR #249 (#244). §2 item 6 is now
   complete — see the erratum there correcting this ADR's earlier claim that
   delta application was still outstanding.
4. The current audit API is unbudgeted; a resource-bounded audit operation is
   required before L3 can make any audit-latency guarantee. **The re-pin does
   not discharge this.** `SaturationBudget` bounds stepping, not auditing; they
   are different axes, and a budgeted saturated run says nothing about audit
   cost.
5. ✅ **Discharged.** #61 is closed and
   [ADR-0014](./ADR-0014_Divergence_Sensitive_Saturation.md) is **Accepted**
   with all four stages landed. This profile consumes that interface under the
   new marker of ⟨D-PROFILE⟩ and does not re-grade any prior status, because
   no prior status was ever emitted.

## 13. Open decisions

None. Every question the re-pin opened is ratified in §11. Two are worth
re-examining if the profile ever grows:

- ⟨D-RESIDUE⟩ reports a denied agenda as quiescence-plus-diagnostic. That is
  correct for a profile whose only policy is the built-in allowlist, where
  denial can only be injected by a test. A profile with a real surface policy
  language should revisit whether residue deserves promotion from diagnostic
  to semantic — it would then be a fact about an authored policy, not about
  L3's bookkeeping.
- ⟨D-TAUZERO⟩ is what makes `Divergent` unreachable and the O(Δ) gate apply
  verbatim. The first profile to move a generator into `𝒢_τ` invalidates both
  conveniences at once and must re-derive them, not inherit them.
