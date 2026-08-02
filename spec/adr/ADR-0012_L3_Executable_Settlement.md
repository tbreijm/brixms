# ADR-0012 — Brix L3: Executable Settlement, v1 Rule-Agenda Profile

Status: **Proposed** (2026-08-02). This is the implementation pin for the
first executable Brix settlement path. It extends
[ADR-0010](./ADR-0010_SOC_Language_Design.md)'s L3 goal, under the authority
and failure rules of the [SOC Constitution](./ADR-0002_SOC_Constitution.md).
It is intentionally narrower than a general Brix runtime: it turns a parsed
module's finite, zero-argument `rule` declarations into a deterministic,
auditable settlement agenda. It does not assign general evaluation semantics
to rule bodies.

Date: 2026-08-02.

Foundation: [ADR-0002](./ADR-0002_SOC_Constitution.md) §§4–5, §8–9;
[ADR-0010](./ADR-0010_SOC_Language_Design.md) §§3, 5, 7; the current
`soc-core::{commit, calendar, journal, audit}` APIs; and SOC-LAW-08,
SOC-LAW-10, and SOC-LAW-11 in
[`spec/SOC_Semantic_Laws.md`](../SOC_Semantic_Laws.md). This ADR is a bounded
runtime bridge in the #178 area. It does **not** discharge #61's saturation,
quiescence-certificate, or observable-behavior obligations.

---

## 1. Decision

L3 v1 SHALL compile a supported `.brix` module into one finite settlement
plan, execute that plan through `soc-core`'s keyed committed coalgebra, and
return an explicit, replay-addressable run result. Its total stop vocabulary
distinguishes bounded-agenda completion, a stalled unsaturated frontier, and
commit-budget exhaustion. Only bounded-agenda completion is successful. None
reports semantic quiescence or a settled fixpoint.

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
step per selected rule. `PlanComplete` means this profile has consumed its
finite pending-rule set; it is not a general Brix fixpoint theorem. The profile
exercises the real calendar, commit, journal, replay, audit, and incremental
candidate boundaries without pretending that the present AST already has a
full transition/effect language.

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
4. **An incremental settlement adapter and total bounded driver.** The public
   runner MUST integrate `IncrementalEngine`, an incrementally maintained
   keyed `Frontier`, calendar selection, fallible decomposition, journal
   append, and a final execution state/stop reason. It MUST compare the
   incremental admissible candidate view and selected key against the naive
   `Regime` relation filtered through the exact `Adm` policy after every
   committed step. `soc_core::run` and `commit_tick` stay reference-only; they
   cannot back the public path. The adapter MUST convert
   source-derived malformed-plan/key/decomposition conditions to a failure
   result rather than expose a panic.
5. **One settlement-authority path.** `soc-core` MUST factor a fallible
   `try_commit_selected` primitive out of `commit_tick`. Given an already
   selected key/candidate and its `SettlementRegime`, this primitive alone
   validates/decomposes the candidate, derives the generator-chain witness,
   constructs the `Derived` judgement/`Observation` and `CommittedStep`, and
   advances `ExecConfig`. Both naive `commit_tick` and the incremental adapter
   MUST call it. The Brix-owned adapter selects and schedules; it never mints a
   settlement judgement. A shared pure `prospective_successor` operation
   exposes the existing oracle successor fold without constructing an
   observation; `try_commit_selected` MUST use that same operation.
6. **Fallible seams.** Required additions are `Interner::try_resolve`, a
   non-mutating `Frontier::peek_least`, transactional exact candidate-delta
   application (checked `(key, expected_candidate)` removal plus
   conflict-detecting insertion), and a
   fallible `try_decompose`/validation interface. A selected candidate remains
   in the frontier until its commit succeeds; its following candidate delta
   performs the removal. The adapter MUST NOT call the current panic paths in
   `Interner::resolve` or `commit_tick` for untrusted source-derived state.
   The adapter also maintains a `BTreeMap<Candidate, Key>` reverse index and
   stages copies of both maps before publishing an update.
7. **Plan-specific audit semantics.** The L3 runtime MUST construct the
   `GeneratorRegistry` and `GeneratorSemantics` supplied to `audit_journal`.
   The semantics checks the exact plan, rule, source world, destination world,
   and fact identity. The settlement loop itself MUST NOT publish `Audited`.

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
- the profile marker is exactly `brix.l3.rule-agenda@1`; an unknown or
  mismatched profile is rejected before any engine state exists;
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
3. the fixed execution-profile marker `brix.l3.rule-agenda@1`;
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

`RunLimitsV1` contains the exact fields `max_selected_rules`,
`max_config_nodes`, `max_total_value_nodes`, `max_total_value_bytes`,
`max_value_depth`, and `max_commits`, all canonically encoded as `u64`. A
config node is one declaration,
field, variant, or variant payload type. A value node is one constructor in
§3.2; root depth is one. Total nodes/decoded string bytes count every visit
while normalizing each `let` and rule, including each substituted occurrence,
rather than deduplicating equal identities. Limits are checked during plan
validation and before the engine or journal exists; exceeding them is
`Rejected`, never truncated normalization or partial candidate enumeration.

`RunContextV1` is a required canonical envelope with, in order: format
version, `ProgramIdV1` (the program revision), initial-world identity, exact
policy identity, exact profile marker, and `RunLimitsV1`. The `ContextId`
passed to commit/audit is derived from this complete envelope;
`ContextId::root()` is not a valid public L3-run substitute. Including profile
and limits makes a budgeted run unambiguously different from an
otherwise-identical run with a different resource contract.

### 3.4 Policy and calendar

`L3PolicyV1` is a canonical immutable envelope containing the plan identity,
the fixed profile marker, and the one compiler-owned regime identity. It
compiles to `AdmRegimeAllowlist` containing precisely
`RegimeId::named("brix.l3.rule-agenda@1")`'s interned handle.
There is no surface policy language in v1, and `AdmAll` is not an acceptable
public default.

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

## 4. Deterministic commit loop and budgets

For a validated plan and `RunLimitsV1`, L3 executes:

1. construct a fresh `Interner`, intern plan/world/policy/regime/witness
   identities, and construct `ExecConfig`;
2. initialize `IncrementalEngine` with the compiler-owned dual regime, apply
   the initial world delta, filter candidate additions through the exact `Adm`
   policy, and materialize only admitted candidates into a keyed `Frontier`;
   this initialization is not a committed step;
3. before every selection, compare the incremental admissible view (and its
   keyed least candidate) with the naive `Regime::candidates` relation filtered
   through the same `Adm` policy for the presented world. A mismatch is
   `RuntimeUnknown`, never a commit;
4. if the pending-rule set is empty, terminate **PlanComplete** without a
   speculative probe;
5. if pending rules remain but the maintained frontier is empty, terminate
   **FrontierStalled**. This says nothing about semantic quiescence. The
   built-in v1 policy/regime cannot produce this state for a validated plan;
   the status keeps the adapter total and is exercised with an injected
   denying policy;
6. if a candidate exists but the committed-step count equals `max_commits`,
   terminate **CommitBudgetExhausted** without selecting, decomposing, or
   appending that candidate;
7. otherwise peek at the least key without mutation and obtain its pure
   `prospective_successor` from `soc-core`. Derive `step_world_delta` and apply
   it to the private incremental engine. On staged frontier/reverse-index
   copies, remove the old candidate and insert additions with phase `n + 1`.
   Compare the resulting admitted view/key with the naive prospective-state
   relation. If those checks pass, call `try_commit_selected`; require its
   successor to equal the prospect, then append/publish its returned step and
   swap in the staged frontier as the new public state. Any failed update,
   comparison, or commit validation aborts as `RuntimeUnknown` with only the
   earlier `ExecConfig`/journal prefix exposed; the mutated private engine and
   staged maps are discarded.

`max_commits` is an L3-v1 safety bound, not a proof of nontermination. It
MUST be included in the run result but does not alter the plan identity.
Zero is valid: an empty agenda is still `PlanComplete`, while a nonempty one is
immediately `CommitBudgetExhausted`.

Current `commit_tick` has no candidate-enumeration, wall-clock, or audit-work
budget. Its `Regime::candidates` returns a materialized `Vec<Candidate>`, so a
regime cannot signal a partial result or enforce a candidate-work limit while
enumerating. L3-v1 MUST state that limitation in its API and MUST NOT claim
such a budget was enforced. Adding pre-emptive enumeration/work limits requires
a fallible bounded regime/`commit_tick` API and is a later compatible extension.
Kernel budgets apply only if a caller separately requests proof elaboration;
L3 does not invoke the proof kernel automatically.

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

The public result MUST use an explicit tagged status, at minimum:

| Status | Meaning | Grade claim |
|---|---|---|
| `PlanComplete` | The profile's canonical pending-rule set is empty. | Bounded agenda completion only; no theorem, semantic quiescence, fixpoint, or #61 certificate. Logged steps are `Derived`. |
| `FrontierStalled` | Pending rules remain but the maintained admissible frontier is empty. | `Unknown` for completion/quiescence; no invented negative fact. |
| `CommitBudgetExhausted` | A candidate exists but committing it would exceed `max_commits`. | `Unknown` for completion/quiescence; earlier logged steps remain `Derived`. |
| `Rejected` | Parse, profile, closure, static-value, resource-limit, or plan-validation failure. | No settlement fact. |
| `RuntimeUnknown` | A fallible runtime/integrity failure prevented a trustworthy result. | `Unknown`; never an invented commit. |

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

- program, `RunContextV1`, initial world, and policy identities;
- the exact `RunLimitsV1`, final resolved world/policy state, and total
  stop reason;
- ordered committed step digests and `Journal::chain_digest()`;
- no raw-handle `ExecConfig.history`, cost records, or audit reports.

`Rejected` is a pre-run compiler outcome and therefore has no fabricated
`ProgramIdV1`, context, journal, or `SettlementRunV1`. Every post-plan status
is carried by `SettlementRunV1`; `RuntimeUnknown` additionally carries a
stable versioned reason code, while human diagnostic text remains outside the
semantic identity.

Its replay identity is valid only when a fresh interpreter reconstructs the
same plan, rebuilds the same journal in order, and obtains byte-identical
`Journal::replay_chain(steps)` and final chain digest. A matching digest alone
does not certify rule semantics; audit remains the tight boundary.

The operational result MAY carry the final `ExecConfig`, per-step
`CostRecord`s, and an optional audit report as non-semantic diagnostics. Those
fields are excluded from `SettlementRunV1`'s semantic identity: current
`ExecConfig.history` folds raw interner handles and is therefore not stable
across interner allocation orders; costs are observational; audits are separate
judgements with their own evidence identities. A mandatory cross-interner-order
fixture MUST rebuild the same plan with distinct intern insertion orders and
obtain identical `ProgramIdV1`, `RunContextV1`, step identities, journal chain,
and `SettlementRunV1` identity.

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
   fallible `RuntimeUnknown`/rejection, not a panic exposed as a successful
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
6. **Stop reasons are not semantic quiescence.** `PlanComplete`,
   `FrontierStalled`, and `CommitBudgetExhausted` do not establish a #61
   certificate. Stalling/exhaustion are `Unknown` completion states, never
   `Refuted`, `Audited`, or a quiescence certificate.
7. **No source-text identity.** Comments, formatting, and path names cannot
   affect canonical plan or replay identity. Explicit version fields protect
   encoder evolution.

## 7. Explicit boundary with #61

`PlanComplete` means only that the finite v1 agenda is empty.
`FrontierStalled` means only that its maintained immediate frontier has no
admissible candidate. Neither MUST be printed as “quiescent,” “settled,” or
“at a fixpoint,” and neither is the total settlement interface required by #61.

In particular, L3 v1 has none of:

- administrative versus realizing (`τ` versus visible) trace labels;
- divergence-sensitive saturation or weak transitions;
- an explicit, independently checkable quiescence certificate;
- saturation/refinement/bisimulation checks or behavioral counterexamples;
- correction/retraction or cross-revision invalidation semantics.

Those remain #61 (and the relevant #59/#178 integration work). A future
saturated profile MUST use a new versioned result status/artifact; it MUST NOT
reinterpret v1 `PlanComplete` or `FrontierStalled` retrospectively.

Accordingly, #178's older “drive `commit_tick` to a fixpoint” wording is
planning shorthand superseded by this pin: the public L3-v1 path is the
incremental adapter above, and it reports bounded agenda completion rather
than a fixpoint. #61 owns the stronger settlement interface.

## 8. Non-goals

L3 v1 does not:

- define general evaluation, effects, recursion, or a value store for Brix;
- execute surface `regime`/`gen` bodies or provide a user-authored policy
  language;
- make every `let` a settlement fact or change L2's type-checking grades;
- replace the naive reference oracle; it instead requires the incremental
  engine's O(Δ) gate and step-by-step differential comparison;
- auto-audit, auto-prove, or establish theoremhood from a completed run;
- provide durable certificates, distributed replication, scheduling
  parallelism, or a saturation witness.

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
fields MUST reject. Rule/value limit failures MUST occur before engine or
journal construction.

### Stage B — one-shot dual compiler regime

Implement `Regime` + `IncrementalRegime`, the plan's footprint, fallible
decomposition, and exact `Decomposition` construction. Fixture: two
zero-argument rules in reverse lexical name order. The module-order rule MUST
commit first, then the other; each decomposed endpoint MUST equal the pre/post
canonical L3 worlds.

### Stage C — incremental calendar adapter, bounded driver, and replay

Implement incremental candidate-delta → keyed-frontier maintenance, total stop
results, and the semantic/diagnostic result split. Fixtures:

1. an empty-rule module returns `PlanComplete` with an empty journal;
2. two rules with sufficient `max_commits` return `PlanComplete`, two
   `Derived` observations, a two-step journal, and reproducible replay chain;
3. the same two rules with `max_commits = 1` return `CommitBudgetExhausted`, one
   committed record, and no false quiescence;
4. the same plan with `max_commits = 0` returns `CommitBudgetExhausted` with an
   empty journal, while an empty plan at zero returns `PlanComplete`;
5. an adapter-level deliberately denied pending rule returns
   `FrontierStalled`, not a quiescence/fixpoint claim;
6. unsupported/parameterized/non-static rules reject before any journal is
   created;
7. deliberately colliding calendar keys or malformed decompositions fail
   closed rather than selecting/minting a result.
8. a candidate-witness/generator mismatch, wrong transition-table candidate,
   or wrong decomposition endpoint fails before `Derived` publication.
9. after every step, incremental candidates and selected key equal the naive
   relation; inert configuration/regime ballast leaves the measured per-step
   cost within the `O(|Delta| × fanout)` gate.
10. increasing trailing agenda ballast leaves one head commit's candidate
    delta, core work, and L3-local apply probe count unchanged: one removal,
    at most one addition, and one lookup per touched world handle.
11. two fresh interners initialized in different insertion orders produce the
   same semantic run/replay identities while their raw engine histories may
   differ.

For a post-plan outcome, the first CLI surface added after these fixtures MUST
print the program, context, semantic run, journal-chain, ordered `Derived`
step identities, and one exact status name from §5. `Rejected` instead prints
deterministic plan diagnostics and no invented semantic identifiers. The CLI
MUST use a nonzero exit status for `Rejected`, `FrontierStalled`,
`CommitBudgetExhausted`, or `RuntimeUnknown`, and MUST NOT print “quiescent,”
“fixpoint,” `Audited`, or `Proven` for ordinary settlement completion.

### Stage D — audit boundary

Use the plan-specific registry/semantics with `audit_journal`. Fixture: the
unchanged journal yields distinct linked `Derived` and `Audited` judgements;
tampering with a rule/world/fact/decomposition yields `AuditResult::Unknown`.

No CLI `brix run` command is added before Stages A–C have the above fixtures.
No `@Audited` display or proof command is added before Stage D's authority
fixtures pass.

## 10. Compatibility and evolution

- New L3 artifact fields, statuses, and encoders are append-only and versioned.
  Existing v1 identities never change in place.
- The existing `soc-core` `CommittedStep` ABI remains authoritative; the L3
  result envelope wraps it and does not copy/re-encode it as an alternate log.
- Adding source-level transition guards, parameters, payload-bearing
  constructors, general regimes, effects, candidate-work limits, or saturation
  requires a new ADR slice and a new execution-profile marker. It cannot
  broaden v1 by accepting formerly rejected forms under the same
  `brix.l3.rule-agenda@1` identity.
- A future implementation may replace the compiler-owned one-shot regime with
  a richer lowering only after differential fixtures establish equivalent
  behavior for the v1 fragment and audit replay remains byte-identical.

## 11. Open blockers and decisions still required

1. `brix-lower` needs a public/module-level checked lowering path for rules;
   today its public `check_module` processes `let` declarations only.
2. The exact canonical schemas and frozen vectors for the new L3 artifacts
   need a separate encoder review; this ADR fixes their required inputs and
   versioning, not their byte tags.
3. `Interner::resolve` and `commit_tick` currently expose panic paths for bad
   handles, malformed empty decompositions, and frontier conflicts. The public
   L3 boundary needs `try_commit_selected`, `prospective_successor`, the
   fallible regime interface, and the named interner/frontier seams in §2
   before accepting source-derived plans.
4. The current audit API is unbudgeted; a resource-bounded audit operation is
   required before L3 can make any audit-latency guarantee.
5. #61 remains the blocker for claims about saturation, certified quiescence,
   observable behavior, or a true general “run to fixpoint.”
