# Seed tickets

Each file here is a complete `TicketStore.enqueue` payload for an ordinary,
narrowly-scoped BrixMS core-package change: one read-only query added to an
existing package, with no new capability, state, or ownership change. They
exist as:

- runnable examples of the ticket schema (see
  `brix_builder.tickets.TicketSpec`);
- fixtures for the deterministic two-ticket and resume integration tests in
  `tests/test_tickets.py`, which load them with `ScriptedBackend` instead of
  a real model;
- a starting point for enqueueing a real ticket against a local package
  checkout.

## Fields

| field                  | meaning                                                             |
| ---------------------- | -------------------------------------------------------------------- |
| `ticket_id`             | stable id; reused on resume, refused on a second `enqueue`           |
| `brief`                 | the scoped task given to the coder role                              |
| `package_path`          | package-relative to `--root`; `.` means the root itself is the package |
| `write_allowlist`       | glob patterns the candidate's `propose_patch` may touch              |
| `acceptance_gates`      | host gates that must pass before the ticket can reach `completed`    |
| `max_iterations`        | bounded coder/critic/gate repair loop budget                         |
| `max_actions_per_role`  | bounded per-role tool-call budget inside one iteration               |
| `context_tokens`        | working-context ceiling passed to each role                          |
| `metadata`              | free-form tags; not interpreted by the worker                        |

## Use against a real package

```sh
brix-builder --root /path/to/my-package \
  enqueue --from-file tickets/seeds/orders-open-query.json
```

Any explicit flag on the command line (`--ticket-id`, `--package`,
`--allow-file`, `--gate`, `--max-iterations`) overrides the matching field in
the file; `brief` on the command line overrides the file's `brief` too. This
lets you retarget a seed at a different ticket id or a narrower allowlist
without editing the seed file.

Then drive it with:

```sh
brix-builder --root /path/to/my-package run-ticket seed-orders-open-query
brix-builder --root /path/to/my-package inspect-ticket seed-orders-open-query
brix-builder --root /path/to/my-package export-proposal seed-orders-open-query /tmp/proposal.json
```

## Core-package seeds (fresh scaffolds)

Point `--root` at the matching empty scaffold under `packages/`:

| seed | `--root` |
| ---- | -------- |
| `math-abs-fn.json` | `packages/brix.math` |
| `core-id-newtype.json` | `packages/brix.core` |
| `time-epoch-seconds.json` | `packages/brix.time` |

```sh
cd tools/brix-builder
./scripts/run-local.sh --root ../../packages/brix.math \
  enqueue --from-file tickets/seeds/math-abs-fn.json
./scripts/run-local.sh --root ../../packages/brix.math \
  --critic-model mlx-community/Qwen3.5-4B-MLX-4bit \
  loop
```
