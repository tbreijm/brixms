# OWNER — ir

**Lane:** ir
**Crates:** brix-ir
**Spec requirements:** Appendix E (static semantics); Part III §§5-9; Part IV §4 (aggregates); Part XII §5 (authority)
**Conformance:** typecheck + effect-row + phase-site assignment feed the oracle and codegen

## Contract
Name resolution, HM-style inference with rows, minimal-coherent trait solving, effect-row inference, purity/determinism checks, Canonical-in-key checking, pattern read-set analysis, stable SiteIds. Core IR is a small closed typed node set; `Display` is a deliverable.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
