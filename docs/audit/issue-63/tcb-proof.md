# TCB manifest — proof acceptance (`Proven`/`Refuted`)

- **Exact modules:** `brix-kernel/src/{lib,term,check,verdict}.rs`; its
  publisher bridge is deliberately outside the kernel in
  `brix-elaborate/src/lib.rs:elaborate_and_publish`.
- **Closure:** `brix-kernel → {brix-canon, brix-semantic}` and transitive
  `blake3`, `indexmap`, `unicode-normalization`.
- **Entry point:** `brix_kernel::acceptance(context, proposition, term, budget)`.
- **Durable inputs/outputs:** context ID, proposition, explicit term, and
  budget yield an exhaustive `Verdict`; only `Accepted(Certificate)` maps to
  `Proven` and the certificate carries a verifier/certificate identity.
  `ResourceExhausted` maps to `Unknown`; the present native path does not mint
  a refutation certificate.
- **Assumptions:** structural checker correctness, complete budget accounting,
  explicit-term/context match, canonical identity substrate, and a stable
  certificate encoding. The final assumption is currently weakened by C-1.
- **Excluded:** parsing, tactics/proof search, inference, regimes, settlement,
  audit, CLI/presentation, and any reverse dependency on them.
