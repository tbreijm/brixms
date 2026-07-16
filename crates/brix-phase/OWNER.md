# OWNER — phase

**Lane:** phase
**Crates:** brix-phase
**Spec requirements:** Appendix F (phase inference); Part III §§5-6 (phases, masks)
**Conformance:** phase assignment invariant under rule declaration order (property test)

## Contract
Direct App. F transcription over petgraph: positive/strict/mask edges, SCC condensation, phase assignment, and minimal offending path extraction for cycle errors emitted as diag structure.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
