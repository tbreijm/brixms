# ADR-0022 — Source-Re-Derived L3 Settlement Manifests

Status: **Accepted** (2026-08-16; Proposed 2026-08-09) (rules on [ADR-0020](./ADR-0020_Oracle_Bound_Audit_Receipts.md) §5 residuals 1–3 and [ADR-0021](./ADR-0021_Settlement_Semantics_Attestations.md) §6 residual 7; follows [ADR-0012](./ADR-0012_L3_Executable_Settlement.md), [ADR-0013](./ADR-0013_Canonical_Certificate_Envelope.md), [ADR-0016](./ADR-0016_Authority_Publication_Fence.md), [ADR-0019](./ADR-0019_Verification_Tags_Are_Earned.md), and ADR-0020).

Date: 2026-08-09.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4 outcome lattice, §4.1 verifier authority, §5 evidence-bearing upgrades, §6 generator registries), [ADR-0012: L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (§3 canonical plan identity and plan limits, §5 audit boundary), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§6 fail-closed decoding, §7 additive versioning, §8 independent vectors), [ADR-0016: Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md), [ADR-0019: Verification Tags Are Earned](./ADR-0019_Verification_Tags_Are_Earned.md), [ADR-0020: Oracle-Bound Settlement Audit Receipts](./ADR-0020_Oracle_Bound_Audit_Receipts.md), and [ADR-0021: Scoped Settlement-Semantics Authorization Attestations](./ADR-0021_Settlement_Semantics_Attestations.md).

This ADR adds no canonical artifact. It changes no existing id, ordinal, evidence kind, outcome, authority route, or vector. It selects the existing `.brix` source frontend as the L3 reconstruction path by which an offline verifier derives the expected `GeneratorRegistry` and `GeneratorSemanticsV1`.

---

## 1. The finding

ADR-0020 makes the settlement semantics executed by an audit identifiable. Its receipt binds the exact `GeneratorRegistryId` and `GeneratorSemanticsIdV1`.

That does not, by itself, tell a verifier which semantics id is expected. For L3, the expected declaration is already a deterministic consequence of the plan:

```text
L3PlanV1
  → ProgramIdV1
  → L3TransitionTable
  → GeneratorRegistry
  → GeneratorSemanticsV1
```

The live source pipeline constructing that plan is:

```text
UTF-8 .brix source
  → brix_syntax::parse
  → brix_lower::lower_l3_plan
  → L3PlanV1
```

The parser and L3 lowerer are deterministic for fixed source, profile, limits, toolchain, and code revision:

- they read no clock, environment variable, filesystem path, network input, or random input;
- semantic collections use `BTreeMap` and `BTreeSet`;
- plan items preserve explicit module order;
- values are normalized before canonical identity;
- the execution profile and `PlanLimitsV1` are explicit inputs; and
- the frozen `ProgramIdV1` vectors and determinism jobs already detect canonical drift.

This is sufficient for sound re-derivation. It is not a promise that every future parser or lowerer release will accept an old source file or lower it identically.

That stronger promise is unnecessary for acceptance safety because the verifier must compare the re-derived `ProgramIdV1` with an independently expected `ProgramIdV1`. If frontend evolution changes the lowered plan, the comparison fails. That is the intended detection of semantic drift, not a defect to conceal.

A repository inspection also corrects two implementation assumptions:

1. `brix-canon` already exposes a public `CanonReader`; strict kernel and saturation-certificate decoders use it. A plan decoder would extend the canonical-reader attack surface rather than create the system’s first reader.
2. `build_l3_transition_table` now computes `program_id(plan)` internally and keeps the former `(ProgramIdV1, plan)` helper private, as required by ADR-0020 D8.

There remains no durable `L3PlanV1` decoder or plan transport. `l3_canon.rs` defines a plan identity preimage, not a reversible plan serialization. In particular, let and rule items place `L3ValueId` digests in the program preimage rather than the full values required to reconstruct an arbitrary typed plan.

---

## 2. Alternatives considered

### (a) Ship source and re-lower — selected

The source is an untrusted reconstruction witness. It need not have a canonical byte representation and is not assigned another id.

A verifier parses and lowers it locally, recomputes `ProgramIdV1`, requires equality with the independently expected program, and only then derives the expected registry and manifest.

This adds no new canonical format, decoder grammar, artifact id, ordinal, vector, cryptographic dependency, or Cargo production edge.

It does make the existing source frontend part of the strict L3 receipt-verification TCB. That is a real boundary consequence. It also requires resource hardening before the frontend is exposed as an adversarial offline-verification entry point.

### (b) Durable versioned `L3PlanV1` transport — deferred

A durable normalized-plan transport would decouple old plans from source-language evolution. That is its real advantage.

It currently buys no necessary acceptance property that source re-lowering plus an expected `ProgramIdV1` does not already provide. It would require:

- a new artifact marker and version;
- a decision about whether to transport complete `L3ValueV1` values or only the identity-relevant projection;
- a plan-specific decoder;
- strict ordinal, framing, truncation, trailing-byte, and noncanonical-encoding rejection;
- allocation, length, and recursion bounds;
- both round-trip laws;
- fuzzing and property tests; and
- an additive two-consumer vector file.

A general `CanonReader` already exists, but it does not remove these plan-specific obligations. Adding generic list, enum, map, identifier, and allocation facilities to the base reader merely to transport L3 plans would widen a shared TCB for speculative reuse.

If durable normalized-plan exchange later becomes an actual interoperability requirement, it requires its own ADR. It is not introduced pre-emptively here.

### (c) Plan-bound transition-table transport — rejected

A table carrying only:

```text
ProgramIdV1 + generators + endpoint rows
```

does not prove that those rows follow from the named program. It repeats the assertion whose authority is in question.

To avoid that relocation of trust, the artifact would have to carry enough plan-bound material to:

1. recompute `ProgramIdV1`;
2. reconstruct the rule and value identities;
3. rebuild every world, generator, and transition; and
4. compare the rebuilt table with the transported table.

At that point it is a normalized-plan projection with substantially the same decoder, canonicality, resource-bound, and vector obligations as option (b). Source re-lowering provides that reconstruction today with fewer formats and fewer identities.

---

## 3. Decision

### D1 — The L3 source is the reconstruction witness

The strict source-based L3 verifier receives the `.brix` source as untrusted UTF-8 bytes and runs exactly:

```text
UTF-8 validation
  → brix_syntax::parse
  → lower_l3_plan(
        module,
        L3_PROFILE_MARKER_V1,
        finite verification PlanLimitsV1
    )
```

The source is not itself canonical. Whitespace, comments, equivalent integer spelling, or record-literal field order may produce different source bytes and the same `L3PlanV1` and `ProgramIdV1`. That is intentional.

No `SourceId`, source envelope, source ordinal, or source vector is introduced.

### D2 — Target selection is an independently expected `ProgramIdV1`

The verifier must receive an `expected_program: ProgramIdV1` independently of the source and receipt being checked.

After lowering, it computes:

```text
derived_program = program_id(plan)
```

and requires:

```text
derived_program == expected_program
```

Failure is a typed `ProgramMismatch` and no receipt is accepted.

`SettlementAuditReceiptV1` does not directly name a `ProgramIdV1`; it names the derived registry and semantics ids. This ADR does not alter the frozen receipt to add one.

An independently validated `SettlementRunV1` may supply the expected program because it canonically contains `ProgramIdV1`. Merely reading a program id from the same otherwise unauthenticated transport bundle does not establish that it is the program the verifier intended.

The source channel need not be trusted for integrity. A substituted source either re-derives the independently expected program or fails. Its availability remains an operational requirement.

### D3 — The expected audit environment is derived, never supplied by the receipt

Only after the program comparison succeeds does the verifier:

1. construct a fresh `Interner`;
2. call `build_l3_transition_table(&mut interner, &plan)`;
3. derive `l3_generator_registry(&table)`;
4. derive `l3_generator_semantics(&table)`; and
5. require exact registry/semantics generator-set agreement.

The strict source entry point accepts no caller-supplied expected registry or semantics declaration.

Transported registry or manifest objects may be retained for diagnostics, but they are not the expectation and are not executed merely because the receipt names their ids.

The transition-table constructor continues to compute `program_id(plan)` internally. No public `(ProgramIdV1, plan)` seam is reintroduced.

### D4 — Receipt acceptance runs the existing ADR-0020 checker

Conceptually, the complete strict operation is:

```text
check_l3_audit_receipt_from_source_v1(
    source_bytes,
    expected_program,
    verification_limits,
    receipt,
    committed_step,
    context
)
```

It runs:

1. reject source bytes exceeding the configured bound;
2. reject invalid UTF-8;
3. parse under bounded syntax limits;
4. lower under the fixed L3 v1 profile and finite plan limits;
5. recompute `ProgramIdV1`;
6. require equality with `expected_program`;
7. build the plan-bound transition table;
8. derive the exact registry and semantics declaration;
9. require their exact key-set agreement;
10. invoke `check_audit_receipt_v1` with those derived values;
11. let that checker compare the receipt’s registry and semantics ids, recheck its contextual fields, rerun settlement replay, reproduce the verified decomposition, and reproduce the receipt id; and
12. return success only if every stage succeeds.

A byte-level receipt decoder, where used, must reject malformed framing, non-minimal integers, unknown marker/version/profile, wrong digest lengths, truncation, and trailing bytes before this sequence accepts anything.

The successful result is the existing validated `SettlementAuditReceiptIdV1` and replay result. This entry point adds no judgement, evidence form, grade, or authority route.

### D5 — Frontend drift is detected, not normalized away

For a previously authorized expected program:

- if a new frontend produces the same normalized plan, verification continues;
- if it produces a different `ProgramIdV1`, verification fails with `ProgramMismatch`;
- if it no longer parses or lowers the source, verification fails with a typed source error; and
- if local resource policy refuses the source, verification fails with a typed resource error.

None of these conditions permits fallback to the receipt’s own manifest id.

A deliberate semantics-affecting revision to parsing, lowering, transition construction, or the L3 execution profile must mint an appropriate new profile or canonical version. It must not silently give an existing `ProgramIdV1` a new meaning.

The frozen `ProgramIdV1`, world, generator, registry, and semantics vectors remain the regression fence against accidental drift.

### D6 — The source frontend becomes part of this verifier’s TCB and must be bounded

Option (a) adds no new parser implementation, but it does elevate the existing parser and lowerer into the strict offline-verification closure:

```text
brix-syntax
  → brix-lower
  → soc-core
  → brix-semantic
  → brix-canon
```

The production dependency graph does not change. The logical trusted boundary does.

`PlanLimitsV1` alone is not a complete hostile-input bound today. Parsing allocates before lowering, and some normalized-value construction or let substitution can allocate before the existing post-construction accounting rejects an oversized value.

Before exposing the strict entry point, implementation must therefore enforce:

- a source-byte limit before UTF-8 conversion or tokenization;
- bounded token count;
- bounded syntax nesting/recursive-descent depth;
- checked collection lengths;
- pre-allocation or incremental `PlanLimitsV1` accounting during value normalization and substitution; and
- checked arithmetic for every count and allocation calculation.

Verification limits are local resource policy, not a canonical artifact. A verifier may refuse a valid large program. That refusal is `Unknown` or a typed resource error, never acceptance under weaker limits.

No panic, stack overflow, unbounded allocation, or permissive retry is an acceptable malformed-input result.

### D7 — No existing canonical identity or vector moves

The following remain byte-identical:

- `ProgramIdV1`;
- `DecompositionId`;
- `TreeDerivationId`;
- `GeneratorRegistryId`;
- `GeneratorSemanticsIdV1`;
- `SettlementAuditReceiptIdV1`;
- `JudgementId`;
- every existing ordinal;
- the ADR-0016 `ROUTES` table; and
- every existing file under `vectors/`.

This ADR introduces no canonical artifact, so it introduces no vector file. The two-consumer discipline is therefore not triggered by ADR-0022.

The existing `vectors/l3_plan_v1.json`, `vectors/generator_semantics_v1.json`, and `vectors/settlement_audit_receipt_v1.json` remain unchanged and serve as regression inputs.

There is no production dependency addition. In particular:

```text
brix-semantic → brix-canon only
soc-core      → brix-canon + brix-semantic
```

remains unchanged.

### D8 — Failures move down and never hide behind a weaker pass

Every failure returns a typed error or maps to `Unknown`.

This includes:

- invalid UTF-8;
- source, token, nesting, plan, value, or allocation limit exhaustion;
- parse failure;
- L3-fragment rejection;
- profile mismatch;
- `ProgramIdV1` mismatch;
- registry/semantics disagreement;
- unexpected receipt registry;
- unexpected receipt semantics;
- receipt field mismatch;
- malformed receipt transport;
- failed replay; and
- refused ADR-0016 publication.

No failure produces `Audited`, `Proven`, or `Refuted`. No strict failure falls back to generic receipt checking under a manifest obtained from the receipt.

An already existing `Audited` judgement is not mutated or downgraded. The strict consumer simply refuses to accept its receipt under the source-derived L3 profile.

### D9 — ADR-0020 residuals 2 and 3 close for this L3 path

ADR-0020 residual 3 is closed for source-available L3 verification:

> Given the source and an independently expected `ProgramIdV1`, the verifier can derive the expected manifest locally. It does not need a separately transported canonical plan, manifest allow-list, or signing key.

This is a scoped full closure of the expected-manifest construction problem. It is not a claim that every input needed for an end-to-end offline audit already has a finished transport decoder.

ADR-0020 residual 2 also closes for the strict L3 entry point:

- the verifier does not accept caller-declared rows as its expectation;
- it derives the rows from the locally accepted source and plan-bound table; and
- a fabricated manifest, even if internally consistent and receipted under its own id, disagrees with the locally derived id and is rejected.

This is stronger than ADR-0021’s signature path because it checks derivation under the local L3 implementation rather than trusting an authorized signer not to lie.

The closure does not extend to:

- generic `GeneratorSemanticsV1` callers;
- another settlement regime without an equivalent derivation rule;
- a deployment that withholds the source; or
- target selection when the verifier has no independently expected program id.

### D10 — ADR-0021 remains Proposed as the plan-unavailable alternative

ADR-0021 is not superseded globally.

It remains **Proposed and unimplemented** for a future deployment in which:

- the source or normalized plan is confidential;
- plan distribution is prohibited or impractical;
- verifiers cannot retain a compatible frontend; or
- deployment policy intentionally chooses a scoped signer as its authorization root.

For an ordinary L3 deployment able to distribute source, ADR-0022 is the preferred and stronger path. Implementing ADR-0021 later creates a complementary acceptance profile, not a replacement or silent fallback.

A deployment must explicitly choose which profile it requires:

```text
source available       → local re-derivation under ADR-0022
source unavailable     → possible future attestation under ADR-0021
```

Success under one profile does not imply success under the other.

---

## 4. Consequences

The expected semantics id no longer needs to be distributed independently for source-available L3 programs.

A `.brix` file becomes a witness for the already-frozen `ProgramIdV1`, not another identity-bearing artifact. Equivalent source spellings remain equivalent.

Frontend changes cannot silently reinterpret an old expected program. They either preserve its `ProgramIdV1` or cause refusal.

The verifier TCB includes more code than the generic ADR-0020 receipt checker: the L3 lexer, parser, lowerer, canonical plan identity, transition-table construction, and manifest derivation. It includes no signature implementation, key lifecycle, new serialization format, or new dependency.

Resource-limit refusal may reduce availability for large valid sources. It cannot increase epistemic grade.

No grade, outcome, existing evidence, or canonical id changes.

SOC-LAW-03, SOC-LAW-04, and SOC-LAW-05 retain their current status. Their traceability gains the strict source-derived negative gates, but ADR-0022 adds no publication route or authority.

---

## 5. Residuals — what this does not fix

1. **Expected-program selection remains out of band.** The verifier must independently know which `ProgramIdV1` it intends to verify. Source plus a matching receipt cannot select its own target.

2. **Source availability remains operational.** The source channel need not provide integrity when the expected program is pinned, but it must provide the bytes. An unavailable source yields no acceptance.

3. **Compatible frontend retention remains an availability obligation.** A future frontend may reject old source. The expected id detects reinterpretation but cannot reconstruct a plan when no compatible implementation remains.

4. **The frontend is trusted code.** A bug in local parsing, lowering, transition construction, or manifest derivation may cause refusal or erroneous acceptance. This ADR adds tests and versioning rules; it does not make the deriver self-proving.

5. **Complete transport of every audit input remains separate.** This ADR specifies how the expected L3 registry and manifest are reconstructed. It does not define a general bundle or decoder for `CommittedStep`, context envelopes, journals, or `SettlementRunV1`. ADR-0020’s strict receipt-byte decoder must also be completed where only typed receipt checking exists.

6. **Confidential plans remain unsupported by this path.** A verifier cannot re-derive a manifest from source it is not allowed to receive. ADR-0021 remains the proposed alternative for that deployment assumption.

7. **Generic caller declarations remain non-authoritative.** The closure applies only to the strict L3 source entry point. It does not make arbitrary `GeneratorSemanticsV1` values self-authorizing.

8. **Journal inclusion remains unproved.** The receipt remains step-scoped and does not bind an ordinal, prefix digest, or complete journal history.

9. **The existing `Audited` judgement remains weaker than retained receipt provenance.** Discarding the receipt discards the source-derived manifest-selection result.

10. **No cross-implementation source conformance standard is created.** A second implementation must reproduce the same plan and manifest identities, but this ADR does not standardize a portable source conformance test suite beyond the existing vectors and fixtures.

The remaining open question is precise:

> If long-lived verifiers cannot retain compatible L3 frontends or cannot receive source, should the project standardize a durable normalized-plan transport, or accept ADR-0021’s scoped signer as the operational authorization root?

This ADR does not answer that future deployment question.

---

## 6. Non-goals

- No canonical source artifact or `SourceId`.
- No durable `L3PlanV1` encoding or decoder.
- No transition-table transport artifact.
- No signature, signing key, trust policy, or revocation mechanism.
- No change to `ProgramIdV1` or its preimage.
- No change to `GeneratorSemanticsV1` or its id.
- No change to `SettlementAuditReceiptV1` or its id.
- No edit or re-blessing of an existing vector.
- No new outcome, grade, evidence kind, authority, or route.
- No `Refuted` path.
- No fallback from strict source verification to caller-declared semantics.
- No journal-wide receipt.
- No guarantee that every future frontend accepts every old source file.
- No claim that source possession identifies the program the user intended without an independently selected `ProgramIdV1`.
- No replacement of ADR-0021 for deployments that intentionally withhold the plan.

