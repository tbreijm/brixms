# 0.1.0-alpha.5 release notes (draft)

R0: cold open without replay. `WorldSession::open` restores the operator network from
content-addressed tries instead of replaying base facts (ADR-0046 s5: persisted state is a cache).

## Results (release build)

| Facts | alpha.4 cold open | alpha.5 cold open |
|---|---|---|
| 10k | ~1 s | ~3 ms |
| 100k | ~10.9 s | ~14 ms |
| 1M | n/a | ~140 ms |

Restore equals replay byte for byte (differential fuzz, negative controls, crash points).
A legacy (`@2`) world replays once on its first open, then restores from `state-cache.json`.

## Known limits (tracked for the next milestones)

- Load time still grows per batch at large N (decoded-node cache thrash); 1M load is ~238 s.
- The first full `all_settlements` at 1M facts / 800k decisions takes ~7 s; a settlement checkpoint is planned.
- Disk is ~1.26x alpha.4; no GC yet.
- `FileNodeStore` pack damage can surface as `MissingState` instead of `CorruptState`.

## API changes for embedders (Atlas)

- New: `WorldSession::open_with_policy(&Path, OpenPolicy)`, `open_replay`, `refresh_state_cache`,
  `last_commit_profile`, `OpenMode`, and the fields `open_mode`, `replay_reason`, `open_ms`,
  `state_source`, `state_refreshed`, `tail_batches`.
- Revision schema is now `brix.world.revision@3` (carries `network_state_root`). Older worlds still open.
  An `@3` revision on an executable world without a root, or with damaged state, is a hard error.
- A restored network has no in-memory base copy: `current_state().base_relations` is empty and
  `recompute_from_scratch` is not meaningful there. Use `verify_state`.
- `set_network` and `from_program` refuse a program that differs from the one the world is bound to.
- Query APIs return `Result`.
- CLI/serve: `--replay`, `brix world open-check <dir> [--verify-state] [--refresh-state]`, JSON fields
  `open_ms`, `open_mode`, `replay_reason`, `state_source`, `state_refreshed`, `tail_batches`; new
  error statuses `corrupt-state`, `missing-state`, `state-mismatch`.
- The `r0-oracle` feature is removed; CI no longer needs it.
- Storage: `NodeStore::get_node` returns `Result<Option<_>, StorageError>` (breaking for custom
  stores). Pack damage is `CorruptedNode` / `WorldError::CorruptState`; OS errors are `Io`.
- GC: `brix world gc <dir> [--dry-run] [--keep-revisions N]` (serve `world.gc`), library
  `world::gc::gc_world(dir, &GcOptions)`. Single writer; reopen sessions afterwards. Default keeps
  every revision readable. `brix world open-check --verify-store` (serve `verify_store`,
  `world::gc::verify_store`) audits every pack body checksum.

## Fixed

- Frontier retract-before-insert bug (also present in alpha.4).
