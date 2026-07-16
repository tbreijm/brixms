# OWNER — canon + diag

**Lane:** canon + diag
**Crates:** brix-canon, brix-diag
**Spec requirements:** Appendix G (canonical encoding); Part III §3 (identity); diagnostic BRX codes throughout
**Conformance:** canon golden vectors are the G0 artifact; every later crate reports through brix-diag

## Contract
Freeze brix-canon's App. G encoding into insta golden `vectors/`, cross-checked by an independent implementation, then append-only. brix-diag exposes `Diagnostic { code, severity, site, message, structure }` with stable BRX ranges, miette + JSON + SARIF.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
