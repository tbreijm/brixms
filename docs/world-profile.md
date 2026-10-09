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
