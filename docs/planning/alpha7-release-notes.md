# Alpha.7: Qualified Nominals, List-Valued Payloads, and World Porting Gaps

Baseline: `v0.1.0-alpha.6` (`94291dd`).

## Scope

Alpha.7 addresses the remaining blockers discovered during Atlas world module porting:

- **R5: Qualified nominal names in batch input exceeding 64 bytes.**
  Batch input now validates identifier limits per segment (up to 64 bytes) rather than capping the entire `<module>::<Type>` string at 64 bytes. Fully-qualified names are bounded up to `MAX_QUALIFIED_NAME_BYTES = 130` (2 × 64 + 2 bytes for `::`). Error reporting names the nominal, offending segment, and relation/key/field path instead of generic string-length errors. Input and output nominal names now fully agree end-to-end through settlement, cold open, and independent verification.
- **R6: List-valued values in types and decision payloads.**
  World `config` declarations may now safely name `List<T>` without failing `world init` even when unreferenced. Settled decision values can carry ordered, bounded `List<T>` payloads (scalars, records, variants). List identity preserves element ordering and duplicates. List operations (`len`, `map`, `filter`, `all`, `any`, `distinct`) work across helper functions and decision expressions, bounded by container width (`MAX_INPUT_CONTAINER_WIDTH = 256`) with deterministic fail-closed enforcement.
- **R7: World-profile porting gaps & query enhancements.**
  - **R7.1 / Chained Counts:** Cancelled transient intermediate delta churn in parent-count cascades via net-effect consolidation.
  - **R7.2 / Disjoint Union:** Incremental `OperatorNode::DisjointUnion` operator combining disjoint relation streams.
  - **R7.3 / Aggregate Match Typing:** Support for match expressions directly inside aggregation arguments (e.g. `sum(match x.act { true => x.amt, false => 0 })`).
  - **R7.4 / Nested Record Projection:** Downstream projection and aggregation over nested record fields.
  - **R7.5 / Absence Conjuncts & Causality:** Top-level `&& not exists` parsing and causal intra-batch delta ordering (net effect balancing of intra-batch insertions and retractions).
  - **R7.6 / Inferred Helper Returns:** Type inference for unannotated helper functions returning constructed nominals.
  - **R7.7 & R7.8 / Clarified Constructor & Keyword Errors:** Actionable diagnostics for ambiguous unqualified constructors across imported modules, and reserved module name validations.

## Storage and Compatibility

- Crate version bumped to `0.1.0-alpha.7`.
- `ExecProfileV1` supports world sessions and audit bundles created under `0.1.0-alpha.5`, `0.1.0-alpha.6`, and `0.1.0-alpha.7`.
- Input wire format and state codecs remain canonical and forward-compatible.
