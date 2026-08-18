# OWNER — SOC settlement core

**Lane:** soc
**Crates:** soc-core
**Spec requirements:** ADR-0002 (SOC Constitution — §1, §4, §5, §8 ⟨D-FO⟩, §9); ADR-0014 (divergence-sensitive saturation); bounded against ADR-0012 §7
**Conformance:** `vectors/soc_quiescence_v1.json` and `vectors/soc_divergence_v1.json`; SOC-LAW-08 and SOC-LAW-10

## Contract
The settlement kernel: the committed coalgebra `γ = select_K ∘ δ` into
`D_O = 1 + O×X` with `O = O_min` (ADR-0002 §8.3, ⟨D-FO⟩ **ratified**), an
append-only journal with deterministic replay, the audit-factorization checker as
the sole authority for `Outcome::Audited`, and — layered strictly *above* `γ` —
divergence-sensitive saturation with quiescence and divergence certificates.

`F_O`, `O_min`, `select_K`, the calendar key `K`, `Committed`, `Observation`, and
the `CommittedStep`/`Journal` encoding are **frozen ABI**. Saturation may not
change them; it did not.

## Frozen artifacts
`vectors/soc_quiescence_v1.json` and `vectors/soc_divergence_v1.json`, pinning the
ADR-0014 §6.2 certificate field lists (⟨D-QCERT⟩) and the v1 observation-profile
preimage (⟨D-OBS⟩). Append-only after the freeze: an existing case may never
change without a **new envelope format version**.

Note one live constraint the freeze carries. The v1 quiescence certificate can
only ever assert a *complete* enumeration, because `EnumerationCompleteness`
admits exactly one ordinal and the reader accepts exactly that one. That is sound
only while `WitnessProvider::candidates -> Vec<Candidate>` stays unbounded and total. If
ADR-0012 §4's contemplated bounded or fallible regime API ever lands, it **must**
mint a v2 certificate and **must not** emit v1.

Guarded by a frozen manifest test plus an independent second construction path
built from primitive `CanonWriter` calls that repeats the frozen literals rather
than importing the constants (`tests/saturation_vectors.rs`). Regenerate
deliberately with `BLESS_VECTORS=1` and review the hex diff by hand.

## Discipline
Depends on `brix-canon` and `brix-semantic` only (ADR-0002 §3 substrate
discipline), enforced by `scripts/check_tcb_dependencies.py` — `brix_kernel::Budget`
is deliberately unavailable, which is why `SaturationBudget` is local. No
`HashMap`/`HashSet` in semantic paths: determinism is a release gate, so
`BTreeMap`/`BTreeSet` throughout. `unsafe` denied.

Fail closed, always. A search that has not terminated has proved nothing
(ADR-0002 §5.3): budget exhaustion, an undeclared hypothesis, and a falsified
declared hypothesis are each an explicit `Unknown` that certifies nothing, and
none of them is ever `Refuted`. Exactly one constructor in this crate yields a
decided negative — `SaturatedStep::Quiescent`, and only via a certificate a
checker can independently re-derive.

Never compare journals or chain digests to decide behavioral agreement: weakly
bisimilar systems have different journals by design (ADR-0014 risk 3).

Ambiguities become errata in `spec/errata/` or a new ADR, never guesses. See
CONTRIBUTING.md for the feedback protocol.
