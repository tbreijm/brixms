# ADR-0020 — Oracle-Bound Settlement Audit Receipts

Status: **Accepted** (2026-08-16; Proposed 2026-08-09) (rules on ADR-0019 §6 residuals 1–3; follows [ADR-0013](./ADR-0013_Canonical_Certificate_Envelope.md), [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md), and [ADR-0019](./ADR-0019_Verification_Tags_Are_Earned.md)).

Date: 2026-08-09.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4.1 verifier authority, §5 evidence-bearing upgrades, §6 generator registries and decompositions), [ADR-0012: L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (§3 canonical plan identity, §5 audit boundary), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§7 versioned additive artifacts and independent vectors), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-PRIM⟩ kernel-owned primitive relations), [ADR-0016: Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md), and [ADR-0019: Verification Tags Are Earned](./ADR-0019_Verification_Tags_Are_Earned.md) (§6 residuals ruled here).

This ADR changes no outcome, authority row, proof rule, grade, existing evidence ordinal, `Decomposition` encoding, `TreeDerivation` encoding, or existing artifact id. It introduces two additive v1 canonical artifacts: declared settlement semantics and a settlement audit receipt.

---

## 1. The finding

ADR-0019 closed unchecked verification-tag minting. `ReplayVerified` can now be reached only by executing registry membership and `GeneratorSemantics::realizes` over every link.

That leaves a real distinction:

> Executing a caller-supplied predicate proves that the predicate ran. It does not identify or authenticate which predicate ran.

A caller can still implement `GeneratorSemantics` as `true` for every input. The in-tree test `an_always_true_semantics_still_passes_a_fabricated_chain` pins that limit.

The practical production surface is narrower than the trait suggests. A workspace-wide call-site inspection finds exactly two non-test implementations:

1. `L3GeneratorSemantics`, whose answer is a lookup in an immutable `L3TransitionTable` derived from an L3 plan; and
2. `LiteralEqualitySemantics`, the stateless diagonal relation for one fixed generator.

Both are already representable as declared data:

```text
L3:       generator ↦ one exact (source, destination) row
Literal:  generator ↦ the diagonal relation {(x, x)}
```

There is therefore no production need for an open executable trait whose implementation can perform arbitrary behavior.

Two further facts constrain the receipt.

First, `audit_step` itself does **not** hold a `Journal`. Its actual inputs are a single `CommittedStep`, `ContextId`, registry, and semantics. `audit_journal` holds the journal but currently only iterates and calls `audit_step`. A step receipt can bind a canonical committed-step digest, including its observation, endpoints, recorded decomposition, witness, and key. It cannot honestly attest that the step occurred at a particular journal index or under a particular prefix-chain digest without widening the API and the artifact.

Second, the current L3 table constructor accepts both `ProgramIdV1` and `&L3PlanV1`, while its own documentation says the caller must ensure they agree and admits that it does not check this. An oracle identity derived from such a table would inherit that unchecked pairing. L3 oracle authentication therefore requires eliminating that inconsistent-construction route.

---

## 2. What authentication can establish

A content-addressed semantics identity can establish:

- which complete relation declaration was executed;
- that two receipts naming different declarations are different receipts;
- that the registry and semantics declaration used together are exactly the ones expected by a consumer; and
- that a receipt cannot be replayed against another context, committed step, verified decomposition, registry, or semantics declaration.

It cannot establish, by itself, that a consumer ought to trust a particular semantics identity.

That final authorization requires an independent anchor:

- for L3, the expected declaration is re-derived from the validated canonical plan, never selected from the receipt;
- for literal equality, the expected declaration is the fixed production constant; and
- for a future settlement profile, some independently reviewed construction must define how its expected semantics id is obtained.

A checker that reads the semantics id from the receipt and then accepts that same id as its expectation has authenticated nothing.

---

## 3. Decision

### D1 — `Audited` and the six-outcome lattice remain unchanged

The receipt is additive. It does not redefine `Outcome::Audited`, replace `Evidence::SettlementReplay`, add an outcome, or modify ADR-0002 §4.1’s authority table.

A successful audit continues to publish the same `Audited` judgement supported by the same `ReplayVerified` `Decomposition`. Its `JudgementId` therefore remains unchanged.

No `Evidence` ordinal is appended in v1. Adding an unused `Evidence::SettlementAuditReceipt` would be decorative; using it would change every affected `JudgementId` and create a second authority route. Neither is needed to authenticate the audit inputs.

The receipt is a stronger, separately checkable artifact adjacent to the existing judgement:

```text
Audited judgement  — existing claim and identity, unchanged
AuditReceiptV1     — identifies the exact inputs and checker profile used
```

A consumer requiring oracle-bound provenance must require and validate the receipt. Existing consumers that require only the ADR-0019 `ReplayVerified` claim continue to behave as before.

### D2 — Settlement semantics becomes canonical declared data

The open `GeneratorSemantics` trait is removed from the production and public verification boundary. It is replaced by a v1 canonical declaration owned by `brix-semantic`:

```text
GeneratorSemanticsV1 {
    relations: Map<GeneratorId, SettlementRelationV1>
}

SettlementRelationV1 ::=
    ExactRows(Set<(ConfigId, ConfigId)>)   // ordinal 0
  | Diagonal                              // ordinal 1
```

The exact Rust representation may use `BTreeMap` and `BTreeSet`, but the semantic requirements are:

- one relation declaration per generator;
- deterministic canonical ordering;
- `ExactRows` accepts exactly its declared rows;
- `Diagonal` accepts exactly `src == dst`;
- a missing generator fails closed;
- unknown relation variants fail closed; and
- the declaration’s generator-key set must equal the supplied `GeneratorRegistry` exactly at the receipted audit boundary.

The exact-key-set condition prevents a registry and semantics manifest from merely overlapping on the links exercised by one candidate chain. The receipt identifies the complete audit environment, not just its used subset.

`GeneratorSemanticsV1::realizes` is ordinary TCB code in `brix-semantic`; it is not dynamically replaceable. Test fixtures construct exact finite rows instead of implementing executable predicates.

### D3 — The semantics declaration has its own content identity

`GeneratorSemanticsIdV1` is the `Domain::Value` digest of this frozen preimage:

| # | Field | Encoding |
|---|---|---|
| 1 | Marker | `write_bytes(b"brix.semantic.generator-semantics")` |
| 2 | Format version | `write_uint(1)` |
| 3 | Relations | canonical map keyed by canonical `GeneratorId` bytes |

Each map value is:

- `ExactRows`: `write_enum(0, …)` containing a canonical set of row encodings; each row canonically writes `src` followed by `dst`;
- `Diagonal`: `write_enum(1, |_| {})`.

The marker, version, ordinals, map shape, row field order, and framing are frozen v1 ABI. A new relation form requires v2; it is not appended opportunistically to v1.

A new manifest instance with different rows is ordinary new data under v1. Reinterpreting an existing id is forbidden.

### D4 — This is a sibling of ADR-0015 ⟨D-PRIM⟩, not a second `PrimitiveRelationId`

This ADR deliberately selects the **sibling identity** option.

It does not reuse or redefine `PrimitiveRelationId`, and it does not prescribe its pending canonical encoding.

The types serve different authority and shape:

| ADR-0015 `PrimitiveRelationId` | ADR-0020 `GeneratorSemanticsIdV1` |
|---|---|
| Kernel-owned trusted axiom data | Settlement-audit input data |
| Judgment-scoped | Settlement `GeneratorId × ConfigId × ConfigId` only |
| Schema-checked `ObjectTerm` endpoints | Opaque canonical `ConfigId` endpoints |
| Finite exact rows only | Exact rows plus the closed-form diagonal |
| Can close a proof leaf and contribute to `Proven` | Can support only settlement replay and a receipt |
| New relation requires a kernel release | New L3 plan produces new rows without changing the checker |

Reusing `PrimitiveRelationId` would either erase its judgment and schema scope or violate ADR-0015’s prohibition on algorithmic/wildcard primitive rows. Treating the literal diagonal as an infinite finite-row kernel relation would be dishonest.

The two identity domains must remain visibly disjoint. There is no conversion between them. A future bridge may reuse a settlement relation as a source for proposing a kernel primitive, but only a separately reviewed kernel registry entry can authorize the latter.

### D5 — The v1 receipt binds exactly the re-derivable audit material

`SettlementAuditReceiptV1` binds:

1. the `ContextId`;
2. the canonical `Domain::Value` digest of the exact recorded `CommittedStep`;
3. the earned `ReplayVerified` `DecompositionId`;
4. the `GeneratorRegistryId`; and
5. the `GeneratorSemanticsIdV1`.

Its frozen preimage is:

| # | Field | Encoding |
|---|---|---|
| 1 | Envelope marker | `write_bytes(b"brix.soc.audit-receipt")` |
| 2 | Format version | `write_uint(1)` |
| 3 | Checker profile | `write_str("brix.soc.audit-factorization@1")` |
| 4 | Context | canonical `ContextId` |
| 5 | Committed step | `write_bytes(step.canon_digest(Domain::Value).as_bytes())` |
| 6 | Verified decomposition | canonical `DecompositionId` |
| 7 | Generator registry | canonical `GeneratorRegistryId` |
| 8 | Generator semantics | canonical `GeneratorSemanticsIdV1` |

`SettlementAuditReceiptIdV1` is the `Domain::Value` digest of that complete preimage.

The profile string identifies the one v1 checking algorithm. A separate `VerifierId` is not added: the existing type specifically identifies proof kernels, while v1 has one fixed settlement audit profile. Adding an audit-verifier field containing another fixed digest would add repetition without an independent choice to validate.

### D6 — The committed-step digest binds the observation and endpoint checks

A separate observation field is not included. `CommittedStep` already canonically contains, in frozen order:

```text
key, observation, recorded decomposition, src, dst, witness
```

Its digest therefore binds the exact observation and endpoint claims checked by `audit_step`, along with the rest of the committed record. Repeating the observation would add no independently re-derivable distinction.

The receipt’s context field is separate because `ContextId` is an argument to `audit_step` and is not part of `CommittedStep`.

The verified decomposition id is also separate because the committed step contains the recorded decomposition. The verified id is the actual stage-3 result used to publish `Audited`; it is not derivable merely by selecting another tag without rerunning the relation check.

### D7 — A receipt is checked by replay, not trusted as a record

`soc-core` owns both receipt issuance and validation because it already owns `CommittedStep`, the contextual audit stages, and the `Audited` publication.

The public validator has the conceptual contract:

```text
check_audit_receipt_v1(
    receipt_bytes,
    committed_step,
    context,
    expected_registry,
    expected_semantics
) -> re-derived audited result or typed failure
```

It must:

1. decode and validate the exact marker, version, profile, field framing, and absence of trailing bytes;
2. recompute the committed-step, registry, and semantics identities from the independently supplied typed values;
3. require exact registry/semantics generator-set agreement;
4. reconstruct and cross-check the recorded `Derived` judgement and observation;
5. check committed-step endpoint agreement;
6. rerun every decomposition link under the supplied declared semantics;
7. reconstruct the same verified decomposition;
8. pass through the existing ADR-0016 publication fence; and
9. compare every re-derived receipt field and id against the supplied receipt.

Unknown versions, profiles, relation variants, malformed fields, mismatched expected identities, failed links, failed publication, or trailing bytes all fail closed.

The validator does not accept only a semantics id. It requires the declaration whose behavior it executes, and separately compares that declaration’s id with the receipt and with the consumer’s expectation.

No validation failure constructs `Audited`, produces a receipt, or yields `Refuted`.

### D8 — The two production oracles become declarations

For L3:

- each table generator receives one `ExactRows` relation containing its single expected endpoint pair;
- the manifest is deterministic over the complete immutable transition table;
- `audit_l3_journal` and `audit_l3_run` use that manifest for audit and receipt production; and
- an independent L3 receipt check derives the expected manifest from the validated plan or from a transition table whose construction is itself plan-bound.

`build_l3_transition_table` must stop accepting an unchecked `ProgramIdV1`/plan pair. Its production constructor computes `program_id(plan)` internally. An internal helper may accept an already-computed id only if it is private and reached after the equality has been established.

This is a Rust API change, not a canonical or behavioral change. Honest existing calls produce byte-identical programs, worlds, generators, journals, and audit results.

For literal equality:

```text
literal-equality.refl@1 ↦ Diagonal
```

is the complete manifest. Its id is a fixed consequence of the generator name and v1 declaration encoding. `LiteralEqualitySemantics` ceases to be an executable trait implementation; it may survive as a zero-sized namespace or constructor for the canonical declaration.

### D9 — The ADR-0019 always-true test is consciously superseded

`an_always_true_semantics_still_passes_a_fabricated_chain` must no longer stay true.

It is replaced by two negative boundaries:

1. arbitrary downstream code cannot implement or pass an executable semantics oracle to `verify_replay` or `audit_step`; and
2. a fabricated exact-row manifest may produce an internally consistent receipt only under its own distinct semantics id, and that receipt fails validation against the independently expected L3 or literal semantics id.

The second condition is essential. Removing the trait prevents executable equivocation; expected-id validation prevents a caller’s alternative declaration from masquerading as the production oracle.

This does not claim that exact-row data is inherently authoritative. It makes any alternative row set visible, content-addressed, and rejectable by a consumer holding the expected identity.

### D10 — Existing canonical identities and authority remain frozen

The following do not move:

- `DecompositionId`;
- `TreeDerivationId`;
- `JudgementId` for existing `Derived` or `Audited` judgements;
- `DecompVerification` ordinals;
- `TreeVerification` ordinals;
- `Evidence` ordinals;
- `Outcome` ordinals;
- the ADR-0016 route table; and
- every existing file and case under `vectors/`.

The new files are additive:

```text
vectors/generator_semantics_v1.json
vectors/settlement_audit_receipt_v1.json
```

No new crate dependency is introduced. The direction remains:

```text
brix-canon ← brix-semantic ← soc-core ← soc-regimes
                           ↖ brix-lower through its existing edges
```

`brix-semantic` still depends only on `brix-canon`. The receipt lives in `soc-core`, which already depends on both required lower crates.

---

## 4. Consequences

The production settlement audit no longer executes arbitrary caller code to decide primitive relations. It interprets a small canonical relation vocabulary.

Two audits over the same chain but different registry or semantics declarations now have different receipt ids, even though their unchanged `DecompositionId` and `Audited` `JudgementId` remain equal.

The receipt records the exact contextual step checked by `audit_step`. A detached receipt cannot silently move to another observation, endpoint pair, context, registry, or semantics declaration.

L3 obtains a canonical oracle identity without putting plan-specific types into `brix-semantic`: its manifest is expressed solely through `GeneratorId` and `ConfigId`.

The literal semantics remains compact. It does not require enumerating every possible reflexive configuration as finite data.

Custom test semantics become exact row fixtures. This is more verbose than an ad hoc closure but makes their audit assumptions inspectable and reproducible.

No grade moves upward. New manifest, receipt, or validation failures either return the existing `AuditResult::Unknown` or a typed receipt-format/checking error. They never produce `Refuted`.

SOC-LAW-03, SOC-LAW-04, and SOC-LAW-05 remain `enforced` with no reopened issue. This ADR strengthens the input and provenance boundary without weakening an existing gate or adding a publication route. Their normative and executable anchors should be extended with ADR-0020 and its negative tests.

---

## 5. Residuals — what this does not fix

1. **A receipt is not a global trust root.** It identifies the exact semantics declaration used. A consumer must independently know which semantics id is expected. Accepting the id embedded in the receipt as its own expectation moves the original hole into receipt selection.

2. **Caller-declared exact rows are not self-authorizing.** A caller can construct a manifest containing a fabricated finite row. That manifest receives a different id and can be rejected against the production expectation. Without such an expectation, the receipt proves only “this chain was checked under this declared relation.”

3. **L3 offline verification needs the canonical plan or another trusted plan-bound reconstruction.** `ProgramIdV1` is a digest, not a decoder for the plan. A receipt alone cannot reconstruct arbitrary L3 transition rows from that digest.

4. **The receipt does not attest journal inclusion or prefix integrity.** It binds the canonical committed step, including its observation, but not a journal ordinal, preceding chain digest, or following chain digest. A future journal receipt would need those fields and would be minted by `audit_journal`, not silently added to this step receipt.

5. **The existing `Audited` judgement does not acquire the receipt’s stronger scope.** Its frozen evidence still names only the verified decomposition. A consumer that discards the receipt retains the older ADR-0019 guarantee, not the oracle-bound one.

6. **Public authority names remain role claims, not runtime capabilities.** This ADR does not prove that the Rust caller is literally `soc-core::audit`; it makes a receipt independently replayable against expected inputs.

7. **No bridge to kernel primitive relations is established.** Settlement semantics cannot close an ADR-0015 primitive leaf or produce `Proven`.

These are real limits. This ADR closes anonymous executable-oracle equivocation on the production path and makes the remaining declaration choice explicit. It does not make content addressing equivalent to semantic authorization.

---

## 6. Non-goals

- No change to `Decomposition`, `TreeDerivation`, or their ids.
- No edit or re-blessing of an existing vector.
- No outcome, grade, authority, route, or existing evidence-kind change.
- No new `Refuted` path.
- No journal-wide inclusion receipt.
- No signature, signing key, or delegated semantic authority.
- No kernel dependency from `soc-core`.
- No reuse or reinterpretation of `PrimitiveRelationId`.
- No general plugin mechanism for executable semantics predicates.
- No arbitrary algorithmic relation variant beyond the frozen v1 diagonal.
- No claim that a receipt validates a plan whose body is unavailable.
- No change to `commit_tick → audit_step → elaborate_decomposition` behavior for honest inputs.

