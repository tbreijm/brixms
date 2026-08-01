# TCB manifest — audit verification (`Audited`)

- **Exact modules:** `soc-core/src/audit.rs` (`audit_step`, `audit_journal`,
  `GeneratorSemantics`) with `journal.rs`; semantic support in
  `brix-semantic/src/{id,context,proposition,decomposition,evidence,judgement,dependency,witness,generator,outcome,realizes}.rs`.
- **Package dependency closure:** `soc-core → {brix-canon, brix-semantic}` and
  transitive `blake3`, `indexmap`, `unicode-normalization`. This describes the
  Cargo closure only; the exact audit authority boundary is the `audit.rs` /
  `journal.rs` and semantic modules named above, not every `soc-core` module.
- **Entry points:** `audit_step` and `audit_journal`.
- **Durable inputs/outputs:** a committed journal step, context, generator
  registry, and replay semantics produce either `AuditResult::Unknown` or a
  new `AuditedStep`: replay-verified decomposition, `Audited` judgement,
  evidence ID, and premise dependency to the rebuilt `Derived` judgement.
- **Assumptions:** generator registry/semantics faithfully represent the
  intended relation and the journal input is available. Endpoint, chain,
  registry, and replay checks are runtime-validated; a failed check is
  explicitly `Unknown`.
- **Excluded:** candidate selection, parser/lowering, type realization,
  proof-term checking, and publishing `Proven`/`Refuted`.
