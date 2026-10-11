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

Decisions read (`all_settlements`, 1M facts / 800k decisions, fresh process): the first read walks
the candidate frontier (about 6-10 s on a loaded machine, including a one-time ~3.5 s to verify the
settlements against the decision root and write the settlement checkpoint); every later read is
served from `settlements.ckpt` in ~0.6 s.

## Known limits (tracked for the next milestones)

- 1M load is ~259 s wall after the load-growth work (lane A); per-batch time still grows with N.
  `open-check --refresh-state` moves the first decisions read off the request path.
- Disk is ~1.26x alpha.4 before GC; `brix world gc` removes about 40% of it (dead nodes of
  superseded revisions). Sessions open across a GC keep working (see "Garbage collection").
- `--keep-revisions N` cannot see in-memory reader pins (`pin_reader`) of other processes: do not
  run it while readers hold older revisions. Persisted checkpoint pins are honoured.
- An idempotency-index trie node that is damaged stays in its pack until the next GC; until then
  each open falls back to the receipt scan (commits and opens keep working).

## API changes for embedders (Atlas)

- New: `WorldSession::open_with_policy(&Path, OpenPolicy)`, `open_replay`, `refresh_state_cache`,
  `last_commit_profile`, `OpenMode`, and the fields `open_mode`, `replay_reason`, `open_ms`,
  `state_source`, `state_refreshed`, `tail_batches`.
- New decisions/idempotency APIs: `all_settlements_with_source`, `settlements_page`,
  `refresh_settlement_checkpoint`, `verify_settlement_checkpoint`, `persist_idempotency_index`,
  `idempotency_lookup`, `world::gc::{gc_world, verify_store}`.
- **Breaking:** `WorldSession.committed_idempotency_keys` (a map of every key) is replaced by
  `idempotency: IdempotencyIndex` (a persisted trie, O(log n) lookups, no receipt scan at open).
  Use `idempotency_lookup(key)`.
- **Retuning:** `node_cache_bytes` is now the budget for the *total* of three caches (state store,
  base relations, raw bytes), default 512 MiB, and a decoded node is accounted at 8x its encoded
  size. Worlds that set `node_cache_bytes` explicitly in `world.json` were tuned for one cache of
  the old accounting and should be retuned (smaller values now mean fewer cached nodes overall).
- Revision schema is now `brix.world.revision@3` (carries `network_state_root`). Older worlds still open.
  An `@3` revision on an executable world without a root, or with damaged state, is a hard error.
- A restored network has no in-memory base copy: `current_state().base_relations` is empty and
  `recompute_from_scratch` is not meaningful there. Use `verify_state`.
- `set_network` and `from_program` refuse a program that differs from the one the world is bound to.
- Query APIs return `Result`.
- CLI/serve: `--replay`, `brix world open-check <dir> [--verify-state] [--refresh-state]`, JSON fields
  `open_ms`, `open_mode`, `replay_reason`, `state_source`, `state_refreshed`, `tail_batches`; new
  error statuses `corrupt-state`, `missing-state`, `state-mismatch`, `world-locked` (another
  writer holds the world: retry; previously reported as `unknown-error`), `stale-session` and
  `revision-compacted` (history dropped by `gc --keep-revisions`, not damage). Existing statuses
  and exit codes are unchanged.
- `open-check --refresh-state` now also rewrites the settlement checkpoint (after proving its
  settlements hash to the decision root).
- The `r0-oracle` feature is removed; CI no longer needs it.
- Storage: `NodeStore::get_node` returns `Result<Option<_>, StorageError>` (breaking for custom
  stores). Pack damage is `CorruptedNode` / `WorldError::CorruptState` (also in GC and for a
  damaged checkpoint or idempotency entry); OS errors are `Io`.
- `pin_revision` of a revision removed by `--keep-revisions` returns `RevisionCompacted(seq)`
  instead of handing back a snapshot whose reads fail with `missing-state`.

## New files in a world directory

All are caches or markers; deleting any of them never changes an answer.

| File | Purpose |
|---|---|
| `state-cache.json` | pointer to restorable network state (legacy worlds) |
| `idempotency.json` | root of the persisted idempotency index (`brix.world.idempotency-index@1`) |
| `.idempotency.tmp` | its temporary file |
| `settlements.ckpt` | flat copy of every settlement at one revision (`brix.world.settlement-checkpoint@1`) |
| `settlements.tmp.<pid>` | temporary file of a checkpoint write (swept by GC when old) |
| `checkpoint.key` | per-world secret (32 random bytes, mode 0600) that authenticates the checkpoint; a checkpoint is only served if its keyed digest verifies |
| `objects/GC_EPOCH` | generation marker rewritten by every GC (see below) |

## Garbage collection

GC: `brix world gc <dir> [--dry-run] [--keep-revisions N]` (serve `world.gc`), library
`world::gc::gc_world(dir, &GcOptions)`. Single writer. Default keeps every revision readable.
`brix world open-check --verify-store` (serve `verify_store`, `world::gc::verify_store`) audits every
pack body checksum.

- **Sessions open across a GC are safe.** GC rewrites `objects/GC_EPOCH` before it retires anything
  and again after. A session re-reads it under the writer lock before every write that persists
  nodes (commit, state-cache refresh, idempotency persist); if it changed, the session reloads the
  pack set and clears every cache first, so nodes GC deleted are written again. No reopen needed.
  A read-only session can still serve pre-GC cached nodes (valid when cached); reopen for a fresh view.
- `verify_after` (default) reopens the store and the world, runs `verify_state`, reads the whole
  idempotency trie and resolves every retained revision's relation, index and decision roots.
- `--keep-revisions` accepts any value (near `u64::MAX` keeps everything).
- `dead_nodes` counts distinct digests (a node written by several commits counts once).

## Read-path cache writes and the writer lock

Opening a world or reading decisions may write caches (a state-cache refresh after a replay, the
idempotency sidecar upgrade, the settlement checkpoint). These are strictly best effort and stay
out of a writer's way:

- They fail fast (`WorldLocked`, recorded as `state_refresh_error`) instead of waiting when a
  writer holds the lock, and do their heavy writes (content-addressed, additive) *without* the
  lock; the lock is taken only to re-check HEAD and the GC generation and publish the sidecar.
- The checkpoint write takes no lock at all (temporary file + rename).
- A commit waits up to 500 ms for the lock before reporting `world-locked`, so a reader's brief
  publish step never fails it. An explicit `refresh_state_cache()` / `open-check --refresh-state`
  holds the lock throughout, as before.

## Fixed

- Frontier retract-before-insert bug (also present in alpha.4).
- A session open before a GC could commit a HEAD whose nodes had been deleted (see above).
- A settlement checkpoint with a self-consistent but forged body was served as truth; checkpoints
  are now authenticated, and the writer verifies the settlements against the decision root first.
- A torn receipt dropped its idempotency key from the receipt scan; the damaged cache can no longer
  block commits (the index is rebuilt from the receipts and the lookup retried).
- `--keep-revisions` near `u64::MAX` overflowed.
