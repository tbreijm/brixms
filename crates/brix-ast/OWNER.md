# OWNER — ast + fmt

**Lane:** ast + fmt
**Crates:** brix-ast
**Spec requirements:** Appendix D (grammar); Part IV (relations/patterns); Part II §6 (ceremony)
**Conformance:** every ```brix block in the spec is a parse fixture (see tests/fixtures/spec)

## Contract
logos lexer + hand-written recursive-descent parser; error recovery and diagnostic quality are the product. CST with full spans -> AST. `brix fmt` v0 = canonical idempotent pretty-printer (format-then-parse over the whole corpus). Grammar gaps -> errata against App. D.

## Discipline
Serialize semantic data only through `brix-canon`. No `HashMap`/`HashSet` in
semantic paths (clippy-denied). `unsafe` denied. `cargo fmt`/`clippy -D warnings`/
`test` are the merge bar. Ambiguities become errata in `spec/errata/`, never guesses.
See CONTRIBUTING.md for the feedback protocol.
