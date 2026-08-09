# ADR-0021 — Scoped Settlement-Semantics Authorization Attestations

Status: **Proposed** (2026-08-09) (rules on [ADR-0020](./ADR-0020_Oracle_Bound_Audit_Receipts.md) §5 residuals 1–3; follows [ADR-0013](./ADR-0013_Canonical_Certificate_Envelope.md), [ADR-0016](./ADR-0016_Authority_Publication_Fence.md), [ADR-0019](./ADR-0019_Verification_Tags_Are_Earned.md), and accepted ADR-0020).

Date: 2026-08-09.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4 and §4.1 outcome lattice and verifier authority), [ADR-0012: L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (§3 canonical plan identity and §5 audit boundary), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§6 fail-closed decoding, §7 additive versioning, §8 independent vectors), [ADR-0016: Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md), [ADR-0019: Verification Tags Are Earned](./ADR-0019_Verification_Tags_Are_Earned.md), and [ADR-0020: Oracle-Bound Settlement Audit Receipts](./ADR-0020_Oracle_Bound_Audit_Receipts.md).

This ADR introduces the first verification input in BrixMS that cannot be recomputed from the candidate artifacts. It adds a scoped cryptographic authorization layer above `SettlementAuditReceiptV1`. It changes no existing outcome, evidence kind, authority route, canonical id, ordinal, or frozen vector.

---

## 1. The finding

ADR-0020 replaces caller-supplied executable settlement predicates with canonical declared data. A receipt identifies the exact `GeneratorRegistry` and `GeneratorSemanticsV1` under which replay occurred.

That closes anonymous oracle substitution, but it does not make the declaration authoritative. A consumer must still obtain the expected `GeneratorSemanticsIdV1` independently.

For L3, that expectation is plan-specific:

```text
L3PlanV1
  → ProgramIdV1
  → L3TransitionTable
  → GeneratorSemanticsV1
  → GeneratorSemanticsIdV1
```

`ProgramIdV1` is a digest, not a decoder. An offline verifier holding only the program id, manifest, and receipt cannot reconstruct arbitrary transition rows. Re-derivation currently requires the canonical plan or another trusted plan-bound reconstruction.

There is no static list of expected semantics ids to distribute: every plan produces another id. A verification key, by contrast, is one static trust anchor capable of authenticating authorizations for many plan-specific ids.

That asymmetry is the reason for this ADR. The argument is not that signatures are intrinsically more truthful than hashes or replay. The argument is:

> A scoped static key can authenticate a changing sequence of plan-specific authorization statements without requiring every offline verifier to possess every canonical plan.

A signature nevertheless establishes attribution, not truth. An authorized signer can sign a fabricated row declaration. The resulting signature is valid and the declaration may pass replay for a fabricated chain. Cryptographic sealing makes unauthorized substitution harder; it does not make the signer infallible.

---

## 2. What the signature attests

Four candidate claims were considered.

**“These rows are correct.”** Rejected. An offline verifier without the plan cannot falsify that claim. It makes the signer a semantic truth oracle and conceals the exact trust being introduced.

**“This manifest was derived from plan P by deriver D.”** Rejected as the primary claim. It is falsifiable when the verifier holds `P`, but then the verifier can derive the manifest itself. Without `P`, it is another process assertion the verifier must trust.

**“This receipt was issued by this signer.”** Rejected. It authenticates one audit event but does not independently establish why the receipt’s semantics id was the authorized one. It also requires one signature per receipt rather than one authorization per plan/profile.

**“This exact registry/semantics pair is authorized for settlement-audit use in deployment/profile S for plan subject P.”** Accepted.

The selected claim is a **deployment authorization**, not a derivation claim, even though it binds a plan identity to a manifest identity.

Normatively, a valid v1 signature says only:

> The holder of the private key corresponding to `key_id`, acting within the scope assigned to that key by the verifier’s trust policy, authorized this exact generator registry and settlement-semantics declaration for audit-receipt acceptance in this deployment, under this audit profile, for this plan subject.

It does not say:

- that the rows are mathematically or operationally correct;
- that a particular implementation derived them;
- that the plan was valid;
- that the signer inspected the plan;
- that the receipted audit replay succeeded;
- that the step occurred in a journal;
- or that any proposition is `Proven`, `Refuted`, or `Audited`.

---

## 3. Decision

### D1 — Sign a deployment authorization, not the receipt or bare manifest

The signed subject is `SettlementSemanticsAuthorizationStatementV1`.

It binds:

1. an authorization purpose;
2. an exact deployment scope;
3. the settlement audit-checker profile;
4. a typed plan-subject namespace;
5. the plan-subject digest;
6. the `GeneratorRegistryId`; and
7. the `GeneratorSemanticsIdV1`.

For L3, the subject namespace is fixed to:

```text
brix.l3.program@1
```

and the subject digest is the exact `ProgramIdV1` digest.

The registry and semantics declarations themselves remain separate canonical artifacts. The statement signs their ids, not duplicated encodings of their bodies.

The authorization is reusable across all step receipts that name that same registry and semantics pair. `SettlementAuditReceiptV1` remains unsigned and unchanged. Receipt integrity still comes from canonical binding and replay; the authorization answers only whether its declared audit environment is permitted in the named scope.

Signing a bare manifest is insufficient because the same declaration could then be replayed across deployments, profiles, or plan subjects. Signing individual receipts is unnecessarily narrow and leaves the relation-authorization claim implicit.

### D2 — The v1 authorization statement has a frozen content identity

The v1 statement material is encoded with `CanonWriter` in this exact order:

| # | Field | Encoding |
|---|---|---|
| 1 | Marker | `write_bytes(b"brix.soc.settlement-semantics-authorization")` |
| 2 | Statement version | `write_uint(1)` |
| 3 | Purpose | `write_str("brix.soc.accept-audit-receipt@1")` |
| 4 | Deployment | `write_str(deployment)` |
| 5 | Audit profile | `write_str("brix.soc.audit-factorization@1")` |
| 6 | Subject namespace | `write_str(subject_namespace)` |
| 7 | Subject digest | `write_bytes(subject_digest.as_bytes())` |
| 8 | Generator registry | canonical `GeneratorRegistryId` |
| 9 | Generator semantics | canonical `GeneratorSemanticsIdV1` |

`deployment` and `subject_namespace` must be non-empty ASCII identifiers under a fixed v1 grammar. They are exact values; no Unicode normalization, case folding, aliases, wildcard matching, or prefix matching is permitted.

`SettlementSemanticsAuthorizationIdV1` is:

```text
Hash(Domain::Value, statement_material)
```

The marker, version, purpose, field order, framing, and subject interpretation are frozen v1 ABI.

The statement id identifies the authorization being signed. It is also the unit for targeted revocation.

### D3 — The signature is carried in a detached, versioned envelope

`SignedSettlementSemanticsAuthorizationV1` is a sibling artifact transported alongside the manifest and receipt. It is not inserted into either existing artifact.

Its encoded fields are:

| # | Field | Encoding |
|---|---|---|
| 1 | Envelope marker | `write_bytes(b"brix.soc.signed-semantics-authorization")` |
| 2 | Envelope version | `write_uint(1)` |
| 3 | Signature suite | `write_str("ed25519-rfc8032@1")` |
| 4 | Attestation key | canonical `AttestationKeyIdV1` |
| 5 | Statement material | `write_bytes(statement_material)` |
| 6 | Signature | `write_bytes(signature)` |

The signature input is not the envelope itself. It is this separately domain-separated preimage:

| # | Field | Encoding |
|---|---|---|
| 1 | Signature-input marker | `write_bytes(b"brix.soc.attestation-signature")` |
| 2 | Signature-input version | `write_uint(1)` |
| 3 | Signature suite | `write_str("ed25519-rfc8032@1")` |
| 4 | Attestation key | canonical `AttestationKeyIdV1` |
| 5 | Statement material | `write_bytes(statement_material)` |

Including the suite and key id in the signed bytes prevents either from being substituted without invalidating the signature.

`AttestationKeyIdV1` is derived from a domain-separated preimage containing:

```text
marker, key-id version, signature suite, exact public-key bytes
```

The verifier recomputes it from the trusted public key. A caller-supplied key id is never accepted without that comparison.

The signed envelope deliberately has **no content-addressed artifact id**. The content-addressed object is the authorization statement. The signature is detached from that identity.

### D4 — Determinism is preserved twice

The v1 suite is Ed25519 under RFC 8032, using a maintained strict-verification Rust implementation in the `ed25519-dalek` class.

Ed25519 is deterministic for a fixed private key and message. V1 signing therefore requires no random nonce and no `SystemTime`, environment, host, or process input.

That is not the only protection. The signature bytes are outside:

- `SettlementAuditReceiptIdV1`;
- `GeneratorSemanticsIdV1`;
- `SettlementSemanticsAuthorizationIdV1`; and
- every existing canonical preimage.

The signed envelope has no id whose stability could depend on signature generation. Its encoding is deterministic for its supplied fields, but its bytes are not used as the durable identity of the authorization.

This separation is mandatory even though the first suite is deterministic. A future signature scheme may be randomized without changing the statement id or any receipt id.

Tests may sign with a fixed, visibly non-production fixture seed. Production key generation, storage, and randomness do not occur inside determinism or reproducibility tests.

### D5 — A key is authoritative only through an independently installed scope

A naked public key is not a trust policy.

The verifier holds an out-of-band `AttestationTrustPolicy` whose entries associate:

- `AttestationKeyIdV1`;
- the exact signature suite;
- the exact public-key bytes;
- key status;
- a finite set of exact allowed scopes; and
- optionally revoked authorization-statement ids.

A key scope is an exact triple:

```text
(deployment, audit_profile, subject_namespace)
```

V1 has no wildcard scope.

For L3, a key may therefore be authorized to sign any changing `ProgramIdV1` under one exact deployment and the `brix.l3.program@1` namespace. That is the deliberate dynamic authority needed to avoid distributing every semantics id.

It cannot, through this protocol:

- authorize another deployment;
- authorize another subject namespace;
- authorize another audit profile;
- sign proof certificates or refutations;
- publish a `Judgement`;
- alter the ADR-0016 route table; or
- turn a settlement result into `Proven`.

The same raw cryptographic key must not be reused in another protocol. The signature-input domain separator prevents accidental cross-protocol verification, but key separation remains an operational requirement.

A key authorized for one L3 deployment can still authorize a fabricated manifest for any plan subject within that scope. Scope limits the blast radius; it does not make the signer truthful.

### D6 — Signature verification is a precondition, not an epistemic judgement

A new strict acceptance operation sits above ADR-0020 receipt checking:

```text
accept_authorized_audit_receipt_v1(
    receipt,
    committed_step,
    context,
    registry,
    semantics,
    signed_authorization,
    expected_deployment,
    expected_subject,
    trust_policy
)
```

Conceptually it:

1. strictly decodes the signed authorization envelope;
2. strictly decodes and canonicalizes its statement;
3. resolves the key in the independently supplied trust policy;
4. checks key status and exact scope;
5. verifies the signature;
6. requires the statement’s deployment, profile, and plan subject to equal the independently expected values;
7. requires the supplied registry and semantics ids to equal the signed ids;
8. requires the receipt to name those same ids; and
9. invokes ADR-0020’s full receipt checker, which replays the committed step and reconstructs the `Audited` result.

The expected semantics id used by receipt checking comes from the successfully verified authorization statement, not from the receipt or manifest being checked.

Signature success alone returns no `Judgement`, `AuditResult::Audited`, `Evidence`, or epistemic grade. Only replay may reconstruct the existing `Audited` result through the existing ADR-0016 route.

The strict operation may return an ordinary noncanonical report containing the accepted statement id and key id for diagnostics and provenance. That report is not evidence and has no route to publication.

No new outcome is added. No new `Evidence` variant is appended. No signature route is added to `ROUTES`.

### D7 — Signature failure is never replay, refutation, or a downgrade-hiding pass

All signature, policy, scope, and format failures return a typed attestation error and produce no accepted receipt.

An orchestration layer that must express the failure in the epistemic lattice maps it to `Unknown`. It never maps it to `Refuted`.

An already-existing `Audited` judgement is not mutated or regraded when attested acceptance fails. The stricter consumer merely refuses to accept it under the attested profile.

In particular:

| Condition | Required result |
|---|---|
| No trust policy or no trust anchor installed | `NoTrustAnchor`; no signature or receipt acceptance |
| Envelope names a key absent from a non-empty policy | `UnknownKey`; no fallback to another key |
| Key is present but marked revoked | `RevokedKey`; reject before receipt acceptance |
| Statement id is explicitly revoked | `RevokedAuthorization`; reject |
| Known key, invalid signature | `InvalidSignature`; reject |
| Unknown suite or envelope version | typed unknown-suite/version error; reject |
| Missing signed authorization | `MissingAttestation`; never treat the unsigned receipt as trusted |
| Valid signature, wrong deployment/profile/subject | typed scope mismatch; reject |
| Valid authorization, failed receipt replay | receipt-checking error or `Unknown`; never `Refuted` |

The unauthenticated ADR-0020 receipt checker may remain available for contexts that explicitly require only replay under independently supplied expectations. A deployment/profile configured to require attestation must expose no silent fallback from the strict operation to that lower-level checker.

### D8 — Distribution is out of band; rotation and revocation are explicit

V1 does not introduce WebPKI, X.509, certificate chains, network key discovery, or a self-declared key document.

Trust-policy installation is a deployment operation. A public key and its scope are pinned through configuration, a binary release, or another channel already trusted by the verifier. The signed envelope cannot introduce its own key or enlarge that key’s scope.

Rotation proceeds by:

1. installing the new scoped public key while the old key remains active;
2. re-signing every currently needed authorization statement with the new key;
3. distributing those envelopes;
4. verifying that consumers accept the new key; and
5. marking the old key revoked.

Revoking a key invalidates every envelope under that key in the current policy, including envelopes created before compromise. V1 has no trusted timestamp or transparency log with which to distinguish a genuine historical signature from a compromised key backdating a new statement.

A single mistaken authorization may instead be rejected by placing its `SettlementSemanticsAuthorizationIdV1` in the policy’s revoked-statement set.

Offline verification can determine revocation only relative to the trust-policy snapshot it holds. It cannot prove that the snapshot is current. Automated policy distribution, freshness proofs, historical validation, and transparency are deferred.

The consequence is explicit: an offline verifier with stale policy may continue accepting a key or statement that has since been revoked.

### D9 — Algorithm agility occurs through envelope versions

V1 recognizes exactly one suite:

```text
ed25519-rfc8032@1
```

Unknown suite identifiers fail closed even when the envelope otherwise has the v1 shape.

A future primitive does not reinterpret or extend v1 opportunistically. It requires:

- `SignedSettlementSemanticsAuthorizationV2`;
- a new envelope marker or version dispatch arm;
- a pinned suite and exact key/signature validation rules;
- its own signature-input definition;
- its own frozen vectors; and
- an explicit trust-policy opt-in.

The v1 authorization statement may be signed by both a v1 and future envelope during migration because its semantic identity excludes the signature. This permits algorithm rotation without changing the authorized registry, manifest, plan subject, or statement id.

If Ed25519 is disabled or compromised, policy revokes its keys and v1 envelopes cease to be acceptable. Old readers reject v2 rather than interpreting it as v1.

### D10 — Signing and verification live above `soc-core`

A new trusted crate, `brix-attest`, owns:

- authorization-statement encoding and identity;
- signed-envelope encoding and decoding;
- key ids;
- trust-policy evaluation;
- strict signature verification;
- the attested receipt-acceptance wrapper; and
- a typed signing operation over already-loaded key material.

Its production edges are:

```text
brix-attest → brix-canon
brix-attest → brix-semantic
brix-attest → soc-core
brix-attest → Ed25519 implementation crate
```

The direction toward `soc-core` is deliberate. `soc-core` continues to own audit replay and receipt checking without depending on cryptography. This preserves the existing base boundaries:

```text
brix-semantic → brix-canon only
soc-core      → brix-canon + brix-semantic
```

Placing the signature crate directly under `soc-core` would make the crypto implementation part of the settlement-commitment closure merely because Cargo dependencies are crate-wide. A higher attested-acceptance layer confines the widening to consumers that require this stronger policy.

The trusted-boundary inventory gains a row:

| Boundary | Workspace closure | Added production closure |
|---|---|---|
| Attested settlement-audit acceptance | `brix-attest`, `soc-core`, `brix-semantic`, `brix-canon` | Ed25519 implementation and its production transitive dependencies, plus the existing `blake3`, `indexmap`, and `unicode-normalization` closure |

This is a real TCB widening. The signature implementation parses attacker-controlled keys and signatures and decides whether the dynamic semantics expectation is authorized.

The assumed dependency class is a maintained, pure-Rust, permissively licensed Ed25519/RFC-8032 implementation with strict verification, such as the `ed25519-dalek` class. BrixMS does not implement curve or signature arithmetic locally.

Every transitive production dependency enters the recorded closure. `DEPS.md` must justify the direct dependency; `deny.toml` must permit every license rather than adding a blanket exception; advisories and source restrictions remain fail-closed.

Private-key file access, hardware-security-module integration, secret storage, and key generation are outside `brix-attest`. Its signing API accepts already-loaded signing key material and never reads a path, environment variable, network source, or system clock.

### D11 — Existing canonical ABI remains untouched

None of the following moves:

- `DecompositionId`;
- `TreeDerivationId`;
- `GeneratorSemanticsIdV1`;
- `SettlementAuditReceiptIdV1`;
- `JudgementId`;
- `Outcome` ordinals;
- `Evidence` ordinals;
- `DecompVerification` ordinals;
- `TreeVerification` ordinals;
- the ADR-0016 `ROUTES` table; or
- any existing vector file.

In particular:

```text
vectors/generator_semantics_v1.json
```

is not edited or re-blessed.

The new additive vector is:

```text
vectors/settlement_semantics_authorization_v1.json
```

It freezes:

- statement material;
- statement id;
- key-id material;
- key id;
- signature input;
- fixture public key;
- fixture signature; and
- complete signed-envelope bytes.

Every case is consumed twice:

1. through the production artifact encoders and verifier; and
2. through an independent reconstruction using primitive `CanonWriter` calls that repeats all markers, versions, strings, field order, and framing and never calls the statement or envelope’s own `canon_write`.

A fixed non-production signing fixture may additionally reproduce the exact Ed25519 signature. The primitive canonical reconstruction remains independent of the production encoders even if it consumes that fixed signature as data.

### D12 — The L3 adapter is small and one-directional

`brix-attest` does not depend on `brix-lower`.

`brix-lower` supplies the L3 adapter that converts:

```text
ProgramIdV1
```

to the generic authorization subject:

```text
subject_namespace = "brix.l3.program@1"
subject_digest    = ProgramIdV1.digest()
```

Authorization issuance must obtain its registry and semantics from the same plan-bound `L3TransitionTable` used by the production L3 audit adapter.

ADR-0020’s requirement to eliminate the unchecked `(ProgramIdV1, &L3PlanV1)` table-construction pair is a prerequisite. An issuer must not sign a manifest built from a table whose program/plan consistency was only asserted by its caller.

An offline L3 verifier then needs:

- the receipt and committed step;
- the registry and semantics declaration;
- the signed authorization envelope;
- the expected deployment and, when target identity matters, expected `ProgramIdV1`; and
- the scoped trust policy.

It does not need the plan body to determine whether the manifest id was authorized.

If it does not independently supply the expected `ProgramIdV1`, it learns only that the signer authorized some plan subject in that deployment. It does not establish that the plan is the one the user intended.

### D13 — This directly shrinks residual 3 and only partially shrinks residual 2

ADR-0020 residual 3 is the primary result.

An offline verifier can now obtain the dynamic expected semantics id from a signature checked against one static, scoped public-key entry. It no longer needs the canonical plan merely to decide which manifest id is authorized.

ADR-0020 residual 2 is narrowed only in its authorization sense:

- a caller’s unsigned manifest is no longer self-authorizing in an attestation-required profile;
- a manifest signed by an unknown, absent, or revoked key is rejected;
- a manifest signed outside the key’s exact scope is rejected.

Its truth component remains:

- an authorized signer can authorize fabricated rows;
- valid authorization does not prove derivation from the plan;
- replay proves only that the chain conforms to those authorized rows.

The hole becomes smaller and more operationally bounded. It does not disappear.

---

## 4. Why the non-cryptographic alternative remains stronger for L3

Shipping the canonical L3 plan, or another independently checkable plan-bound reconstruction, can solve the same offline-selection problem without a signing key.

A verifier that can:

1. decode and validate the canonical plan;
2. recompute `ProgramIdV1`;
3. build the transition table; and
4. derive `GeneratorSemanticsV1`

obtains a stronger result than this ADR. It checks that the rows follow from the available plan under the local deriver. It need not trust an authorizer not to lie, and it does not add a signature implementation or key lifecycle to the TCB.

Signing is preferable only under the selected operational assumptions:

- the plan body is unavailable, confidential, too large, or intentionally not distributed;
- a durable plan decoder or plan-bound reconstruction format is unavailable;
- offline verifiers can pin one deployment key more readily than they can receive every plan;
- and the deployment accepts the signer as the authority deciding which semantics declaration may be used.

Where the canonical plan can reasonably be shipped, plan re-derivation should remain the high-assurance verification path. The signed authorization is a plan-unavailable path, not a replacement for stronger evidence.

---

## 5. Consequences

A deployment can authorize arbitrarily many plan-specific semantics declarations through one scoped public key.

Changing a plan, registry, or row declaration changes the signed statement id and requires a new signature. It changes no existing receipt, judgement, decomposition, or semantics encoding rule.

A valid signed authorization may be reused for many receipts under the same plan and deployment. Signing throughput is therefore proportional to authorized plan revisions, not journal steps.

Receipt replay remains the only route to `Audited`. Attestation adds no route to `Proven` and no route at all to `Refuted`.

The signing key is a real runtime cryptographic capability, but it is not an ADR-0016 `Authority`. `Authority::AuditChecker` remains the role claim governing publication after replay. `AttestationKeyIdV1` identifies a key whose possession enables a separate deployment-authorization act. Neither substitutes for the other.

Consumers that discard the signed authorization retain only ADR-0020’s ordinary oracle-bound receipt guarantee.

Consumers that accept the envelope’s deployment or plan subject as their own expectation have authenticated the signer’s choice, not their own intended target.

---

## 6. Residuals — what this does not fix

1. **An authorized signer can sign a lie.** The signature authenticates authorization, not row correctness or plan derivation.

2. **The signer’s decision procedure is not audited.** V1 does not record which plan bytes, deriver implementation, review, or approval workflow caused the signer to authorize a manifest.

3. **Offline revocation has no freshness proof.** A verifier can reject according to its installed policy snapshot but cannot know that no newer revocation exists.

4. **Historical validity is not established.** Revoking a key invalidates all of its envelopes under current policy. V1 has no trusted timestamp or append-only transparency log.

5. **Trust-policy distribution remains external.** Compromise of the channel that installs keys or scopes compromises this layer.

6. **Target selection remains independent.** Without an independently expected deployment and plan subject, the verifier proves only that the signer authorized the subject named in the statement.

7. **Canonical plan transport remains unsolved.** This ADR does not define a durable `L3PlanV1` decoder or a plan-bound transition-table artifact.

8. **The receipt remains step-scoped.** No journal ordinal, prefix digest, journal inclusion, or history consistency is signed or introduced.

9. **The attestation is not part of `Audited` evidence.** The existing judgement id does not acquire authorization provenance. A consumer must retain the statement and envelope separately.

10. **ADR-0016 authorities remain role claims.** This ADR creates a separate signing capability; it does not make `SettlementKernel`, `AuditChecker`, or `ProofKernel` unforgeable runtime principals.

The next actionable trust question is therefore precise:

> Should signer authorization remain an operational root, or must future verification also carry either the canonical plan or an independently checkable, transparently logged record of how the signer derived and approved the manifest?

That question is not answered here.

---

## 7. Non-goals

- No claim that a signature makes settlement rows true.
- No signature over individual audit receipts.
- No signature inside an existing content-addressed preimage.
- No change to the outcome lattice or its ordering.
- No new `Evidence` variant.
- No ADR-0016 route-table change.
- No publication capability derived from a signing key.
- No `Refuted` path.
- No WebPKI, X.509, certificate chain, network key discovery, timestamp authority, or transparency log.
- No private-key storage, file format, HSM integration, or recovery protocol.
- No system-time input in verification.
- No reinterpretation of v1 under another signature primitive.
- No edit to an existing vector or canonical id.
- No dependency from `brix-semantic` other than `brix-canon`.
- No dependency from `soc-core` on the cryptographic layer.
- No replacement of plan-based L3 re-derivation where the plan is available.
