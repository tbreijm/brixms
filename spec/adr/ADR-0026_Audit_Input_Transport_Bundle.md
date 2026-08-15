# ADR-0026 — The Audit-Input Transport Bundle, and `brix verify`

Status: **Proposed** (2026-08-16). Closes [ADR-0022](./ADR-0022_Source_Re_Derived_Manifests.md) §5
residual 5 (complete transport of every audit input) and rules on residual 8 (journal inclusion).
Governs issue #290.

Date: 2026-08-16.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail
closed), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md)
(§7 additive versioning, §8 independent vectors),
[ADR-0016: Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md),
[ADR-0019: Verification Tags Are Earned](./ADR-0019_Verification_Tags_Are_Earned.md) (D1/D2/D7 —
the seals this ADR must not reopen), [ADR-0020: Oracle-Bound Audit Receipts](./ADR-0020_Oracle_Bound_Audit_Receipts.md),
[ADR-0022](./ADR-0022_Source_Re_Derived_Manifests.md) (the re-derivation doctrine),
[ADR-0023: Primitive-Relation Identity](./ADR-0023_Primitive_Relation_Identity.md) (⟨D-RELID⟩,
⟨D-DISJOINT⟩).

---

## 1. What is actually missing

The oracle-authentication arc — ADR-0016 §7.1 → 0019 → 0020 → 0021 → 0022 — is complete as
*semantics* and incomplete as *transport*.

`check_l3_audit_receipt_from_source_v1` (`crates/brix-lower/src/l3_audit.rs`) takes its `receipt`,
`step` and `context` as **live Rust values**. `CommittedStep` has a `Canonical` impl and no
decoder. So a verifier in another process cannot be handed the thing it is supposed to verify, and
four ADRs of work terminate in a library entry point with deliberately no CLI command.

This ADR defines the artifact that crosses that boundary, the discipline for decoding it, and the
command that consumes it.

**It closes transport. It does not strengthen any claim.** Everything the bundle enables was
already true of an in-process verifier; the contribution is that a second party can now check it.

## 2. Decision — ⟨D-NOTAG⟩ the wire format carries no verification tag

> The transported step material SHALL NOT contain a `DecompVerification` field, in any encoding, in
> any version. A decoder SHALL reconstruct a decomposition only through
> `Decomposition::recorded(generators, configs)`.

This is the load-bearing decision, and it is what keeps ADR-0019 intact.

ADR-0019 ruled that a verified tag is the **output of a checked transition, never a constructor
input**: it removed `Decomposition::replay_verified` and `TreeDerivation::structure_verified`, made
both artifacts' fields private, refused a `test-support` feature (D7) because a non-default feature
is still an advertised way back in, and pinned the seals with `compile_fail` doctests. A decoder
that reconstructs a `Decomposition` from untrusted bytes is exactly such a route unless it is
designed not to be.

Three routes were considered.

- **Decode the artifact into an explicitly unverified form.** Sound, and effectively what is
  adopted — but only because the unverified form already exists and is already the hot loop's
  vocabulary.
- **Do not transport the artifact at all; transport what is needed to re-derive it.** Selected.
  This is ADR-0022's own move applied one layer down: the verifier reconstructs rather than
  receives, so there is nothing to trust about the reconstruction.
- **Decode into a separately identified `ClaimedDecomposition`.** Rejected. It mints a second
  canonical identity for a thing that is not evidence, gives callers a second vocabulary to confuse
  with the earned artifact, and adds no check. The bundle as a whole is already the claim.

The resulting flow has no shortcut in it:

```text
hostile bytes
  → structural decode (bounded)
  → Decomposition::recorded          ← the only constructor reached
  → CommittedStep
  → receipt check
  → audit_step
  → Decomposition::verify_replay     ← the only promotion, unchanged
  → ReplayVerified
  → ADR-0016 publication fence
```

`verify_replay` refuses any receiver that is not `Recorded`, so a decoded artifact is
*structurally* incapable of arriving pre-verified. That property is not new — it is ADR-0019's, and
⟨D-NOTAG⟩ exists so that transport cannot quietly erode it.

**Why the field must be absent rather than ignored.** `Decomposition` canonically encodes its
verification tag along with its generators and configurations. A wire format that *carried* the tag
would be transporting a claim about its own verification status, and every consumer would then rely
on a decoder remembering to discard it. Absence is checkable; discipline is not.

**No tree material is transported.** This bundle is settlement-side only, so it creates no route to
`TreeDerivation` in any form, and `verify_structure` remains the sole way to earn
`StructureVerified`.

### ⟨D-REISSUE⟩ A receipt is accepted by reproduction, never by decoding

> Receipt bytes SHALL decode only to a crate-private claimed view. There SHALL be no public
> conversion from decoded bytes to `SettlementAuditReceiptV1`. Acceptance SHALL be: replay the
> audit, reissue the receipt locally, and require byte-for-byte equality with the transported bytes.

`SettlementAuditReceiptV1`'s fields are private for the same reason `Decomposition`'s are —
constructing one claims an audit ran (ADR-0020 D5). A public byte-to-receipt constructor would let
any caller mint a receipt naming an audit environment that never existed, which is precisely the
hole ADR-0019 closed on the artifact side.

Reissue-and-compare needs no such constructor. It also closes ADR-0022 residual 5's second half —
"ADR-0020's strict receipt-byte decoder must also be completed where only typed receipt checking
exists" — without the strict decoder ever becoming an issuer.

## 3. Decision — ⟨D-SNAPSHOT⟩ residual 8 is answered with a snapshot, not a proof of inclusion

> The v1 bundle SHALL carry the **complete** journal in commit order. Verification SHALL fold from
> `History::empty`, SHALL require ordinals to be exactly `0..n-1`, SHALL recompute every prefix
> digest, and SHALL require the recomputed final digest to equal the bundle's. It SHALL NOT resume
> from a supplied digest.

A prefix digest plus a step does **not** prove journal inclusion, and the reason is structural
rather than incidental:

- `History::append` folds `h' = H(h_digest, step)` and reads only `self.digest`
  (`crates/soc-core/src/history.rs`). **The ordinal is not hashed in**, so a prefix digest can be
  relabelled with any ordinal at all.
- `History::from_digest(digest, len)` accepts arbitrary values with no check that the digest is
  reachable from `empty`, so a claimed prefix state need not have ever existed.
- Consequently `prefix + step` proves only "this step is the successor of *some claimed* state",
  and `step + final digest` cannot link an interior step to that final digest without the
  intervening material.

So v1 carries everything and re-folds it. That yields O(n) snapshot membership and prefix
integrity. It does not yield O(1) or O(log n) random access, and this ADR does not pretend
otherwise.

**`History` is not changed.** A Merkle or skip-list accumulator would move every existing chain
identity and every artifact containing one, to buy random-access inclusion that nothing currently
needs. If it is ever needed it arrives as an additive `JournalCommitmentV2` with its own vectors.
`CommittedStep`'s field order is frozen ABI and stays exactly `key, observation, decomposition,
src, dst, witness`.

**The honest scope of what a verified bundle establishes**, stated so that no reader over-reads it:

> Under an independently selected L3 program and the registry and semantics re-derived from its
> source, every recorded step in this exact content-addressed snapshot replayed successfully, every
> receipt was reproduced byte-for-byte, and every claimed ordinal and prefix relation was
> recomputed from the empty-history anchor.

It does **not** establish that no later step existed, that this snapshot was the operator's
intended journal, or that it was ever published. Content addressing is not authentication, and a
bundle is not a witness to history.

## 4. Decision — ⟨D-PIN⟩ the verifier's target is supplied from outside the inputs

> `brix verify` SHALL require an expected `ProgramIdV1` from outside both the source and the
> bundle. Absent one it SHALL refuse. The two-argument form SHALL be a usage refusal, never a
> weaker self-consistency mode.

ADR-0022 residual 1 is explicit that "the verifier must independently know which `ProgramIdV1` it
intends to verify" and that source plus a matching receipt "cannot select its own target". A
`brix verify <file.brix> <bundle>` that inferred its target from its inputs would check that the
bundle agrees with the source — a self-consistency check dressed as verification, and exactly the
shape ADR-0020 §2 warns about, where "a consumer that adopts the receipt's own expectation has
authenticated nothing".

So the surface is:

```text
brix verify --expect-program <hex> <file.brix> <bundle>
```

A deployment-provided pin may supply the flag's value. Nothing may derive it from the inputs.

## 5. Decision — ⟨D-BUNDLE⟩ contents and identity

Working backwards from what `check_audit_receipt_v1` consumes — receipt, `CommittedStep`,
`ContextId`, expected registry, expected semantics — and noting that the last two are *re-derived*
from source and therefore must never be transported:

```text
SettlementAuditInputBundleV1
  context            : ContextId
  entries            : [ Entry ]        in commit order
  final_chain_digest : Digest

Entry
  ordinal            : uint
  prefix_digest      : Digest           the chain state immediately before this step
  step_material      : RecordedStepMaterialV1
  receipt_bytes      : bytes            the exact canonical receipt issued for this step

RecordedStepMaterialV1
  key                : Key
  observation        : Observation      decode admits Derived ONLY
  generators         : [ GeneratorId ]
  configs            : [ ConfigId ]
  src, dst           : ConfigId
  witness            : WitnessId
```

Identity is content-derived over the whole of it, following ADR-0023 ⟨D-RELID⟩:

```text
SettlementAuditInputBundleIdV1 = Digest(Domain::Value,
  write_bytes b"brix.soc.audit-input-bundle"
  write_uint  1
  write_str   "brix.soc.audit-input-bundle@1"
  ContextId
  write_list  entries
  write_bytes final_chain_digest
)
```

Per ADR-0023 ⟨D-DISJOINT⟩ the marker bytes are disjoint from every other identity domain, and there
SHALL be no conversion to or from `PrimitiveRelationId`, `GeneratorSemanticsIdV1`,
`SettlementAuditReceiptIdV1`, or any certificate id, in either direction. A test SHALL assert
non-collision against at least `brix.kernel.primitive-relation`,
`brix.semantic.generator-semantics`, `brix.soc.audit-receipt`, `brix.kernel.certificate`,
`brix.soc.quiescence`, `brix.soc.divergence`, `brix.l3.plan` and `brix.l3.run`.

**The observation decoder admits only `Outcome::Derived`.** Every other outcome and every unknown
ordinal is a *format* refusal at decode time. Decoding an `Audited`, `Proven` or `Refuted`
observation and relying on a later stage to reject it would put an unearned outcome into a live
value, however briefly.

**Deliberately not transported:** any verified decomposition; the registry or semantics manifest
(re-derived — transporting them is the authenticated-nothing hole); `SettlementRunV1`; the
transition table; context *contents*; any key, signature or attestation.

Limits, file paths, the expected program id and the source bytes are **not** bundle content and are
not hashed into its identity — they are the verifier's own policy and inputs.

## 6. Decision — ⟨D-DECODELIMITS⟩ every bound fires before the work it governs

> Decoding SHALL be governed by a noncanonical `AuditDecodeLimits`, contributing to no identity.
> Every bound SHALL be enforced **before** the work it governs, not after it.

The ordering requirement is the whole content of this decision; a bound applied afterwards has
already paid the cost it exists to avoid. This is not hypothetical — the same defect existed on the
source path, where `max_source_bytes` was enforced only after `from_utf8` had validated the entire
slice, and the test that claimed otherwise asserted an error only reachable *after* that work
(fixed under #290).

Bounds: total bundle bytes; journal steps; entry bytes; receipt bytes; generators and configs per
step; cumulative decomposition links; cumulative framed bytes.

Normative ordering: bound the file before reading it whole — the CLI's existing `read_to_string`
path is unbounded and MUST NOT be reused for bundles; check total bytes before hashing or
constructing a reader; validate marker, version and profile before any journal or source work;
validate every count and convert it with a **checked** `u64 → usize` before allocating or looping;
validate a frame length before slicing, copying or reserving; charge cumulative totals with
`checked_add` before the governed work; require `configs == generators + 1` before constructing the
vectors; reject duplicate, decreasing, skipped or overflowing ordinals before any semantic work;
fully consume every nested frame and reject outer trailing bytes; and only then touch the source.

**v1 contains no maps and no attacker-directed recursion**, which eliminates duplicate-key and
depth attacks rather than budgeting for them. A future version introducing maps MUST require
strictly increasing canonical key bytes and reject duplicates — it must never sort hostile input,
and must never inherit `CanonWriter`'s last-write-wins behaviour on it.

Every refusal is typed and fail-closed. No decode failure yields a weaker pass, and none yields
`Refuted` — absence is not refutation (ADR-0015 §8.8's rule, holding here too).

## 7. Ownership and TCB

`soc-core` owns the bundle, the decoder and snapshot validation: it already owns `CommittedStep`,
`Journal`, `History`, receipt issuance and the contextual audit stages. `brix-lower` owns only the
source-derived L3 wrapper. The CLI owns file I/O and rendering.

**Not `brix-semantic`.** Putting journal concepts into the artifact-owning crate is the placement
ADR-0019 rejected, and `brix-semantic` continues to depend only on `brix-canon`.

No new third-party dependency and no new Cargo edge, so the canonical-identity closure —
`brix-semantic`, `brix-canon`, `blake3`, `indexmap`, `unicode-normalization` — is unchanged. What
does grow is trusted *code*: a decoder and snapshot validator inside `soc-core`, which the audit
inventory already lists as trusted. `CanonReader` is used rather than extended; it already has
untrusted-byte consumers in the kernel and saturation certificate decoders, and bundle semantics
do not belong in `brix-canon`.

## 8. The CLI surface

**Producer.** `brix audit <file.brix> --bundle <out>`. A bundle is emitted only when a journal
exists, **every** step returned `AuditResult::Audited`, every receipt came from that audit, and the
prefix and final digests were recomputed from `History::empty`. If any step is `Unknown`, **no
bundle is written** — a partial bundle with missing receipts would be a different artifact, and
silently emitting one would be the fail-open version of this feature.

**Verifier.** On complete success, `brix verify` prints the expected program, context, validated
bundle id, validated final chain digest, one `verified` line per receipt, the membership count, and
`status: audit-bundle-verified`, exiting zero. On any refusal it prints
`status: unknown (<stable-reason>)` and exits nonzero — the discipline `brix run` already holds
when it cannot re-verify its own certificate.

**What `brix verify` must refuse to say:** `Proven` or `Refuted`; `quiescent`, `settled`,
`complete` or `fixpoint`; that the bundle selected or authenticated the expected program; that the
`ContextId`'s *contents* were decoded or validated; that a journal was published or is globally
complete; that any key or signer authorized anything; that an audited journal strengthens a
quiescence claim; or a bare `audited` when any receipt, step or membership check was skipped.

## 9. Hard boundaries

A decoded bundle SHALL NEVER:

1. install or select `ReplayVerified` or `StructureVerified`;
2. produce a `SettlementAuditReceiptV1` purporting to have been issued;
3. construct an `Audited`, `Proven` or `Refuted` judgement ahead of the real checked transition;
4. bypass `Decomposition::recorded`, `verify_replay`, `audit_step`, or the ADR-0016 publication
   fence;
5. supply the expected registry or semantics for the strict L3 path;
6. select its own expected `ProgramIdV1`, or authorize its own `ContextId`;
7. act as a key, signature, attestation, delegation or trust root — ADR-0021 stays Proposed and
   this path must not become a second route to it;
8. authenticate historical occurrence, operator intent, source provenance or journal finality;
9. prove quiescence, or validate a `SettlementRunV1`;
10. normalize malformed, duplicate, unordered, unknown-version or trailing input into acceptance;
11. fall back to generic receipt checking after strict L3 verification fails;
12. become a `test-support` feature or any other advertised unchecked constructor route (ADR-0019
    D7);
13. change `CommittedStep`, `History`, existing receipt fields, existing ordinals, or any frozen
    vector in place.

## 10. Staged implementation

Each stage is separately mergeable. Two stages change what a verifier can accept, and they are
named as such.

- **Stage A — decoder prerequisites.** Checked length conversion in `CanonReader`; the source-size
  bound fired before UTF-8 validation. *Landed ahead of this ADR under #290/#291.*
- **Stage B — strict receipt-byte checking.** The crate-private claimed view, `AuditDecodeLimits`,
  and `check_audit_receipt_bytes_v1` by reissue-and-compare, with negative and independent vectors.
  **First stage where a verifier accepts receipt bytes it previously could not** — still requiring
  typed step, context and environment.
- **Stage C — recorded step material.** The transport projection with no verification field,
  `Derived`-only observation decoding, reconstruction solely via `Decomposition::recorded`, and
  `compile_fail` gates against any conversion route.
- **Stage D — bundle identity and snapshot checking.** The artifact, the full-history fold from
  `empty`, ordinal/prefix/final-chain checks, the checked issuer, v1 vectors under the two-consumer
  discipline, and marker non-collision tests.
- **Stage E — L3 source integration.** `check_l3_audit_input_bundle_from_source_v1`: independently
  check the expected program, derive registry and semantics once, then validate every receipt.
  **First stage providing complete cross-process audit acceptance.**
- **Stage F — producer CLI.** `brix audit --bundle`, atomic output, no artifact on any `Unknown`.
- **Stage G — verifier CLI.** The pinned `brix verify` surface, with exact rendering and exit tests.
- **Stage H — hostile-input gates.** Truncations, nonminimal lengths, huge counts, cumulative
  overflow, duplicate and sparse ordinals, altered prefixes, nested frame trailing bytes, wrong
  markers and profiles, and both limit boundaries. **Release gates, not deferred polish.**

## 11. Compatibility

- Additive throughout. No existing canonical id moves, no frozen vector is edited, no ordinal is
  renumbered, and `CommittedStep`/`History`/`SettlementAuditReceiptV1` are untouched.
- `vectors/settlement_audit_input_bundle_v1.json` is new, guarded by a production encoder and an
  independent primitive-`CanonWriter` reconstruction (ADR-0013 §8).
- Sparse journals, optional receipts, a different membership structure, or any added field require
  a v2 bundle and a new marker — never a reinterpretation of v1.

## 12. Open decisions

- Whether `brix verify` should also accept a pinned expected **bundle** id, so a deployment can
  name the exact snapshot it expects rather than only the program. Not blocking; it strengthens
  target selection the same way ⟨D-PIN⟩ does and can be added additively.
- Whether random-access journal inclusion is ever wanted. If so it is an additive
  `JournalCommitmentV2`, never a change to the existing fold (§3).
