# Erratum 0006 (proposed): A9-3 host-scoped named-query profile

Status: **Proposed implementation ruling** (2026-11-01). This draft resolves
the A9-3 grammar, scope, context-binding, cursor, and initial capability-profile
details left open by ADR-0046 and the Alpha.9 release contract. It does not
change existing program or canonical identities; it adds a versioned query
surface and versioned host-query artifacts.

Affected contracts: `spec/Alpha9_Release_Contract.md` §5, ADR-0046 §§3.2–3.5,
and `spec/Context_Contract.md` §§9–10. A9-3 conformance IDs: `A9-3-NQ-01`
(declaration and exact typed arguments), `A9-3-SCOPE-01` (pre-operator scope),
`A9-3-PIN-01` (pinned revision), `A9-3-CURSOR-01` (bound continuation), and
`A9-3-RO-01` (read-only execution).

## Proposed ruling

1. A named read query uses the additive declaration form:

   ```brix
   query catalogue(kind: Str) = select { id: c.id, kind: c.kind }
     from c in catalogue_entries where c.kind == kind
   ```

   It is a new top-level item, not a `rel derived` declaration. Its parameters
   are ordered named values with a closed initial scalar type set `Str`, `Int`,
   and `Bool`. Execution requires exactly the declared argument names and
   types. Values bind as typed data directly into a cloned query plan; callers
   cannot supply expression source. The result schema is the existing
   relational lowerer's typed output schema.

2. The initial host scope profile is exactly
   `brix.host-scope.partition@1`. The trusted host config maps each authorized
   base input relation either to one declared `Str` partition field or lists it
   in the explicitly approved global-relation set. An input relation may not
   be both. The partition value is the tenant in the independently validated
   V2 context. Derived relations are recomputed through the existing relational
   DAG after base rows are filtered. A query is refused when any transitive
   base input lacks one of those explicit profile entries. Relations outside
   the query's transitive dependency closure do not affect admission.

3. The first profile requires Production mode, no effective typed assumptions,
   kernel/regime/axiom profiles `none`, and observation/redaction profiles
   `none@1`. The V2 `capability_view` is exactly
   `<profile-id>:<profile-digest-hex>`. The profile digest canonically binds the
   profile ID, sorted relation-to-field map, sorted global allowlist, and all
   six host-owned query caps. The host caps page rows, scanned input rows and
   bytes, evaluation work and matches, and total output rows; request values
   may only reduce those caps. The V2 step budget must be at least the
   configured evaluation-work cap.

4. The context must contain a `program` artifact reference whose digest is the
   world's linked-program digest, and a `world.revision@1` artifact reference.
   The latter is the `Domain::Value` digest of the brix-canon preimage tagged
   `brix.world-revision-artifact@1`, containing world ID, revision sequence,
   revision digest, and program digest in that order. The V2 context validator
   resolves both artifact preimages before a private authorized scope handle is
   issued. Query execution compares those validated references with the actual
   pinned snapshot and linked program.

5. The initial profile refuses every dependency on a decision-winner relation.
   A future profile may admit decisions only after it proves that each winner
   key preserves the same partition before selection. Undeclared cross-partition
   dependencies are refused. Explicitly approved global inputs are the only
   exception.

6. A scoped query resolves HEAD once for a first page. A continuation supplies
   its pinned sequence; it remains on that revision after HEAD advances. A new
   `brix.scoped-query-cursor@1` envelope binds query identity, program digest,
   canonical typed-argument digest, authorized scope identity, validated
   context ID, world ID, revision sequence and digest, ordering version, and
   continuation position. The cursor is not an authorization credential;
   every request revalidates host scope and all binding fields. Legacy
   alpha.8 cursors are refused for named queries.

7. Scoped execution scans only the transitive base inputs, filters partitioned
   rows before any relational operator, and evaluates an ephemeral instance of
   the existing `WorldNetwork`. It creates no temporary input relation in the
   world and persists no network/cache/checkpoint state. Read-only open loads metadata and base roots without global network
   hydration or replay, file writes, cache access, or writer-lock acquisition. Missing or compacted revisions fail explicitly.

## Compatibility and exclusions

The query item is absent from alpha.8 modules, so old AST and program identity
vectors remain byte-for-byte unchanged. The query's containing source still
contributes to the ordinary program graph digest. The added query identity is
canonically derived from that program digest and declared query name. This first
profile does not accept list/record query parameters, custom semantic context
profiles, simulation mode, dynamic policy lookup, decision-winner dependencies,
or implicit/global relations.

These rulings are an implementation proposal for Alpha.9 A9-3. They do not
retroactively ratify the illustrative source fragment in ADR-0046.
