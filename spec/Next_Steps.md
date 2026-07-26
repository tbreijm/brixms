# Next Steps — SOC transition

**The immediate 3–5 actions, in order.** Each has one acceptance criterion. Full
plan: [Build_Plan_v3_SOC.md](./Build_Plan_v3_SOC.md). Constitution:
[ADR-0002](./adr/ADR-0002_SOC_Constitution.md).

---

### 1. Ratify ADR-0002, including ⟨D-AUD⟩ (S0) — ✅ RATIFIED 2026-07-25

Review and accept `spec/adr/ADR-0002_SOC_Constitution.md`. Confirm that the
carried-forward ADR-0001 §§4–5–7 decisions (outcome ordinals, authority table,
`ContextId` root-digest invariant) are unchanged, and ratify the one
append-only lattice extension: **`Audited`** (ordinal 5, sole authority = the
audit-factorization checker / reference replayer; the typed precondition for
`elaboration-boundary` edges — ADR §4 ⟨D-AUD⟩). On ratification, `outcome.rs`
gains the sixth member (append-only; existing golden vectors unchanged).

**Acceptance:** ADR-0002 status flips **Proposed → Accepted** ✅; ADR-0001 stays
marked *Superseded-in-part*; `Audited` lands in `outcome.rs` with its authority
row, an explicit lattice partial-order function (not derive order), and golden
vectors for ordinal 5.

### 2. Resolve the behavior-signature decision ⟨D-FO⟩ (S0) — ✅ RATIFIED 2026-07-25

Ratify `(O, F_O)`. **Ratified on the presented default:** `F_O = D_O = 1 + O×X` (partial
deterministic committed behavior), `O = O_min` (settlement-event tags + committed
`JudgementId` digest), deliberation in `B^uk_{K,O}`. See ADR §8.

**Acceptance:** `(O, F_O)` chosen ✅ and version-tagged (like the canon vectors);
recorded in ADR §8. **No `soc-core` encoder is frozen before this.**

### 3. Extend `brix-semantic` with the SOC artifacts (S1)

Add `Witness`, `RegimeId`, the generator registry `𝒢`, `Decomposition` evidence,
and the `Realizes(w,x,y)` proposition kind — content-addressed, versioned
encoders, `brix-canon`-only. (Issue-disposition new-issue #1/#3 track this.)

**Acceptance:** golden vectors for every new artifact; malformed-artifact
rejection; **the `ContextId` root-digest invariant golden vector is green**
(`ScopeId::root()` parity preserved); retraction-closure fixtures still green.

### 4. Stand up `soc-core` skeleton + the naive oracle (E1 → S2/E2)

New crate: interner + persistent store + chained history digest (E1); then the
realization interface, `Adm`, and `cand(e)`/`Succ(e)` as the single-threaded
naive reference oracle (S2/E2). Correct, not fast.

**Acceptance:** the **governance-conservation law** runs as an executable
conformance property — tightening `Adm` shrinks `cand(e)` pointwise for every
reachable state.

### 5. Wire the O(Δ) gate harness early (E5, scaffold now)

Stand the O(Δ) benchmark harness up against the naive oracle **before** building
the fast incremental engine, so the invariant is measurable from the first
delta-driven candidate. Instrument via ADR stage-4a cost records.

**Acceptance:** a benchmark that **doubles inert configurations and asserts
per-step cost is unchanged** exists and runs in CI (red on regression), even if
the only engine behind it is still the oracle.
