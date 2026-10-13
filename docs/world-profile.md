# Persistent world expressions

The world profile evaluates pure helpers, relation projections and predicates,
and per-entity proposal guards and values. Linked packages provide declarations;
the root program owns the decisions to execute.

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
