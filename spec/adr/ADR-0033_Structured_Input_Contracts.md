# ADR-0033 — Structured inputs and composite function contracts

Status: **Proposed implementation**, 2026-09-25. Follows the authorized
usability milestone after ADR-0032. The encoding and limits below describe
the implementation submitted for review, not separate semantic ratification.

Extends the finite-decision profile from ADR-0030 and the external-input
boundary from ADR-0031. Supersedes ADR-0032's refusal of nominal function
annotations only for the fully validated schemas described here.

## User-visible behavior

A program can declare an external `Order` record containing another record
or a sum, and pass it to a helper with an `Order` contract. Validation checks
the entire value, not only its nominal name. The same source and snapshot
work across `check`, `run`, `why`, `whynot`, `audit`, and `verify`.

```brix
config Destination = Domestic | Export(Str)
config Order = { units: Int, destination: Destination }

input order: Order
fn enough(o: Order): Bool = o.units >= 10

propose accept() priority 10 when enough(order) = true
propose hold() priority 100 when true = false
commit decision from (accept, hold)
```

This milestone does not add mutable worlds, time advancement, recursion,
project commands, or a scenario-testing language.

## Transport boundary

`brix.input@1` continues to accept precisely its existing scalar transport.
`brix.input@2` adds records and sums, using the CLI's tagged value shape:

```json
{
  "schema": "brix.input@2",
  "values": {
    "order": {
      "type": "record",
      "nominal": "Order",
      "fields": [
        {"name": "units", "value": {"type": "int", "value": "12"}},
        {"name": "destination", "value": {
          "type": "sum", "nominal": "Destination", "variant": "Domestic", "args": []
        }}
      ]
    }
  }
}
```

The decoder rejects duplicate JSON keys at every object level, duplicate
record field names, unknown fields, malformed scalar values, trailing
garbage, and exhausted limits. It must detect duplicates before values enter
an ordinary map. Property ordering does not affect acceptance: neither
`schema` nor `type` needs to appear first.

Records canonicalize fields by identifier; sum arguments preserve positional
order. Shards remain disjoint, never overrides. A v1 scalar shard can be
combined with a v2 structured shard. Transport version does not distinguish
otherwise identical admitted values.

Existing file, aggregate, identifier and string limits remain enforced.
Structured decoding also bounds nesting, total value nodes and container
width before the governed recursion or allocation.

Default structured limits are 32 tagged-value levels, 4,096 value nodes per
shard, and 256 fields or arguments per container. JSON transport wrappers
have a separate bounded allowance so they do not consume tagged-value
levels. Existing limits remain 1 MiB per file, 16 files, 4 MiB in aggregate,
256 top-level inputs, 64-byte identifiers, and 64 KiB string values.

## Schema and contract boundary

The first schema subset is closed, acyclic, and nongeneric. Fields and sum
payloads may contain `Int`, `Bool`, `Str`, or another admitted nominal type.
Unsupported recursive/generic schemas, unknown type references and graded
schema fields are rejected explicitly. Outer helper `@Derived` contracts
retain ADR-0032 semantics.

Only definitions transitively reachable from nominal input declarations or
helper contracts participate in this schema table. Existing unannotated
configuration usage is not silently reinterpreted as a schema contract.
Schema count, dependency depth and total components are bounded.

The schema table allows at most 128 definitions, 32 nominal dependency
levels, and 1,024 total record fields and sum payload slots. Shared schema
definitions count once; every dependency path must satisfy the depth limit.

One validation implementation checks both external inputs and helper
argument/return boundaries. It requires the exact nominal identity, exact
record field set and field types, and exact sum variant, arity and payload
types. This includes values constructed by Brix source before crossing a
helper contract. Diagnostic paths identify the failing nested component.

Invalid external inputs prevent runtime construction. A failed helper
contract yields `Unknown` with no committed decision. No external value or
helper acquires `Audited` or `Proven` merely by passing a schema check.

## Identity and replay

Scalar value encodings retain their existing ordinals and bytes (`Int = 0`,
`Bool = 1`, `Str = 2`). Structured input values append `Sum = 3` and
`Record = 4`, including nominal names and recursively encoded payloads or
sorted named fields. All encoding uses `brix-canon`.

An optional `brix.l3.finite-decision.schemas@1` program frame binds every
reachable schema definition, including field and payload types. It is absent
from plans without composite input/helper contracts. Existing scalar-only
program, snapshot, and context identities therefore remain unchanged.

The schema frame follows the existing config declarations and precedes the
input declaration frame. It writes the schema count, then each nominal name
and body in name order. A sum body uses ordinal `0`, followed by its variant
count and variants in declaration order; each variant writes its name,
payload count, and positional payload types. A record body uses ordinal `1`,
followed by its field count and sorted field-name/type pairs. Schema types
use ordinals `Int = 0`, `Bool = 1`, `Str = 2`, and `Named = 3`; the last writes
the referenced nominal identifier. Composite input declarations append
`Sum = 3` and `Record = 4` to the existing declaration type encoding, each
followed by its nominal identifier.

Changing a reachable schema changes the program pin even when one particular
decision is unaffected. Changing a supplied nested value changes the snapshot
and context identities. Audit verification reconstructs both the schema and
helper tables from caller-supplied source and validates caller-supplied
inputs; a bundle cannot introduce trusted schema metadata.

## Acceptance

- The order-policy example passes the complete six-command CLI workflow.
- Malformed nested fields, variants, payloads, duplicate keys/fields and
  exhausted bounds fail closed.
- Malformed source-constructed values cannot pass annotated helper contracts.
- Record field ordering and disjoint shard ordering preserve identities.
- Changed source schemas, helpers or input values reject an earlier audit.
- Alpha.2/alpha.3 scalar workflow pins and frozen vectors remain unchanged.
- Workspace tests, formatting, Clippy, canonical cross-checks, TCB dependency
  checks and semantic-law traceability pass before the milestone PR.
