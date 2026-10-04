# P6 audit contract — independent evidence for `brix.world@1`

Status: **design check, 2026-10-04**, for plan §P6 (`persistent-world-runtime-plan.md`) and ADR-0046 §5.
Scope: what `brix world audit` emits, what `brix world verify` recomputes, and the smallest
PR sequence Sonnet can build against. Every claim about current behaviour cites the code on
`feature/persistent-world-runtime` as of this date. Nothing here changes an accepted ADR;
ADR-0046 is still Draft and no `brix.world.*` schema has a frozen vector (`vectors/` has none).

## 0. What the runtime persists today, and what the verifier needs

| Persisted | Where | Verifier use |
| --- | --- | --- |
| World catalog (`brix.world@1`) with `program_digest`, `program_required`, relation decls | `manifest.rs:106-116`, digest `154-168` | Recompute digest; schema for tuple decoding |
| Source closure `program.json` (`brix.world.program@1`: `root_module`, `sources`, `program_manifest_digest`) | written `session.rs:1316-1323`, checked on open `609-656` | Re-link independently; the only program identity |
| Per-revision record (`brix.world.revision@1`): seq, timestamp, base, idempotency key, `batch_digest`, `program_digest`, `previous_revision_digest`, `relation_roots`+cardinalities, `secondary_index_roots`, `decision_root`, `changed_keys`, status | `revision.rs:92-109`; digest `180-235`; decode recomputes digest `524-530` | Chain + claimed roots |
| Primary and secondary trie nodes, content-addressed, digest-verified on lazy read | `store.rs:1211-1249` encode, `1369-1378` verify, `1562-1575` resolve | **Producer only** (§2) |
| HEAD: `"0\n"` at create, `"<seq> <digest>\n"` after a batch | `session.rs:432` vs `1066`; open parses first token only `503-509` | Head pin cross-check |

**Not persisted (gaps the contract must close):**

- G1 **Batch bodies.** Only `batch_digest` is stored (`session.rs:1033`); staging chunks are deleted
  after publication (`1606`). The digest is therefore unverifiable from the directory. Contract: the
  logical source delta is *re-derived from roots*, and `batch_digest` is reported as an unverified label.
- G2 **Decisions.** `decision_tree` is in-memory only (`network.rs:593`); only its root reaches the record.
  `persist_to_store` is called for relation/secondary tries only (`session.rs:994-1002`). Historical
  settlements cannot be read back; the whole network is rebuilt by replaying every base record on open
  (`session.rs:1280-1300`).
- G3 **Settlement trace.** No per-revision record of which `(decide, entity)` settlements changed.
  `NetworkDeltaReport.settlements` has the inserts (`network.rs:1495-1499`) but removals are only
  visible as `decision_tree.remove` (`1501`), and the report is discarded.
- G4 **Execution profile.** `ProgramGraphManifest` digest covers profile ident, root module, per-module
  source/interface digests, byte counts and imports (`module_graph.rs:459-490`) but no evaluator or
  compiler version; `ModuleLoaderLimits::default()` is hard-coded (`session.rs:45`, `module_graph.rs:24-33`).
  No fuel/exhaustion limit exists; `SettlementStatus::Unknown` is never produced (`network.rs` only
  surfaces `NetworkError("Unknown(EvaluationFault)…")` strings, `network.rs:70,130`).
- G5 **Entity identity is heuristic.** `extract_entity_id` (`network.rs:1790-1838`) falls back from
  `propose.deps[0]` to `entity_id`/`order_id`/`id`/`key`, then any `*_id` field, then the first field,
  then `"default_entity"`. An independent evaluator cannot be independent of an unspecified heuristic.
- G6 **Timestamp** is the constant `"2026-10-04T00:00:00Z"` (`session.rs:1622`) and is hashed into the
  revision digest (`revision.rs:184`). Harmless for determinism, misleading as provenance.
- G7 `recompute_from_scratch` (`network.rs:1621-1645`) instantiates the same `WorldNetwork`; its doc
  comment already says it "checks update history, not independent semantics". It is not P6 evidence.

## 1. The world audit bundle `brix.world.audit@1`

Producer: `brix world audit <dir> --out <file.brixaudit> [--from-checkpoint <seq>]`. Verifier input
only; never read by the runtime. Mirrors ADR-0026 ⟨D-BUNDLE⟩/⟨D-SNAPSHOT⟩: transport what is needed
to **re-derive**, never the derived artifacts.

```text
WorldAuditBundleV1                      canonical via brix-canon CanonWriter (write_tag "brix.world.audit@1")
  world_manifest      : WorldManifest     full catalog; verifier recomputes WorldManifest::digest
  program             : ProgramClosureV1  root_module, sources as sorted (ident, bytes) list,
                                          program_manifest_digest  (= today's program.json content)
  exec_profile        : ExecProfileV1     see §1.1 — recorded, not trusted
  scope               : Genesis | Checkpoint{ seq, revision_digest, state: CheckpointStateV1 }
  revisions           : [ RevisionEntryV1 ]  contiguous seq = scope.start+1 .. head.seq
  head                : { seq, revision_digest }

RevisionEntryV1
  record              : WorldRevision     every field of brix.world.revision@1 (decode recomputes digest)
  source_delta        : [ (relation ident, key bytes, Option<tuple bytes>) ]  strictly increasing (relation,key)
  decision_delta      : [ (decide ident, entity str, Option<DecisionTupleV1>) ] strictly increasing (decide,entity)

CheckpointStateV1   (only when scope = Checkpoint)
  relations           : [ (relation, [ (key, tuple) ]) ]   full source state at checkpoint, sorted
  secondary_indexes   : none — rebuilt from relations + catalog
  decisions           : [ (decide, entity, DecisionTupleV1) ]  full settlement map at checkpoint

DecisionTupleV1 = fields of SettledDecision encoded exactly as decision_tuple() (network.rs:450-460):
  candidate_name, priority, phase, value, tiebreak digest
```

Identity: `WorldAuditBundleIdV1 = Digest(Domain::Value, "brix.world.audit-bundle" ∥ 1 ∥ "brix.world.audit-bundle@1" ∥ body)`.
Add the ADR-0023 ⟨D-DISJOINT⟩ non-collision test against `brix.soc.audit-input-bundle`,
`brix.kb.revision@1`, `brix.world.revision@1`, `brix.world@1`, `brix.world.batch@1`.

**Deliberately not transported:** trie nodes, `objects/`, secondary-index contents, operator states,
supports, candidate frontiers, `WorldNetwork` anything, any `Audited` tag, any decode limit.
`source_delta` is derived by the producer from `record.changed_keys` plus lookups at roots `n-1` and
`n` (`WorldSnapshot::get`, `session.rs:310-317`), i.e. the same walk `diff_page` does (`1492-1558`).
`decision_delta` requires closing G3 (§6 step 2) — until then the producer must recompute it with the
reference evaluator, which makes "audit" cost O(Σ|world_n|) and is acceptable only as a bootstrap.

### 1.1 `ExecProfileV1` (closes G4)

`{ profile: "brix.world.exec@1", evaluator: "brix-world-ref@1" | "brix-world-net@1", crate_version,
module_loader_limits{depth,modules,module_bytes,total_bytes}, numeric_semantics: "ADR-0045",
settlement: "least-key(phase,priority,tiebreak)" }`. Recorded at `world init` into `program.json`
(additive field `exec_profile`; today's files decode with the default) and into the bundle. The verifier
*compares* it to its own profile and refuses on mismatch; it never adopts limits from the bundle
(plan §3.8: "Host operational limits must never be relaxed by an untrusted artifact").

## 2. Verifier authority — `brix world verify`

```text
brix world verify --expect-head <hex> --expect-program <hex> <bundle.brixaudit>
brix world verify --expect-head <hex> --expect-program <hex> --in-place <dir>     # audit-to-memory then verify
```

Two external pins are **required** (ADR-0026 ⟨D-PIN⟩): the head revision digest and the
`program_manifest_digest`. A one-pin or zero-pin form is a usage refusal, not a self-consistency mode.

**Recomputes (authority):**
1. `WorldManifest::digest`, `ProgramGraphManifest.transitive_manifest_digest` by re-running
   `ModuleGraph::load` + `link()` on the transported sources with the verifier's own limits
   (`session.rs:40-60` shape); requires closure == sources exactly (`640-644`); equals `--expect-program`.
2. Revision chain: `record.seq` contiguous, `previous_revision_digest` == previous `revision_digest`,
   `expected_base_revision == seq-1`, `program_digest == Some(expect_program)` for every entry,
   `status == Committed` (an `Unknown` record is not verifiable and must not appear in a bundle, as
   ADR-0026 §8 refuses partial bundles); final digest == `--expect-head`.
3. Source state: own `TrieMap` per relation (in-memory, `MemoryNodeStore` or none) folded from
   genesis-empty (or `CheckpointStateV1`) by `source_delta`; after each revision
   `root_digest == record.relation_roots[rel]`, `len == relation_cardinalities[rel]`, and
   `keys(source_delta) == record.changed_keys` as sets (both directions). Secondary indexes: rebuild
   from scratch per revision via `extract_indexed_field`/`encode_secondary_key` (`codec.rs:112-137`)
   and compare `secondary_index_roots`. Rebuilding is O(|rel|) per revision; it is charged (§4).
4. Decisions: the **reference evaluator** (separate lane; no `network.rs` symbol imported) computes
   the full settlement map `S_n` from source state `n`; verifier requires
   `compute_decision_root(S_n) == record.decision_root` (`network.rs:430-440` is pure canonical
   encoding; reuse is encoding reuse, not derivation reuse — move it to `world/decision_codec.rs` so
   neither evaluator owns it) and `decision_delta == S_n △ S_{n-1}` exactly, for **every** entity in
   **every** decide block. There is no first-decision or head-only shortcut.
5. Settlement rule check inside the reference evaluator: least `Key(phase, priority, tiebreak)` among
   candidates with non-empty support (`network.rs:1530-1551`), tiebreak as
   `compute_candidate_calendar_key` (`471-491`); scalar guards/values via `brix_lower::l3_v2` only.

**Trusts (reported, never verified):** `timestamp`, `idempotency_key`, `batch_digest` (G1),
`world_id`, `created_at`. Printed under a `not-verified:` heading, as `brix verify` refuses to over-claim
(ADR-0026 §8).

**Outcome lattice.** Reuse `brix_semantic::Outcome` (`outcome.rs:43-67`): the runtime's own results
are `Derived`; a complete pass earns `Audited` on *each* `(seq, decide, entity)` settlement and on the
head state; any refusal is `Unknown(<stable-reason>)`, exit nonzero, never `Refuted` (absence is not
refutation). `Proven` is never emitted. JSON: `brix.cli.world-verify-result@1`.

**Scope reporting (mandatory line):** `scope: complete-from-genesis (0..N)` or
`scope: checkpoint-suffix (C..N) trust=<basis>`. Checkpoint trust bases: (a) `verified-by-this-tool`
— the verifier previously emitted a `CheckpointStateV1` digest and the caller passes it as
`--trust-checkpoint <hex>`; (b) `caller-pin` — `--trust-checkpoint <hex>` supplied by deployment.
The verifier recomputes relation roots and `decision_root` *of the checkpoint state* against the
checkpoint record before replaying the suffix, so a pin only vouches for history, not for the
transported bytes. Without a pin, a Checkpoint-scoped bundle is `Unknown(checkpoint-untrusted)`.

## 3. Tamper matrix

Each row is one test in `crates/brix-kb/tests/world_audit_tamper.rs` (library-level) plus the CLI
rendering in `crates/brix-cli/tests/world_verify.rs`. All must end in `Unknown`, never `Audited`.

| Tampered | Specific check that rejects | Test |
| --- | --- | --- |
| Input tuple bytes at revision `n` (edit `source_delta` or an `objects/` leaf before `--in-place`) | step 3: rebuilt `root_digest != relation_roots[rel]`; in-place: `decode_node_verified` → `CorruptedObject` | `t01_input_tuple_edit_breaks_relation_root` |
| Dependency code: one imported helper edited in `sources` | step 1: re-linked `transitive_manifest_digest != --expect-program` and `!= record.program_digest` | `t02_dependency_source_edit_breaks_program_pin` |
| Caches: `objects/` node swapped for a different valid node | step 3 via producer: `resolve_node` digest mismatch → `CorruptedObject`; bundle path: unaffected because nodes are not transported, roots recomputed | `t03_cache_object_swap_is_corruption_not_evidence` |
| Secondary index root in record edited (digest re-sealed, chain re-sealed) | step 2: head digest `!= --expect-head`; if head also forged: step 3 rebuilt index root mismatch | `t04_index_root_forgery_fails_pin_then_rebuild` |
| Supports: a derived tuple's support set faked (only reachable by patching the runtime) | not an input to the verifier at all; reference evaluator ignores supports and recomputes `S_n` → `decision_root` mismatch | `t05_support_table_is_not_verifier_input` |
| Checkpoint state edited (`CheckpointStateV1` relation or decision) | §2 scope: recomputed checkpoint roots `!= checkpoint record`; and `--trust-checkpoint` digest mismatch | `t06_checkpoint_state_edit_rejected` |
| Trace order: two `decision_delta` entries swapped, or two revisions swapped | format refusal before any semantic work: non-increasing `(decide, entity)` / non-contiguous `seq` (ADR-0026 ⟨D-DECODELIMITS⟩ ordering rule) | `t07_trace_reorder_is_format_refusal` |
| One later decision: `DecisionTupleV1` of a non-head revision changed | step 4 at that `seq`: `compute_decision_root(S_n) != record.decision_root` or `decision_delta` mismatch — verified per revision, so head agreement cannot mask it | `t08_single_historical_decision_edit_rejected` |
| Cross-model boundary: a tuple in a relation owned by module `B` injected into revision `n` of a world whose closure pins `B` | step 1/3: relation must exist in `world_manifest.relations` **and** in the re-linked program's `rel_inputs` (`validate_program_relations`, `session.rs:62-75`); unknown qualified relation → `Unknown(unknown-relation)`. Pinned foreign-world connections do not exist yet (`connect` unimplemented); bundle reserves `connections: []` and refuses non-empty | `t09_foreign_module_relation_rejected` |
| Honest control: fresh process, `objects/` deleted after `audit`, bundle verified | all steps pass; `scope: complete-from-genesis`; every settlement `Audited` | `t10_honest_replay_without_caches_passes` |
| Attribution control: support for an entity retracted at `n+1` | `decision_delta[n]` still carries the original settlement, `decision_delta[n+1]` carries `None`; both Audited | `t11_historical_evidence_keeps_original_revision` |

## 4. Lazy evidence expansion — bounds and metering

- `WorldAuditDecodeLimits` (noncanonical, verifier-owned, contributes to no identity): total bundle
  bytes, revisions, entries per delta, tuple bytes, checkpoint rows, sources bytes, total decisions.
  Every bound fires **before** the allocation or loop it governs (ADR-0026 §6 ordering; checked
  `u64→usize`; frame length before slice; `checked_add` cumulative totals).
- Work meter `VerifyWorkReport { tuples_decoded, trie_nodes_built, index_rows_rebuilt,
  candidates_evaluated, settlements_computed, revisions_replayed }` printed on success and failure;
  a `--max-work <n>` budget refuses with `Unknown(verification-budget-exhausted)` and the scope
  actually completed. Partial completion is never reported as a pass (plan §3.8).
- Explanation expansion (`brix world explain --rev <n>`) is **not** evidence; it reads the persisted
  decision trie (after G2 is closed) and is bounded by the same decode limits. It must print
  `authority: Derived` unless the caller supplies a bundle id previously verified in this process.
- Expected cost, stated honestly: complete verification is Σ_n (|delta_n| + |index rebuild_n| +
  |S_n| evaluation) — size-dependent by design (plan §1). Checkpoint-suffix bounds it to the suffix.

## 5. KB v1 → world import

`brix world import-kb <kb-dir> <world-dir>`: read-only on `<kb-dir>`; refuses if `<world-dir>` exists.

1. Run `brix_kb::ops::verify` (`ops.rs:826-895`) first; any `Unknown` refuses the import.
2. Create a **storage-only** world (`program_required=false`) with one relation `kb_inputs`
   keyed by `name`, value fields `{schema, value_json}`; each `InputValue` is encoded with the strict
   `snapshot_io` encoder for the single value (exact, versioned, round-trips).
3. One world revision per KB revision `1..=head`, batch = set difference of consecutive snapshots,
   `idempotency_key = "kb-import:<kb revision digest hex>"`.
4. Provenance sidecar `import-provenance.json` (`brix.world.import-kb@1`): source dir, `brix.kb@1`
   profile, KB HEAD digest, and per-revision `{kb_seq, kb_revision_digest, program_id, snapshot_id,
   context_id, result{status, candidate, decision_digest}, world_seq, world_revision_digest}`. The
   KB result is carried as a **claim attributed to KB v1**, never as a world decision; the world's
   `decision_root` stays `None`.
5. **Refusals** (`Unknown(import-unsupported:<reason>)`, no partial directory left): `Change::Program`
   anywhere in history (a finite-decision program is not a relational program; re-targeting it would
   approximate), `Status::Unknown` or `MissingInputs` results are *allowed* as claims but a broken
   chain/HEAD mismatch refuses, any input name not a valid `write_ident`, a snapshot that fails strict
   re-encoding, or a KB profile other than `brix.l3.finite-decision@1`.
6. Tests: `kb_import_preserves_old_directory_bytes`, `kb_import_refuses_program_change`,
   `kb_import_round_trips_every_snapshot`, `old_kb_commands_still_read_imported_source`.

## 6. Smallest P6 slice — ordered, PR-sized

| # | PR | Files owned (nothing else) | Exit |
| --- | --- | --- | --- |
| 1 | Freeze entity identity (G5) | `brix-lower/src/relation_dag.rs` (require `decide … per <field>` or `deps[0]`), `brix-kb/src/world/network.rs` (delete heuristic fallbacks, error instead), tests `world_operator_network.rs` | Unspecified entity → lowering error; existing P4/P5 fixtures updated to explicit field |
| 2 | Persist decisions + trace (G2, G3) | `session.rs` (persist `decision_tree` nodes; write `revisions/{seq}.decisions.json` = `decision_delta` before revision fsync), `network.rs` (return removed settlements in `NetworkDeltaReport`), `paths.rs` | `explain --rev n` reads history; crash points a06 extended to the sidecar |
| 3 | `ExecProfileV1` (G4) | `session.rs` program.json additive field, `manifest.rs` no change, `brix-cli/commands/world.rs` init prints it | Old `program.json` still opens |
| 4 | Reference evaluator (separate lane, already running) | new `brix-kb/src/world/reference.rs`; `decision_codec.rs` extracted from `network.rs:430-460` | Differential test vs `WorldNetwork` on P4/P5 fixtures; `cargo tree`-style test asserts `reference.rs` has no `use super::network` |
| 5 | Bundle codec + producer | new `brix-kb/src/world/audit.rs` (`WorldAuditBundleV1`, encode/decode with limits, id, non-collision test), `brix-cli/commands/world.rs` `audit` | Hostile-input gates (truncation, counts, order) |
| 6 | Verifier library + CLI | `brix-kb/src/world/verify.rs`, `brix-cli/commands/world.rs` `verify`, `cli.rs` arg parsing, `serve.rs` `world.verify` | Tamper matrix §3 green; `t10` honest control |
| 7 | Checkpoint scope | `audit.rs`/`verify.rs` additive `CheckpointStateV1` | `t06`; scope line rendered |
| 8 | KB import | new `brix-kb/src/world/import_kb.rs`, CLI op | §5 tests |
| 9 | ADR-0046 §5 + plan §9 checklist update | `spec/adr/ADR-0046…md`, plan | Schemas named here become field-level definitions |

Avoid concurrent edits to `cli.rs`, `serve.rs`, `mod.rs` re-exports; PRs 5–6 touch them once each.

## 7. Decisions that need Tony

1. **Entity identity (G5):** require an explicit `per <field>` in `decide` (breaking P4/P5 fixtures) or
   freeze `deps[0]` as the rule and reject everything else? Recommendation: explicit field.
2. **Decision delta binding:** sidecar claims re-verified by recompute (no format change, this doc) vs
   additive `brix.world.revision@2` carrying `decision_delta_digest`. Recommendation: sidecar now; `@2`
   only if a consumer needs the digest without running the evaluator.
3. **Reference-evaluator cost:** full recompute per revision is O(Σ|world_n|). Accept for P6 with
   checkpoint-suffix as the practical path, or require a semi-naive (still network-free) evaluator
   before P8's million-row claim?
4. **KB import semantics:** confirm "storage-only world + KB results as attributed claims" is the
   intended translation, rather than refusing import for any executable KB.

### 7.1 Decided — Tony, 2026-10-04

1. **Entity identity: explicit `per <field>` in `decide`.** The heuristic in `extract_entity_id`
   is retired; a `decide` without `per` is refused. P4/P5 fixtures are migrated, not grandfathered.
2. **Decision delta: bound into `brix.world.revision@2`** with `decision_delta_digest` (and decision
   root persisted). No world format is frozen yet, so this replaces the sidecar plan in §1/§6 step 2.
3. **Reference cost: accepted for P6.** Full recompute is the authority; checkpoint-plus-suffix is the
   practical path at 1M rows and must report its scope; complete-from-genesis runs as a periodic
   release-qualification job. No second (semi-naive) evaluator before P8.
4. **KB import: confirmed** — storage-only world, KB results carried as attributed claims, refusals
   as in §5.
