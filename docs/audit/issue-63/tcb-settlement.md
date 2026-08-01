# TCB manifest — settlement commitment (`Derived`)

- **Exact authority-path modules:** `soc-core/src/commit.rs`
  (`commit_tick`, `run`, `SettlementRegime`) is the publication boundary, with
  direct path dependencies `calendar.rs` (frontier/key selection), `adm.rs`,
  `regime.rs`, `exec.rs`, `intern.rs`, `journal.rs`/`history.rs` (recording and
  chained history), `oracle.rs` (**`apply` only**), and `cost.rs`. The semantic
  data boundary is `brix-semantic`'s context/config/decomposition/evidence/
  judgement/outcome/realizes/witness types over `brix-canon` digests.
- **Package dependency closure:** `soc-core → {brix-canon, brix-semantic}` and
  transitive `blake3`, `indexmap`, `unicode-normalization`. This is the Cargo
  closure, not a claim that every module in `soc-core` is authority code.
- **Entry points:** `commit_tick` and `run`; `SettlementRegime::decompose` is
  the boundary by which an untrusted regime supplies a recorded decomposition.
- **Durable inputs/outputs:** inputs are canonical context/config/generator
  identities plus regime candidates and calendar keys; outputs are a journaled
  `CommittedStep`, `Observation { Derived, JudgementId digest }`, and a
  revision-scoped `SettlementReplay` evidence ID.
- **Assumptions:** calendar key uniqueness, regime candidate/decomposition
  honesty, interner handle-to-digest correctness, and recorded decomposition
  construction. The reference oracle/differential tests constrain behavior but
  do not replace the settlement boundary.
- **Excluded:** `delta.rs`, `engine.rs`, and `store.rs`; `oracle` candidate
  enumeration (`cand`, `cand_instrumented`) and successor helpers other than
  `apply`; realization/type inference, parser/lowering, proof search, proof
  acceptance, and audit verification (which is a separate route).
