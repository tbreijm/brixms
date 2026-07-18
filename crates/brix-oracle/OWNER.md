# OWNER — oracle

**Lane:** oracle
**Crates:** brix-oracle, brix-conformance
**Spec requirements:** Part III (kernel semantics); Part I (flagship); Appendix I (conformance)
**Conformance:** G1 reached (issue #24, `crates/brix-oracle/tests/flagship.rs`): the
flagship parses, checks (via `brixc::lower_file`), and runs end-to-end on the oracle
through `frontend::program_from_source`, with `why` answering from oracle provenance.
**The oracle is now frozen — changes only through `spec/errata/`.**

## Contract
The semantic authority; design goal is BORING. Single-threaded; extents as BTreeMap<CanonBytes, Row>; full fixpoint phase by phase per revision; masks, key conflicts, error edges, constraints, snapshot-isolated transactions, naive protocol lifecycle, sim clock as state. brix-conformance: differential harness (oracle vs engine, canon bytes bit-for-bit) starts here.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
