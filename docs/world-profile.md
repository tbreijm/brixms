# Persistent world expressions

The world profile evaluates pure helpers, relation projections and predicates,
and per-entity proposal guards and values. Linked packages provide declarations;
the root program owns the decisions to execute.

## Policy-backed dispatch requests

The library API admits one policy workflow from ADR-0048: `qty>=1` for a
record with `id: Str` and `qty: Int`, addressed by its string primary key in
a named base relation. Order IDs follow the journal codec's token restriction:
nonempty, with no spaces, newlines, `;`, or `=`. This is a runtime predicate check, not a kernel proof
or a claim about stock, permissions, delivery, or the truth of external data.

```rust,ignore
use brix_kb::world::policy_evidence::{DispatchEvidence, verify_dispatch_evidence};
use brix_lower::evidence::DISPATCH_POLICY;

let revision = world.current_revision();
let expected = world.dispatch_target(revision, "root::orders", "o1", DISPATCH_POLICY)?;
let evidence = world.request_dispatch(revision, "root::orders", "o1", DISPATCH_POLICY)?;
let receipt = world.use_dispatch_evidence(
    &evidence, revision, "root::orders", "o1", DISPATCH_POLICY,
)?;

// After reopening the world, load and recheck the retained artifact.
let retained = world.load_dispatch_evidence(&expected)?;

// An independent process needs only these bytes and a separately selected target.
let portable = DispatchEvidence::decode(&retained.encode()?)?;
let independently_checked = verify_dispatch_evidence(&portable, &expected)?;
assert_eq!(receipt, independently_checked);
```

`request_dispatch` reads the immutable revision and its persisted tuple; the
application supplies neither quantity nor digest. A successful request atomically
stores `revisions/<revision>.policy-<target-digest>.evidence` under the world writer
lock. The JSON artifact (`brix.world.dispatch-evidence@1`) contains the target,
revision record, dispatch certificate, journal, and Merkle lookup path. Requests
are idempotent: an existing artifact is rechecked, and corruption is an error.
Refused requests do not publish certificates or change the revision identity.

Use requires the same world manifest digest, revision number and digest, relation,
order, and policy. Even an unrelated edit requires a new request for the new
revision. Earlier certificates remain valid historical claims; later updates,
retractions, and journal reloads do not rewrite them. The artifact carries its
lookup nodes, so independent verification and `load_dispatch_evidence` can still
check it after old trie nodes are compacted. Ordinary use through a session still
requires that the requested revision is available to that session.

The independent verifier recomputes the revision digest, verifies the lookup path
against the relation root, decodes the order's `id` and signed 64-bit `qty`, and
checks both the certificate and journal against that quantity. Cached decisions,
receipt seals and recorded authority labels cannot authorize use. A successful
check issues fresh `RuntimeCheck` evidence. Unsupported policies, missing orders,
malformed quantities, mismatched contexts, missing nodes and corrupted proofs
are refused; a well-formed quantity below one produces `Violated`.

The consumer must obtain its expected world and revision identities independently
of the artifact. The trusted base is that choice of context, the canonical
revision and tuple codecs, hash/Merkle verification, integer decoding, and the
bounded predicate checker. The artifact does not authenticate who supplied the
world or establish general context confinement. It also does not execute a
dispatch or reserve resources. This workflow is exposed through the library;
there is no additional CLI command.

## Records and variants

Nominal records and sum variants are values in world expressions and tuple
fields. Their types survive settlement, explanations, historical reads and audit
verification. For example, save this program as `atlas.brix`:

```brix
config Rec = { id: Str, amt: Int }
config Decision = Approved(Rec) | Rejected
fn positive(d: Decision): Bool = match d {
  Approved(r) => r.amt > 0
  Rejected => false
}
rel input orders: { id: Str, amt: Int, status: Decision } key id
rel derived eligible = select { id: o.id, status: o.status }
  from o in orders where positive(o.status)
decide approve for o in orders per id {
  propose yes priority 10 when o.amt > 0 = Approved(Rec { id: o.id, amt: o.amt })
  propose no priority 100 when true = Rejected
}
```

Use a payload binder such as `Approved(r)`, or `_` to ignore a payload:
`Approved(_)`. Helpers resolve names in their defining module. Qualified
constructors and patterns identify their nominal sum; ambiguous bare constructor
references are rejected with the qualified alternatives.

## Structured JSON

Structured tuple fields use the tagged value shapes of `brix.input@2` (and the
explicit numeric tags of `brix.input@4`). World nominal names are fully qualified
by module, including the root module. A record's `fields` is an array of
`{name, value}` entries; each payload is itself tagged. Lists are not admitted as
world tuple fields.

```json
{
  "schema": "brix.world.batch@1",
  "expected_base_revision": 0,
  "idempotency_key": "first-order",
  "operations": [{
    "op": "upsert",
    "relation": "atlas::orders",
    "key": "order-1",
    "tuple": {
      "id": "order-1",
      "amt": 5,
      "status": {
        "type": "sum",
        "nominal": "atlas::Decision",
        "variant": "Approved",
        "args": [{
          "type": "record",
          "nominal": "atlas::Rec",
          "fields": [
            {"name": "id", "value": {"type": "string", "value": "order-1"}},
            {"name": "amt", "value": {"type": "int", "value": "5"}}
          ]
        }]
      }
    }
  }]
}
```

A nullary outcome is
`{"type":"sum","nominal":"atlas::Decision","variant":"Rejected","args":[]}`.
Missing or unknown fields, wrong nominal types, wrong constructor arity and wrong
payload tags are errors. The decoder bounds nesting and container sizes.

`world.decisions` and `world.explain` return structured outcomes using the same
tagged shapes. Existing scalar decision values retain their string presentation.
Stored structured values use canonical typed bytes, not display strings; record
field order in JSON does not change their canonical value identity. Decision
history and independent verification retain the full nominal type and payload.

## Decisions as relations

A `decide` block exposes its settled winners as a relation with its entity key,
`candidate`, `priority`, and typed `value`. A later relation or decision can read
those rows. Every proposal in one block must have the same result type. Reserved
winner-field collisions, priorities outside the `Int` range, and dependency
cycles are rejected before execution.

```brix
decide approval for o in orders per id {
  propose yes priority 10 when o.amt > 0 = Approved(Rec { id: o.id, amt: o.amt })
  propose no priority 20 when o.amt <= 0 = Rejected
}
decide review for a in approval per id {
  propose reviewed priority 10 when true = a.value
}
```

One batch settles all affected stages before publication. Removing an upstream
winner withdraws its downstream support. Explanations identify upstream
settlements as well as supporting source facts. A decision row is a runtime
settlement, not a proof or an independently audited claim.

## Grouped aggregates

Use `count()`, `sum(expression)`, `min(expression)`, or `max(expression)` in a
grouped projection. Other projected fields must name grouping expressions:

```brix
rel input lines: { id: Str, order_id: Str, amount: Int } key id
rel derived totals = select { id: l.order_id, total: sum(l.amount),
                             least: min(l.amount), most: max(l.amount) }
  from l in lines group by l.order_id
```

Sums admit exact `Int` and `Decimal` values; floating-point sums are refused.
Overflow refuses the batch atomically. Ordinary grouping produces no row for an
empty group. Retraction of an extremum selects the next supported value.

## Absence and empty parents

Correlated absence uses one equality between an outer field and the inner
relation. The inner binding must differ from every outer binding:

```brix
rel derived missing = select { id: o.id } from o in orders
  where not exists l in lines where l.order_id == o.id
```

This fragment is acyclic: recursive dependencies, including cycles through
absence or decisions, are refused. It does not admit general nested negation or
arbitrary inner predicates. Inserting the first matching child removes the
absence result; removing its last support restores it. Removing the parent also
removes the absence result.

To retain an explicit zero count, use `count_or_zero()`:

```brix
rel derived line_counts = select { id: o.id, count: count_or_zero() }
  from o in orders, l in lines where l.order_id == o.id group by o.id
```

The first source is the parent input relation with a single-field key; the second
is the child. The correlation must use the complete parent key, and the grouping
must name that key. Exactly two distinctly named projections are admitted: the
parent key and the count. Additional filters, composite parent keys, and combined
absence/zero-count queries are refused rather than approximated.

The complete [Atlas example](../examples/world-atlas/README.md) demonstrates
these operators together across separate CLI processes.

### Using a world certificate in a finite decision

A portable world certificate proves a settlement at a named revision. It does
not prove that the input observations were true. To consume it in a finite
`validate<RuntimeOutcome>` call, pass an evidence environment alongside the
ordinary `brix.input@5` input:

```json
{"schema":"brix.evidence.env@1","runtime":["<full decision-evidence JSON object>"]}
```

Replace the placeholder with the complete JSON object returned by `brix world
evidence`. Independently configure `BRIX_EXPECT_RUNTIME_CLAIMS` with the expected
`DecisionTarget::id().to_hex()` (comma-separated for multiple targets). A trusted
`world evidence --json` issuance also returns this as `target_id`. The ID
commits the world, program, revision and digest, decision, entity, candidate,
value digest, caller, release and grants. Obtain this expectation from the
consumer's chosen context, not from an untrusted certificate's assertion.
The input file cannot supply its own trusted pin.

`run`, `why`, `audit`, and `verify` use the shared input loader. Each invocation
checks the target pin and performs complete bounded world replay before
installing runtime authority. Supply the same environment and independent pins
when verifying the finite audit bundle in a fresh process; a missing bundle or
changed target is refused. The environment identity is included in `ContextId`.

For documentary authority, use the environment's `documentary.entries` and
configure `BRIX_EXPECT_EVIDENCE_DIGEST` independently with the documentary
snapshot's canonical digest. An embedded `documentary.expected_digest` is only
a consistency check. It cannot establish trust.

Rust embedders should call `verify_decision_evidence`, then the resulting opaque
`brix_kb::world::decision_evidence::VerifiedDecisionEvidence::admit_into` on their
finite evidence registry. Construction of that handle is private to the complete
verifier. The lower evaluator's `TrustedRuntimeEvidenceProvider` interface is an
explicit host trust boundary: an embedding that implements its own provider is
responsible for its authority checks. Untrusted Brix inputs cannot implement a
provider. The retired lower-level prototype constructors always fail closed.
