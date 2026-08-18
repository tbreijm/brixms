# ADR-0029 — L3 Witness-Frontier Profile

Status: **Proposed** (2026-08-18).

`brix.l3.witness-frontier@1` is a new executable profile. It neither widens
ADR-0012's serial rule-agenda v1 profile nor changes SOC core semantics. It
builds on ADR-0028's separation of witness possibilities from non-ontological
discovery providers.

The accepted source is a finite set of ordinary zero-argument `rule`
declarations, with the same closed `config`/`let` value vocabulary that
ADR-0012 validates. A source rule already means propose -> commit; this
profile adds no `regime`, `gen`, candidate, or witness-declaration syntax.

Every rule witness begins at one initial configuration. All are presented in
one keyed frontier under `AdmAll`; least `(phase, source ordinal, witness
digest)` commits exactly one witness. The selected successor records the stable
source `RuleId`, not endpoint-bound `GeneratorId`. The latter is derived only
after that destination is fixed, and then supplies the witness, decomposition,
journal record, and audit relation.

More precisely, each ordinary source rule lowers to one candidate witness
realization `x —w→ y`. Their coexistence in the frontier expresses concurrent
possibility; core settlement selects one candidate, making its successor and
appended history the resulting consequence. This changes neither the meaning
of a witness nor the core's single-commit rule.

The profile's plan, worlds, policy, run context, and generators are canonical
and versioned. `PlanLimitsV1::max_selected_rules` bounds its finite rule set.
The runtime calls `soc_core::try_commit_tick`, so the core remains the sole
publisher of `Derived`; audit is an explicit `audit_journal` operation. It
makes no quiescence or settlement-why claim.

SOC's ontology here remains configurations and witnesses: a candidate is the
witness realization `(w, y)` at execution configuration `(x, p, h)`. The
private `WitnessPresenter` merely implements the core's `WitnessProvider` and
`SettlementWitnessProvider` discovery/decomposition interfaces. It creates,
owns, and defines no semantic possibilities, and it has no identity in
`Candidate`, source syntax, or the public profile API.

Candidate locks, payload input, schemas, derived expressions, source policy,
music semantics, and settlement `why`/`whynot` remain out of scope.
