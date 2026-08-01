# TCB manifest — canonical identity and encoding

- **Exact modules:** `brix-canon/src/lib.rs`, `int_codec.rs`, `decimal.rs`,
  `float.rs`, `quantity.rs`; semantic identity wrappers and `Canonical`
  implementations in `brix-semantic/src/{id,context,proposition,evidence,judgement,decomposition,dependency,witness,generator,regime,outcome}.rs`.
- **Boundary and closure:** the canonical **codec root** is `brix-canon` plus
  `blake3`, `indexmap`, and `unicode-normalization`. The canonical **artifact
  identity boundary** also includes `brix-semantic`, whose identity wrappers
  and `Canonical` implementations consume that codec root. Thus the workspace
  closure reported in the audit table is `brix-semantic → brix-canon`, not
  `brix-canon` alone.
- **Entry points:** `CanonWriter`, `Canonical::canon_write`, `Digest::of`, and
  each `*Id::of`/`from_canon` wrapper.
- **Durable inputs/outputs:** canonical bytes and domain-separated digests for
  contexts, propositions, evidence, judgements, decompositions, dependencies,
  witnesses, regimes, and verifier/certificate IDs.
- **Assumptions:** canonical writer tags, field order, enum ordinals, Unicode
  normalization, and BLAKE3/domain separation remain stable; frozen vectors
  are reviewed on intentional ABI changes.
- **Excluded:** parsing, settlement selection, audit replay, proof search,
  proof acceptance, CLI/presentation, and host serialization formats.
