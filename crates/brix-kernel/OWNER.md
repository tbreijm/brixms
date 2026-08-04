# OWNER — proof kernel

**Lane:** kernel
**Crates:** brix-kernel
**Spec requirements:** ADR-0003 (dependent calculus), extended by ADR-0004 (Profile 1.1) and ADR-0006 (Profile 1.2); ADR-0013 (canonical certificate envelope)
**Conformance:** `vectors/kernel_certificate_v1.json` is the frozen certificate-identity artifact; SOC-LAW-01 and SOC-LAW-12

## Contract
Accept or reject proof terms against propositions in an explicit context, under a
declared budget. A `Verdict::Accepted` carries a `Certificate` whose identity is
the `Value`-domain digest of the pinned ADR-0013 v1 envelope — never a `Debug`
rendering, never a format string. The envelope's marker, format version, profile
string, and field order are frozen ABI (ADR-0013 §7).

## Frozen artifacts
`vectors/kernel_certificate_v1.json`. Append-only after the freeze: an existing
case may never change without a **new envelope format version**. A layout change
takes a new version number and leaves v1 untouched; readers reject unknown
versions outright rather than best-effort parsing them.

Two consumers guard it — the frozen manifest test, and an independent second
construction path that spells out every field with primitive `CanonWriter` calls
and repeats the frozen literals rather than importing the constants
(`tests/certificate_vectors.rs`). Regenerate deliberately with `BLESS_VECTORS=1`
and review the hex diff by hand.

## Discipline
Depends on `brix-canon` and `brix-semantic` only — the trusted-boundary policy in
`scripts/check_tcb_dependencies.py` enforces this, and widening it is an ADR
decision, not a Cargo edit. Serialize only through `brix-canon`. No `HashMap`/
`HashSet` in semantic paths. `unsafe` denied. Decoding and validation fail closed:
no certificate, evidence, or `Proven`/`Refuted` outcome may be constructed from
bytes that failed to decode. Ambiguities become errata in `spec/errata/`, never
guesses. See CONTRIBUTING.md for the feedback protocol.
