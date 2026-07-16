# OWNER — oracle

**Lane:** oracle
**Crates:** brix-oracle, brix-conformance
**Spec requirements:** Part III (kernel semantics); Part I (flagship); Appendix I (conformance)
**Conformance:** G1: flagship parses/checks/runs end-to-end on the oracle; oracle then frozen

## Contract
The semantic authority; design goal is BORING. Single-threaded; extents as BTreeMap<CanonBytes, Row>; full fixpoint phase by phase per revision; masks, key conflicts, error edges, constraints, snapshot-isolated transactions, naive protocol lifecycle, sim clock as state. brix-conformance: differential harness (oracle vs engine, canon bytes bit-for-bit) starts here.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
