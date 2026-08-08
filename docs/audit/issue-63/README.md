# Issue #63 Phase A — current TCB and authority-boundary audit

This is an evidence-first snapshot of the nine-crate workspace at the commit
that contains it. It deliberately does not change calculus, semantic APIs, or
the active sum/match/coverage work.

## Reproducing the dependency evidence

```sh
python3 scripts/check_tcb_dependencies.py --write
python3 scripts/check_tcb_dependencies.py --check
scripts/test_tcb_dependency_gate.sh
```

The generator consumes `cargo metadata --format-version 1 --no-deps`, sorts all
output, and writes the reviewable [DOT graph](workspace-dependencies.dot) plus
the machine-readable [edge and closure inventory](workspace-dependencies.json).
The closure includes production external crates because they are part of the
actual trusted closure, not merely workspace edges.

## Current TCB closures

| Boundary | Workspace closure | Full production closure |
| --- | --- | --- |
| Canonical identity/encoding | `brix-semantic`, `brix-canon` | `brix-semantic`, `brix-canon`, `blake3`, `indexmap`, `unicode-normalization` |
| Settlement commitment | `soc-core`, `brix-semantic`, `brix-canon` | plus `blake3`, `indexmap`, `unicode-normalization` |
| Audit verification | `soc-core::audit`, `brix-semantic`, `brix-canon` | plus `blake3`, `indexmap`, `unicode-normalization` |
| Proof acceptance | `brix-kernel`, `brix-semantic`, `brix-canon` | plus `blake3`, `indexmap`, `unicode-normalization` |

The manifests below define the boundary in more detail. `soc-regimes`,
`brix-elaborate`, `brix-syntax`, `brix-lower`, and `brix-cli` are outside both
kernels.

## Authority-construction inventory

| Route | Code path | Outcome/evidence | Enforcement today |
| --- | --- | --- | --- |
| Universal substrate constructor | `brix-semantic/src/judgement.rs:Judgement::new` | any `Outcome` with any `EvidenceId` | Convention only; public constructor has no authority/evidence pairing validation. |
| Settlement commit | `soc-core/src/commit.rs:commit_tick` | `Derived`, `SettlementReplay` of recorded `Decomposition` | Runtime construction and deterministic journal checks; authority ownership is convention plus module boundary. |
| Audit publication | `soc-core/src/audit.rs:audit_step` | `Audited`, `SettlementReplay` of replay-verified `Decomposition` | Runtime replay, registry, endpoint, and journal-integrity validation; outcome authority is a debug assertion plus convention. |
| Proof certificate | `brix-kernel/src/check.rs:acceptance` | `Verdict::Accepted(Certificate)`, verifier `brix.kernel@0.1` | Type/API and runtime proof checking; certificate identity is the pinned canonical v1 envelope (ADR-0013), frozen by `vectors/kernel_certificate_v1.json` (C-1/C-2 resolved). |
| Proof publication | `brix-elaborate/src/lib.rs:elaborate_and_publish` | `Proven`, `KernelCertificate`, `ElaborationBoundary` | Runtime kernel acceptance; source is accepted as any `Judgement`, so the required audited-source boundary is convention only (finding A-2). |
| Realization route (outside kernels) | `soc-regimes/src/type_realization.rs:type_check` | `Derived`, `SettlementReplay` of a recorded `Decomposition` | Convention only at the outcome publication point; active Track 2 surface, not changed here. |
| Realization audit route (outside kernels) | `soc-regimes/src/type_realization.rs:audited_type_check` | `Audited`, `SettlementReplay` of a replay-verified `Decomposition` | Replay-verified decomposition construction is runtime-validated, but publication authority is convention only; active Track 2 surface, not changed here. |
| Tree realization audit route (outside kernels) | `soc-regimes/src/type_realization.rs:audited_type_check_tree` | `Audited`, `SettlementReplay` whose body is derived from the proposition; returns a `RealizesTree`, not a `Decomposition` | Tree well-formedness is runtime-validated, but this direct `Audited` publication is convention only; active Track 2 surface, not changed here. |
| Refutation vocabulary | `brix-semantic::Evidence::KernelRefutation`, `Outcome::Refuted`; `brix-kernel::Verdict::outcome` | `Refuted`/kernel refutation representation | No current native acceptance path creates a refutation certificate; the public substrate constructor can still assemble one by convention. |
| Verifier/certificate identity | `brix-semantic/src/evidence.rs:VerifierId::named`, `brix-kernel/src/certificate.rs` | verifier is canonical name string; certificate is opaque digest | Both are canonically encoded: the certificate digests a pinned envelope over (marker, version, profile, verifier, context, proposition, explicit term). |

## Canonical-boundary inventory and findings

| ID | Classification | Evidence | Disposition |
| --- | --- | --- | --- |
| C-1 | Canonical compatibility risk | `brix-kernel/src/check.rs` forms certificate payload with `format!("{context:?}:{proposition:?}:{term:?}")`; this makes a durable certificate identity depend on Rust `Debug` output. | **Resolved** by [#229](https://github.com/tbreijm/brixms/issues/229) / [ADR-0013](../../../spec/adr/ADR-0013_Canonical_Certificate_Envelope.md): `brix-kernel/src/certificate.rs` pins the v1 envelope and `acceptance` no longer formats identity material. |
| C-2 | Canonical compatibility risk | `brix-canon/tests/vectors.rs` has frozen encoding vectors and a Python cross-check, but there are no frozen vectors for kernel certificate payloads or verifier/certificate pairs. | **Resolved** by [#229](https://github.com/tbreijm/brixms/issues/229): `vectors/kernel_certificate_v1.json` freezes implication, realization-composition, and finite-sum certificate cases, each reproduced by an independent primitive-`CanonWriter` path. |
| A-1 | Authority risk | `Judgement::new` is public and accepts every `Outcome`/`EvidenceId` combination. | **Resolved** by [#228](https://github.com/tbreijm/brixms/issues/228) / [ADR-0016](../../../spec/adr/ADR-0016_Authority_Publication_Fence.md): `Judgement` is `#[non_exhaustive]` and `new` is `pub(crate)`, so `Judgement::publish` is the sole external door; it consults `brix-semantic/src/publication.rs:ROUTES` and takes the supporting *artifact* rather than an `EvidenceId`, and refuses a mismatched pairing with a typed `PublicationError`. |
| A-2 | Authority risk | `elaborate_and_publish` invokes the kernel but takes arbitrary `&Judgement`; it does not validate `source.outcome == Audited` or replay-verified support before emitting an elaboration-boundary edge. | **Resolved** by [#228](https://github.com/tbreijm/brixms/issues/228) / ADR-0016 §6: `elaborate_and_publish` takes an `AuditedSource`, whose `verify` checks `outcome == Audited`, route legality (a settlement support must be replay-verified), **and** that `support.evidence().id() == judgement.evidence` — the binding that makes the source non-forgeable. `elaborate_decomposition`/`elaborate_tree` return `ElaborationResult::Refused` rather than reaching the kernel. |
| A-3 | Authority risk | `soc-regimes/type_realization.rs:type_check` creates `Derived`; `audited_type_check` and `audited_type_check_tree` directly create `Audited` outside `soc-core`. | **Resolved.** All three were first routed through `Judgement::publish` against `ROUTES` (#228 / ADR-0016). [#259](https://github.com/tbreijm/brixms/issues/259) / [ADR-0017](../../../spec/adr/ADR-0017_Tree_Realization_Support.md) then gave `audited_type_check_tree` a checked `TreeDerivation` artifact and the leaf generator-membership check it lacked; [#262](https://github.com/tbreijm/brixms/issues/262) / [ADR-0018](../../../spec/adr/ADR-0018_Retire_The_Flat_Typing_Lane.md) **removed** `type_check` and `audited_type_check` outright — their padded chains misstated their own intermediate configurations, and nothing called them. The general residual that `Decomposition::replay_verified` is an unchecked stamp (ADR-0016 §7.1) stands, but `soc_core::audit::audit_step` is now its only caller. |
| D-1 | Documentation debt | ADR-0003 says proof acceptance takes canonical explicit terms, while the current public `ExplicitTerm` is in-memory and the certificate ID is Debug-derived. | **Resolved in part** by [#229](https://github.com/tbreijm/brixms/issues/229) / ADR-0013 §6: `decode_material_v1`/`validate_material_v1` give a total, fail-closed certificate decoder/encoder contract. A durable on-disk `ExplicitTerm` artifact format remains open under #56/#58. |
| C-3 | Audit result | Trusted production modules contain no `serde`/`serde_json` use and no `SystemTime`, environment, process, or host-path identity input found by the Phase A source scan. | No follow-up proposed; this is a bounded source-level result, not a proof about future dependencies. |
| C-4 | Audit result | `Decomposition::recorded` versus `replay_verified` is a typed, canonical distinction; audit failures return `Unknown` rather than an accepted audit result. | Accepted current design; tests cover the fail-closed audit route. |

`Debug` in diagnostics is not itself a finding. C-1 is limited to its use in
certificate identity material. The scan did not identify a durable-artifact
decoder that fails open; there is currently no general durable certificate
decoder, which is the documentation/API gap captured by C-1/C-2.

## Refactor decisions

Rejected in Phase A: moving regimes/elaboration into a kernel, merging the
kernels, changing sum/match/coverage rules, or making an API redesign without
a reviewed boundary contract. Accepted: this dependency gate and audit
inventory, because they only make already-settled dependency rules observable.

Bounded follow-ups: [#229](https://github.com/tbreijm/brixms/issues/229) landed
C-1/C-2/D-1 as the ADR-0013 canonical certificate envelope (coordinated with
#56), [#228](https://github.com/tbreijm/brixms/issues/228) for A-1/A-2, and
#178 coordination for A-3.
