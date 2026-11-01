# Alpha.9 named query profile

This implements A9-3 on the reviewed #52/#53/#59 + A9-1/A9-2 release
foundation. It does not complete alpha.9: request evidence contexts (A9-4),
string admission (A9-5), collect ordering/truncation (A9-6), and scale
qualification (A9-7) remain separate release requirements.

## Declare and call

```brix
rel input catalogue_entries: { id: Str, tenant: Str, kind: Str } key id
query catalogue(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue_entries where c.kind == kind
```

The first query profile supports `Str`, `Int`, and `Bool` parameters. Missing,
extra, unsupported, or incorrectly typed arguments fail explicitly; values
are bound as typed data. The result schema is inferred from the admitted
relational query. Query declarations do not create persisted input relations.

```sh
brix world init ./world catalogue.brix --json
brix world query-named ./world catalogue \
  --host-scope ./host-policy.json --args '{"kind":"book"}' --limit 20 --json
```

Use the full `brix.host-query-policy@1` configuration shown in the
[Python SDK README](../../bindings/python/README.md). Partition relation names
are qualified (for this file, `catalogue::catalogue_entries`). Set the tenant,
principal, namespace, authorized base relations, validity intervals and read
caps in the host policy. A relation is either partitioned by a declared `Str`
field or explicitly authorized as global. Missing authorization fails closed.

The embedding host must authenticate callers and select the policy and world
itself. The policy file is a host input, not a request parameter to forward
from an unauthenticated client. There is no automatic authentication layer.

## Transport boundary

`serve --stdio` refuses `world.query-named`. An embedding host can enable
`serve --stdio --trusted-host` when it owns the channel. That channel accepts
`{dir, name, args, host_policy, rev?, limit?, cursor?}` for `world.query-named`.
The host selects `host_policy` separately from untrusted query `args`.
Other server methods retain their existing behavior; owning the channel also
means controlling which methods an external client may invoke.

Python's immutable `HostQueryPolicy` copies and validates configuration at
construction. `BrixClient(host_query_policy=policy)` starts the trusted-host
channel; `world_query_named` always supplies the stored policy and offers no
per-call policy override. Do not expose the raw client to untrusted callers.

The adapter constructs a canonical ContextModelV2 and validates it using #59's
validator. Its artifact references resolve to the actual saved program and
pinned world revision. The scope handle is private and verifier-issued; a
cursor, copied digest or caller-provided flag cannot construct one.

## Read and continuation semantics

A first read resolves HEAD once unless `--rev` is supplied. Continuations use
the revision in their bound cursor, even after HEAD advances. The cursor binds
query, program, typed arguments, scope, V2 context, world, exact revision and
ordering version. Changing a binding or supplying a legacy cursor is rejected.
A cursor is not an authorization credential.

Only the named query's base dependencies are loaded into its ephemeral
network, after applying partition filters. Joins, groups and absence therefore
operate on authorized rows. Decision dependencies are refused in this initial
profile; a globally selected winner is never reused as a tenant-local winner.

Reads do not acquire the writer lock or create caches, checkpoints, input
facts or revisions. Budgets bound scanned rows/bytes, evaluation work/matches,
output rows and page size. Scanning another tenant's row counts toward the
scan budget, even though that row cannot enter query evaluation. The storage
layer may decode one node before the query byte counter can reject its tuple;
that allocation remains bounded by the existing 64 MiB node limit. The initial
implementation evaluates an ephemeral query per page; it does not promise
indexed or incremental execution for parameterized reads.

JSON results use `brix.world.named-query-result@1`, with typed `rows`, query,
context and scope identities, revision sequence/digest, `has_more`, and
`next_cursor`. Existing unscoped world queries and frozen persisted encodings
keep their prior contract.
