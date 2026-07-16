# OWNER — brixc + cli + pkg

**Lane:** brixc + cli + pkg
**Crates:** brixc, brix-cli, brixpkg
**Spec requirements:** Part XXVIII (two-pass compilation); Part II §6 (ceremony); Part XIII (distribution)
**Conformance:** generated code for the flagship is an insta snapshot; `brix run` cache hit < 100 ms

## Contract
brixc: ast -> ir -> phase -> plan -> emit; codegen via quote + prettyplease into a generated cargo workspace, one module per relation/rule, determinism `#[deny]` headers. brix-cli: new/build/run/repl/test/sim/fmt/why/whynot/explain. brixpkg: TOML manifest, lockfile with exact digests, pubgrub resolution, content-addressed local registry.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
