# OWNER — rt + ABI

**Lane:** rt + ABI
**Crates:** brix-rt, sdk/driver-wit, sdk/brix-driver-rs
**Spec requirements:** Part III §§2-4,11; Part VII (protocols/Drivers); Appendix H (lifecycle); Part XXVIII (runtime)
**Conformance:** engine == oracle under sustained fuzz (G2)

## Contract
GraphCore (node interner + arenas + global incidence index), RelationStore trait view, revision log (canon-encoded, append-only, mmap), MVCC, settle scheduler, support counting, provenance store, KeyConflict service, transaction pipeline, protocol lifecycle engine, sim clock. The delta ABI = one Rust trait + one WIT world from a single definition (coordinate the design).

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
