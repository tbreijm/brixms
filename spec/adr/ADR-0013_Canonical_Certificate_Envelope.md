# ADR-0013 — Canonical Proof-Certificate Envelope, Version 1

Status: **Accepted** (2026-08-02, ratified by user) (refines [ADR-0003](./ADR-0003_Proof_Kernel_Profile.md) §6; governs `crates/brix-kernel/src/certificate.rs` and `vectors/kernel_certificate_v1.json`).

Date: 2026-08-02.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§3 D1, §5.2), [ADR-0003: Proof Kernel Profile](./ADR-0003_Proof_Kernel_Profile.md) (§6, the kernel-agnostic certificate contract), [ADR-0004: Kernel Profile 1.1](./ADR-0004_Kernel_Profile_1_1.md), [ADR-0006: Kernel Profile 1.2](./ADR-0006_Kernel_Profile_1_2.md), `spec/BrixMS_v9_0.md` Appendix G (canonical encoding), and the trusted-boundary audit in `docs/audit/issue-63/tcb-proof.md`.

This ADR pins the byte layout that gives a **native** accepted proof certificate its identity. It defines no proof rules and changes no calculus.

---

## 1. Context

ADR-0003 §6 fixes the certificate *contract* — `Certificate { verifier: VerifierId, certificate_id: CertificateId }` — and deliberately leaves the certificate payload to the kernel: as `brix-semantic` puts it, a `CertificateId`'s "internal encoding is the kernel's concern; the substrate only references it."

The native kernel exercised that freedom badly. `brix-kernel::acceptance` derived the payload as:

```rust
let cert_payload = format!("{context:?}:{proposition:?}:{term:?}");
let certificate_id = CertificateId::from_canon(cert_payload.as_bytes());
```

This made a **durable** theorem identity depend on Rust `Debug` output — a formatting detail with no stability contract, which a `#[derive]` reorder or a field rename silently changes. It is audit finding **C-1**, with **C-2** (no frozen certificate vectors) and **D-1** (ADR-0003 promises canonical explicit terms while the identity was Debug-derived), and it is why **SOC-LAW-01** (canonical identity) and **SOC-LAW-12** (verifier closure) were held at `Partial`.

The regression was narrow rather than deliberate: the identity was originally canon-encoded and was replaced with `format!` while the Slice-2b constructs were landing. Every `Canonical` impl needed to do this properly — `ObjectTerm`, `Prop`, `Var`, `TermKind`, `ExplicitTerm` — already exists in `crates/brix-kernel/src/term.rs` with frozen, append-only enum ordinals.

`Debug` in **diagnostics** is not a finding, and this ADR does not touch it: the `RejectionReason`/`Malformed` strings built throughout `check.rs` stay exactly as they are. Only identity material is in scope.

## 2. The v1 preimage

The certificate preimage is written with `brix-canon`'s primitive operations in **exactly** this order:

| # | Field | Writer call | Bytes (identity fixture) |
|---|---|---|---|
| 1 | Envelope marker | `write_bytes(b"brix.kernel.certificate")` | `0117` + 23 bytes |
| 2 | Format version | `write_uint(1)` | `0101` |
| 3 | Kernel profile | `write_str("brix.kernel.profile@1.2")` | `0117` + 23 bytes |
| 4 | Verifier | canonical `VerifierId::named("brix.kernel@0.1")` | `0120` + 32-byte digest |
| 5 | Requested context | canonical `ContextId` | `0120` + 32-byte digest |
| 6 | Proposition | `write_bytes(<canonical `Prop` bytes>)` | `01`·len + payload |
| 7 | Explicit term | `write_bytes(<canonical `ExplicitTerm` bytes>)` | `01`·len + payload |

Fields 4 and 5 are length-framed 32-byte digests because that is how `brix-semantic`'s `digest_id!` macro and `ContextId` implement `Canonical`.

The identity is then

```
CertificateId = Hash(Domain::Value, preimage)
```

via `CertificateId::from_canon`, i.e. `blake3("canon/1" ++ ":value:" ++ preimage)`.

**`write_str`, not `write_ident`, for field 3.** A profile name is a *value*, not an identifier: it must preserve its exact code points rather than NFC-fold. The string is ASCII, so the bytes are identical today either way — the choice is pinned here so it is never "corrected" later into a different encoding.

The encoder writes the verifier constant in field 4 unconditionally rather than accepting one from the caller. The native encoder is therefore structurally incapable of minting a foreign-verifier identity; only hand-assembled bytes can claim one, and the reader rejects those.

## 3. Why the payloads are length-framed opaque blobs

Fields 6 and 7 wrap the proposition and term in `write_bytes` rather than splicing their canonical bytes inline. This is the load-bearing decision in the layout.

Length framing lets a reader locate every field boundary, and therefore reject truncation, misalignment, and trailing bytes, **without a general recursive decoder for proof artifacts**. Splicing the payloads inline would make "is this envelope well-formed?" equivalent to "can I parse an arbitrary proof tree?", which is a much larger trusted surface and is deliberately out of scope here (it belongs to the durable-artifact work tracked under #56).

It also keeps two ABIs separable: the *envelope* layout frozen by this ADR, and the *proof-term ordinals* frozen independently in `term.rs`. Either can gain vectors, or a version, without disturbing the other.

## 4. Why the budget is excluded

The resource `Budget` is deliberately **not** part of the material. Two successful checks of the same verifier, profile, context, proposition, and term under two different sufficient budgets identify the **same** proof certificate.

Identity is a property of the artifacts, not of the effort spent checking them. Folding the budget in would mint a fresh certificate every time a caller widened a limit, and would make certificate equality depend on caller configuration rather than on mathematics. The resource contract remains where it belongs — in the verdict, where `ResourceExhausted` maps strictly to `Outcome::Unknown` (ADR-0003 §3).

## 5. Context binding

`ExplicitTerm` carries its own `context: ContextId` field and canon-writes it first, so the context appears twice in a v1 envelope: once as field 5, and once inside field 7's payload.

This repetition is intentional and is **validated, not tolerated**. Because `ExplicitTerm::canon_write` emits the embedded context first as a length-framed digest, a reader can extract the term's own claim about its context with a single nested `read_bytes` — no proof-tree parsing — and require it to equal field 5. An envelope whose term disagrees with its own context header is rejected.

## 6. Validation contract

`decode_material_v1` and `validate_material_v1` in `crates/brix-kernel/src/certificate.rs` are total and **fail closed**. Every rejection returns an error; none constructs a `Certificate`, `Evidence::KernelCertificate`, `Outcome::Proven`, or `Outcome::Refuted`.

| Error | Rejected because |
|---|---|
| `BadMarker` | leading marker is not `brix.kernel.certificate` |
| `UnknownVersion(n)` | version field names a format this build does not implement |
| `UnknownProfile` | profile field is not `brix.kernel.profile@1.2` |
| `VerifierMismatch` | verifier field is not the native verifier |
| `ContextMismatch` | context field differs from the expected context |
| `PropositionMismatch` | proposition payload differs from the expected proposition's canonical bytes |
| `TermMismatch` | term payload differs from the expected term's canonical bytes |
| `TermContextMismatch` | the context the term embeds differs from field 5 |
| `Truncated` | input ended mid-field |
| `BadLength` | a length prefix ran past the buffer, or a digest field was not 32 bytes |
| `NonMinimalInt` | a magnitude carried a non-minimal leading zero |
| `TrailingBytes` | bytes remained after field 7 |

An **unknown version or profile is rejected outright, never best-effort parsed.** A future reader that understands v2 must dispatch on the version field and refuse anything it does not implement; reinterpreting unknown bytes under a known layout is exactly the failure mode this envelope exists to prevent.

The decoder reconstructs a `ContextId` and `VerifierId` from canon-framed digest bytes. These are identities to **compare**, never authority to act on: they name what the envelope claims, and every one of them is checked against something the caller already holds before any identity is returned.

The public kernel acceptance API remains typed (`ContextId`, `Prop`, `ExplicitTerm`). This ADR adds **no** path that accepts arbitrary opaque bytes as a proof.

## 7. Evolution rule (append-only)

> The v1 field list, their order, the marker bytes, the version number, and the profile string are **frozen ABI**. A field may never be inserted, removed, reordered, or reinterpreted.
>
> Any change to what a certificate is bound to requires a **new** format version with its own encoder, its own decoder arm, and its own appended vector cases. The v1 cases in `vectors/kernel_certificate_v1.json` must continue to reproduce byte-for-byte forever, and decoders must reject unknown versions rather than parse them optimistically.
>
> A change to the *contents* of an existing v1 vector is a spec erratum under `spec/errata/`, not a code change.

This mirrors the discipline `brix-canon` applies to `CANON_VERSION` and `vectors/canon_vectors.json`, one level up: `CANON_VERSION` governs how a value becomes bytes, and this ADR governs which values a certificate commits to.

## 8. Evidence

Frozen in `vectors/kernel_certificate_v1.json`, covering the three accepted shapes:

- `identity_implication` — `P -> P` by `\x. x`;
- `realizes_composition` — Profile 1.1 realization composition (ADR-0004);
- `finite_sum_case` — `P + Q -> Q + P` by a total two-arm case split, the coverage shape ADR-0011 builds on.

Each case records the declarative input shapes, the context/proposition/term hex, the full material hex, and the certificate id, so it can be replayed from the manifest alone.

Every case is guarded twice. `kernel_certificate_vectors_are_frozen` re-derives the manifest through the production encoder. `kernel_certificate_vectors_reproduced_by_primitive_canon_writes` rebuilds each envelope through a **second construction path** that spells out the marker, version, profile, verifier preimage, context, and payload framing with primitive `CanonWriter` calls — repeating the frozen literals rather than importing the constants, so a typo'd constant cannot agree with itself — and never calls the production encoder. This follows the independent-reproduction idiom already sanctioned for SOC-LAW-01 by `ContextId`'s `golden_vector_root_extend_reproduced_independently`.

## 9. Consequences and non-goals

**Resolved.** C-1 (Debug-derived identity), C-2 (no certificate vectors), and — for the acceptance boundary — D-1 (decoder/encoder contract).

**Still open, deliberately.** There is no durable on-disk `ExplicitTerm` artifact format and no general recursive decoder for arbitrary proof artifacts; those remain with #56/#58, and SOC-LAW-01 and SOC-LAW-12 stay `Partial` because of them, not because of certificate identity.

**Out of scope.** No new calculus constructs, normalization, conversion, proof search, or tactics. No change to `CANON_VERSION`, to existing canonical ordinals, or to any existing frozen vector. No change to ADR-0011 match/coverage semantics, to the authority-publication API, or to the separation of the proof and settlement kernels.
