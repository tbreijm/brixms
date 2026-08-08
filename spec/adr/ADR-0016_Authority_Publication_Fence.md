# ADR-0016 — The Authority Publication Fence: Legal Publisher Routes and the Audited-Source Boundary

Status: **Proposed** (2026-08-08) (extends [ADR-0002](./ADR-0002_SOC_Constitution.md) §4.1 with an evidence-kind column and makes it executable; discharges audit findings A-1/A-2 from `docs/audit/issue-63/README.md`; governs `crates/brix-semantic/src/publication.rs` and every `Judgement` construction site in the workspace).

Date: 2026-08-08.

Foundation documents: [ADR-0001: The Shared Proof Substrate](./ADR-0001_Proof_Substrate.md) (§4 the outcome lattice, §4.1 the original verifier-authority table, §5.3 evidence, §5.4 judgement identity), [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4 ⟨D-AUD⟩, §4.1 the frozen authority table, §5 ¶1–2 the `Derived → Audited → Proven` upgrade path and the elaboration boundary, §5.1/§5.2 recorded-vs-replay-verified, §6 the `Decomposition` artifact), [ADR-0003: Proof Kernel Profile](./ADR-0003_Proof_Kernel_Profile.md) (§ verdict vocabulary — `Accepted` is the sole source of `Proven`), [ADR-0012: Brix L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (§6.5 no grade laundering), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§6 the fail-closed decoder contract), [ADR-0014: Divergence-Sensitive Saturation](./ADR-0014_Divergence_Sensitive_Saturation.md) (§5.1 the uniform grading rule), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-PRIM⟩ — the other side of this boundary, see §0), and the Phase A trusted-boundary audit `docs/audit/issue-63/README.md`.

This ADR adds no proof rules, no calculus, no language semantics, and no CLI surface. It changes **who may construct a `Judgement` with which outcome**, and nothing about what a judgement *is*. Every canonical encoding named in §8 is untouched.

---

## 0. Relationship to ADR-0015 — the same boundary from two sides

This ADR and [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md) were drafted
concurrently on separate branches and took the same number by accident; per
convention the first-merged keeps it, so this one is 0016. The collision is
worth more than an editorial note, because the two turned out to interlock.

They approach the *same* boundary from opposite sides:

- **ADR-0015 ⟨D-PRIM⟩ asks what a leaf may claim.** It gives the kernel a
  compiled-in, judgment-scoped primitive-relation registry and a zero-premise
  introduction term, so a primitive realization leaf becomes a
  **kernel-checked fact** instead of an assumption discharged by a prose
  correspondence argument.
- **This ADR asks who may publish the resulting judgement**, and on what
  evidence, once the derivation over those leaves exists.

Neither subsumes the other. A kernel-checked leaf still needs a publisher whose
authority matches the outcome it claims; a fenced publisher still needs its
leaves to mean something. Together they close the gap from "this leaf is
assumed" to "this judgement is published by the one authority entitled to
publish it, on evidence that supports it."

**One consequence runs the wrong way and should be read carefully.** ADR-0015
reasons about whether `Proven` is honest for arithmetic on the premise that a
typing result is safely *capped at `Audited`*. §7 of this ADR reports that on
the typing path — `soc-regimes::audited_type_check_tree`, the function behind
every `brix check`, `brix why`, and `brix prove` typing result — that `Audited`
currently rests on evidence computed from the claim itself. The cap is real,
but what sits under it is weaker than ADR-0015's premise assumes. That is a
statement about the finding in §7, not a defect in ADR-0015's reasoning, and it
resolves when issue #259 does.

The system rests on one sentence:

> `Derived` comes only from the settlement hot loop, `Audited` only from the audit-factorization checker, and `Proven` only from a kernel `Accepted` verdict. There is no path upward without evidence.

ADR-0002 §4.1 froze that as a table in prose. `brix-semantic` encodes half of it as data — `Outcome::authority()` is a total `const fn` mapping each of the six outcomes to its sole `Authority` — and the module doc claims this is "data, checkable, not a review-time convention."

It is data. It is not checked. **No publish site in the workspace calls it to reject a construction.** The one place it appears at a publish site is `crates/soc-core/src/audit.rs`, as a `debug_assert_eq!(Outcome::Audited.authority(), Authority::AuditChecker)` — a tautology over the constant table, not a check on the value being built.

What the Phase A audit found, and what the source still shows:

- **A-1.** `Judgement`'s four fields are `pub` and `Judgement::new` accepts every `Outcome`/`EvidenceId`
  combination. Because the fields are public, a caller does not even need the constructor: a struct
  literal from any crate works.
- **A-2.** `brix_elaborate::elaborate_and_publish` invokes the kernel but takes an arbitrary
  `&Judgement` as its source. It never checks `source.outcome == Audited`, and never checks that the
  support behind it was replay-verified — although ADR-0002 §5 ¶2 is explicit: *"only
  `Audited`-supported settlement evidence may enter an `elaboration-boundary` edge — an unaudited
  commit is not even a certified rule-match chain, let alone a theorem."*

There is a structural reason the gap was easy to miss, and it dictates the shape of the fix:

> **A `Judgement` carries an `EvidenceId` — a digest — not the `Evidence`.**

So no check applied to a finished `Judgement` can recover what supports it. A digest is a digest. The
fence therefore cannot be a validator over judgements; it has to sit **at construction**, and it has
to be handed the supporting *artifact*, not its id.

## 2. Decision

Four decisions, all in `brix-semantic` — the crate that already owns `Outcome`, `Authority`,
`Evidence`, and `Decomposition`, and that is already inside all four TCB closures, so none of this
widens a dependency boundary.

**D1 — One authoritative route table.** A single `const ROUTES` enumerates every legal
(authority, outcome, evidence-kind) triple, with the extra conditions each route demands. This is
the thing the code consults. ADR-0002 §4.1's rows are its authority-and-outcome columns; §4 below
adds the evidence-kind column that table never had.

**D2 — Publication is a fallible operation.** `Judgement::publish` is the sole door through which a
crate outside `brix-semantic` can obtain a `Judgement` value. It takes an explicit `Authority` claim
and a `Support` — the artifact, not a digest — consults `ROUTES`, and returns
`Result<Judgement, PublicationError>`. A mismatched outcome/evidence pairing is a construction
error, not a runtime surprise.

**D3 — The struct is sealed.** `Judgement` becomes `#[non_exhaustive]` and `Judgement::new` becomes
`pub(crate)`. Outside the crate, struct literals no longer compile; field *reads* are unaffected.
Without this, D1 and D2 are documentation.

**D4 — The elaboration boundary validates its source.** An `AuditedSource` is a witness that a
judgement really is `Audited` *and* that a presented artifact really is the support named by its
evidence id. `elaborate_and_publish` takes one. You cannot call it without having verified.

### 2.1 What is deliberately *not* decided

`Judgement`'s four `pub` fields stay readable, and its canonical encoding, field order, and
`JudgementId` are byte-identical. The fence is about construction. Everything downstream that reads
`j.outcome` or `j.evidence` compiles unchanged.

## 3. Publication is not identity computation

A checker frequently needs the *id* of a judgement it is validating and has no authority to publish.
`soc-core::audit::audit_step` re-derives the `Derived` judgement's id to compare it against the
recorded `Observation` — it is auditing that judgement, not minting it. The quiescence certificate
re-derives its own claim id from the presentation.

Forcing those through `publish` would be wrong twice: it would make the audit checker claim the
settlement kernel's authority, and it would fail, because the checker holds a digest and not the
artifact.

So there are two doors, and the distinction is normative:

| Door | Yields | Claims authority | For |
|---|---|---|---|
| `Judgement::publish(authority, …, support)` | a `Judgement` **value** | yes — checked against `ROUTES` | publishers |
| `JudgementId::recompute(context, proposition, outcome, evidence)` | a `JudgementId` | **no** | checkers re-deriving a claim's identity |

`recompute` mints an identity, never a judgement. This is not a hole: a `JudgementId` is a blake3
digest over four canonical fields, and anyone able to run the hash function can compute one. What
authority attaches to is *holding a `Judgement` value*, and — for the settlement route specifically
— the journal `Observation` whose integrity `audit_step` checks. Naming the identity-only door
explicitly is what keeps `publish` from being watered down to accommodate checkers.

## 4. Publication routes (extends ADR-0002 §4.1)

The `authority` and `outcome` columns are ADR-0002 §4.1 verbatim. The rest is new.

| Authority | Outcome | Evidence kind (`SupportKind`) | Extra condition | Status |
|---|---|---|---|---|
| `ProofKernel` | `Proven` | `KernelCertificate` | — | Settled |
| `ProofKernel` | `Refuted` | `KernelRefutation` | — | Settled |
| `SettlementKernel` | `Derived` | `Settlement` | the `Decomposition` is `Recorded` | Settled |
| `AuditChecker` | `Audited` | `Settlement` | the `Decomposition` is `ReplayVerified` | Settled |
| `AuditChecker` | `Audited` | `TreeRealization` | — | **Provisional** (§7) |
| `ExternalDriver` | `Measured` | `Measurement` | — | Settled |
| `ExternalDriver` | `Measured` | `ExternalResult` | — | Settled |
| `AnyResolver` | `Unknown` | any | — | Settled |

**Every triple absent from this table is illegal.** The table is total in the direction that matters:
each `Outcome` has at least one route, and each route's authority is exactly `outcome.authority()`,
so `ROUTES` cannot silently disagree with ADR-0002 §4.1. Both properties are asserted by test.

Three rows deserve their reasoning stated, because each closes a specific way the old surface could
be abused.

**`Proven` demands `KernelCertificate`; `Refuted` demands `KernelRefutation`.** The poles are not
interchangeable, and neither may be supported by a settlement replay. This is ADR-0003's verdict
vocabulary made structural: an `Accepted` verdict is the only thing that produces the certificate,
and the certificate is the only thing that opens the `Proven` route. A regime may construct a
candidate term; it cannot assemble the outcome.

**`Derived` demands a `Recorded` decomposition.** This is not new policy — ADR-0002 §5.1 says the hot
loop records "a compact support record plus the (unverified) `Decomposition`", and two existing
tests already depend on it: `soc-core/tests/audit_factorization.rs`'s
`non_recorded_decomposition_is_rejected` and `brix-lower/src/l3_audit.rs`'s
`tampered_decomposition_yields_unknown` both require `audit_step` to fail closed when a step arrives
carrying an already-`ReplayVerified` chain. Making it a route condition promotes an invariant the
audit checker enforced downstream into one the publisher cannot violate upstream.

**`Audited` demands a `ReplayVerified` decomposition.** ADR-0002 §4.1: the hot loop "may *record*
decompositions, never assert their verification." The two forms already have different
`DecompositionId`s by construction (ADR-0002 §5.1/§5.2, `decomposition.rs`); this makes the
distinction load-bearing at the publication point rather than only in the artifact's identity.

`Unknown` accepts any support, deliberately. ADR-0001 §4 and ADR-0014 §5.1 both put the discipline on
the *other* side: anyone may fail closed to bottom, and no one may downgrade a stronger outcome to
hide a failure. A fence on `Unknown` would obstruct honest failure without preventing any escalation.

## 5. The `Support` and `PublicationError` contract

`Support` is `Evidence` with the bodies replaced by the artifacts they digest, plus a borrow where
the artifact is inspectable:

```rust
pub enum Support<'a> {
    KernelCertificate { verifier: VerifierId, certificate: CertificateId },
    KernelRefutation  { verifier: VerifierId, certificate: CertificateId },
    Settlement(&'a Decomposition),
    Ground { body: Digest },
    Measurement { body: Digest },
    ExternalResult { body: Digest },
    Suggestion { body: Digest },
    TreeRealization { body: Digest },   // provisional — §7
}
```

`Support::evidence()` is the total projection back into `Evidence`; `Support::kind()` is the `Copy`
tag `ROUTES` matches on. `Settlement(d)` and `TreeRealization { body }` both project to
`Evidence::SettlementReplay` — they are the same evidence *variant* distinguished by what stands
behind it, which is exactly the distinction the old `EvidenceId`-only surface could not express and
the reason §7's finding was invisible.

**Fail-closed rule (normative).** Every rejection returns a `PublicationError` and constructs
nothing. A refused publication is never a downgraded outcome, never `Unknown`, and never `Refuted` —
this ADR does not add a new way to reach a decided negative, and ADR-0014 §5.1's uniform grading rule
is untouched: `SaturatedStep::Quiescent` remains the sole decided negative in `soc-core`.

Refusals are enumerated:

| Variant | Raised when |
|---|---|
| `WrongAuthority { outcome, claimed, sole }` | the claimed authority is not `outcome.authority()` |
| `UnsupportedEvidence { authority, outcome, support }` | no route pairs that evidence kind with that outcome |
| `DecompositionVerificationMismatch { outcome, found }` | `Derived` from a `ReplayVerified` chain, or `Audited` from a `Recorded` one |
| `EvidenceBindingMismatch { expected, found }` | §6: the presented artifact is not what the judgement's evidence id names |
| `NotAudited { found }` | §6: an elaboration source whose outcome is not `Audited` |

Convention follows the workspace: a plain `enum`, `#[derive(Clone, Debug, PartialEq, Eq)]`, no
`Display`, no `thiserror` (the workspace has no such dependency).

## 6. The audited-source boundary (finding A-2)

```rust
pub struct AuditedSource { /* private */ }
impl AuditedSource {
    pub fn verify(judgement: &Judgement, support: Support<'_>) -> Result<Self, PublicationError>;
    pub fn judgement(&self) -> &Judgement;
}
```

`verify` checks three things, in order, and fails closed on each:

1. `judgement.outcome == Outcome::Audited`, else `NotAudited`.
2. `(AuditChecker, Audited, support.kind())` is a route in §4, and its extra condition holds — so
   `Settlement(d)` requires `d.is_replay_verified()`.
3. **The binding.** `support.evidence().id() == judgement.evidence`, else `EvidenceBindingMismatch`.

Step 3 is what makes the source non-forgeable rather than merely well-typed. Without it a caller
could hand over a genuine `Audited` judgement alongside an unrelated verified decomposition and
elaborate the wrong claim. With it, the judgement's evidence digest must actually *be* the digest of
the artifact presented, so an `AuditedSource` is a proof that the chain it names is the chain it was
audited on — under the collision-resistance assumption that already carries every content-addressed
identity in the substrate.

`brix-elaborate` consumes it:

- `elaborate_and_publish(source: &AuditedSource, …)` — the boundary is in the signature.
- `elaborate_decomposition` and `elaborate_tree` run `verify` themselves and return
  `ElaborationResult::Refused(PublicationError)` when it fails. `Refused` is a third variant, kept
  distinct from `NotElaborated(Verdict)`: a kernel that rejects a term and a caller that never had
  standing to ask are different facts, and collapsing them would lose exactly the signal this ADR
  exists to produce.

The canonical upgrade path of ADR-0002 §5 ¶2 — `Derived → Audited → Proven`, engine commit, oracle
replay, kernel elaboration — is now typed at each joint rather than asserted in prose.

## 7. Erratum: the tree-realization `Audited` support is unsupported

**This section records a finding. It does not bless it.**

`crates/soc-regimes/src/type_realization.rs`'s `audited_type_check_tree` publishes an `Audited`
judgement whose evidence body is:

```rust
Evidence::SettlementReplay {
    body: brix_canon::Digest::of(brix_canon::Domain::Value, prop.digest().as_bytes()),
}
```

The body is a digest **of the proposition being claimed**. No `Decomposition` stands behind it,
replay-verified or otherwise; the function returns a `RealizesTree`. So the evidence is a function of
the claim: it is satisfiable by anything that can state the proposition, and it distinguishes nothing.
Under §4's `Audited` row as written for settlement support, it is unsupportable.

Per issue #228's instruction — *"if the work reveals that a currently-legal path is actually unsound,
stop and report it rather than quietly fixing it"* — this ADR reports it and does not fix it. The
route exists in `ROUTES` as `SupportKind::TreeRealization`, marked `RouteStatus::Provisional`, so the
path keeps compiling and behaves exactly as it does today. What changes is that the hole is now
**named in the table** instead of being invisible inside a general-purpose constructor.

Resolving it means giving tree realization a real replay-verified support artifact, which is
`soc-regimes` Track 2 / ADR-0007 territory and requires coordination that #228 explicitly excludes.
Tracked by `spec/errata/0004-tree-realization-audited-support.md` and issue #259.

**Resolved by [ADR-0017](./ADR-0017_Tree_Realization_Support.md).** The ruling is reading 2:
the outcome was right and the support was not. The `Provisional` row is retired and replaced
by a `Settled` route conditioned on a checked `TreeDerivation` artifact, so **no
`RouteStatus::Provisional` route remains in `ROUTES`**. The status is kept in the type for
the next such hole, and `scripts/check_soc_law_map.py` now couples its presence to
SOC-LAW-05 staying `partial` (ADR-0017 §8) — which is the answer to the "loud placeholder"
question this section raised.

### 7.1 A second residual, stated rather than overclaimed

`Decomposition::replay_verified` does not replay anything. It stamps the `ReplayVerified` tag on
whatever generators and configurations it is given. The `Audited` fence therefore bottoms out at
*whoever called that constructor*, and there are two callers:

- `soc-core::audit::audit_step`, which performs the real work — endpoint checks against the committed
  step, log-integrity comparison against the recorded `Observation`, and a
  `GeneratorSemantics::realizes` call for every link in the chain — before it stamps;
- `soc-regimes::audited_type_check`, which performs none of it and stamps a type-inference derivation
  directly. **Removed by [ADR-0018](./ADR-0018_Retire_The_Flat_Typing_Lane.md) (#262)**, so
  `audit_step` is now the *only* caller — which is what this residual assumed all along. The
  residual itself stands: the constructor is still unchecked, and a future caller could stamp an
  unearned tag.

This ADR does not narrow that constructor. Doing so is audit finding A-3, which #228 routes to #178 /
Track 2 coordination. The fence as built guarantees that an `Audited` judgement is *accompanied by a
chain someone tagged verified and bound to its evidence id* — not that the tagging was earned. That
is a strictly stronger guarantee than exists today and a strictly weaker one than the prose implies,
and the difference is worth writing down rather than smoothing over.

**Ruled by [ADR-0019](./ADR-0019_Verification_Tags_Are_Earned.md).** Two corrections to this
section as written. First, the residual is not only that the constructor is unchecked: every
`Decomposition` field is `pub`, so the tag can be selected by direct assignment without calling
any constructor at all — sealing the constructor alone would not have closed it. Second,
ADR-0017 reproduced the pattern for `TreeDerivation::structure_verified` rather than narrowing
it, so by the time this section was written there were two unchecked verified-tag constructors,
not one.

ADR-0019 rules that a verified tag is an **output of a checked transition, never a constructor
input**: both raw verified constructors are removed in favour of `Decomposition::verify_replay`
and `TreeDerivation::verify_structure`, which perform their defining checks in the crate that
owns the artifact. The tag-minting residual is thereby closed. What remains open is narrower and
is restated in ADR-0019 §6: a caller still supplies the `GeneratorSemantics`, and the verified
id does not name the semantics or registry it was verified against.

## 8. Non-goals

- **The frozen ABI is untouched.** `Committed`, `Observation`, `CommittedStep`/`Journal` encoding,
  `F_O`, the certificate envelopes of ADR-0013, and the `Outcome`/`Evidence`/`DecompVerification`
  canonical ordinals are all out of scope. `Judgement::canon_write` keeps its field order, so every
  legal `JudgementId` is byte-identical before and after — asserted by a test comparing
  `publish(…)?.id()` against `JudgementId::recompute(…)` over the same four fields, and by the frozen
  vectors under `vectors/`, which are **not** re-blessed.
- **No calculus change.** No proof rules, no sums/matches/coverage, no ADR-0011 Track 2 semantics.
  The three `soc-regimes` publish sites are migrated mechanically; their behavior is identical.
- **The two-kernel split is preserved.** Everything new lives in `brix-semantic`, already inside all
  four TCB closures of `docs/audit/issue-63/README.md`. `scripts/check_tcb_dependencies.py` stays
  green with `RULES` unwidened.
- **No new decided negative.** §5's fail-closed rule adds refusals, not verdicts.
- **`Refuted` and `Measured` remain unconstructed.** Neither has a production publisher anywhere in
  the workspace today. Their routes are declared and adversarially tested; this ADR does not create
  the producers.

## 9. Consequences

`Outcome::authority()`'s module-doc claim — "exactly one named producer may publish each outcome;
this is data, checkable, not a review-time convention" — becomes true. `SOC-LAW-05` (authority
non-escalation), held at `partial` with `open_issues: [228]` and a failure mode reading "reject
unrecognized publishers and mismatched outcome/evidence authority without constructing a judgement",
gains an executable gate.

The cost is that adding a legitimate new publisher is now a deliberate act: a new row in §4 and in
`ROUTES`, with a reviewed reason. That is the intent. An over-helpful refactor can no longer mint a
`Judgement` whose outcome its evidence does not support, because it cannot construct one at all.
