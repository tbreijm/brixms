# Alpha.9 collection ordering and truncation

A9-6 introduces a versioned collection evaluator and an explicit bounded-output
projection. The three-argument `collect(value, sort_key, limit)` still rejects
cardinality overflow atomically. It continues to return a list.

```brix
rel input child: { id: Str, parent: Str, position: Int, label: Str } key id
rel derived parent_children = select {
  parent: c.parent,
  all_labels: collect(c.label, c.position, 10000),
  preview: collect(c.label, c.position).take(3)
} from c in child group by c.parent
```

`preview` has the structural type `{ items: List<Str>, truncated: Bool }`.
The non-negative integer bound may be zero. `take(0)` produces an empty list
and `truncated=true` for an existing nonempty group. When the final child is
removed, that grouped row disappears under normal grouping semantics.

The result keeps the first `n` supported children. The truncation flag counts
child identities, so equal projected values from distinct keys retain their
multiplicity. The operator retains the complete ordered support, including the
tail; removing a retained child promotes the next child. An independent
support, work or storage budget still aborts the entire batch. Truncation does
not permit dropping unexamined input or ignoring a budget failure.

## Versioned ordering

Typed ordering compares sort values, then the child's declared typed primary
key. Strings compare lexicographically by UTF-8 bytes; composite keys compare
components in declared order. A qualified source identity resolves equal keys
across relations. Join children use ordered structured pairs of child identities;
distinct and grouped children use their typed stable output/group identity.
Decision-row children use the complete declared `per` tuple.

Input `WorldKey` bytes remain opaque. The ordering key comes from actual source
row values under the declared schema, including after projection, cold restore
or an external alias key. No provenance hash substitutes for a missing stable
identity. Unsupported, ambiguous or duplicate identities fail before HEAD is
published. Typed persisted keys require the exact versioned structure; malformed
keys and incomplete accumulator entries are rejected. Discovery and validation
have explicit depth/work bounds.

## Persistence and compatibility

New worlds with materialized collections record evaluator
`brix-world-net@2`. Its network fingerprint includes the typed-order marker;
its validity key uses `brix.world.net-lowering@3`. The existing state-value
encoding already supports the structured keys and remains `@3`.

Saved alpha.8 worlds retain `brix-world-net@1` and their original comparator.
Opening them does not rewrite their history into new semantics. New typed
semantics require a new world (or a separately specified migration); changing
an existing world's execution profile through the constructor is rejected.
Worlds without materialized collections keep their existing profile and
fingerprint. `collect(...).take(...)` is rejected under the legacy evaluator.

Portable audits and decision certificates carry the actual producing profile.
The verifier selects the supported implementation and requires typed evaluator
metadata to be bound into revision identities. A frozen legacy collect audit,
produced at the preceding foundation with deliberately different key order,
continues to verify without altering its bytes or independent pins.

The low-level `WorldNetwork::from_program` remains a legacy compatibility
constructor. Explicit callers use `from_program_with_collect_ordering` with
`CollectOrdering::TypedKeyV1`. Session creation chooses the current profile for
collections; explicit session constructors can reproduce a supported legacy
profile. Scoped named reads use current typed ordering and cursor ordering
version 2, so earlier development cursors are rejected rather than reinterpreted.

Typed CLI output and portable verification validate the structural record,
including nested lists/records, with fixed depth/container/work bounds. The
query source carries the truncation bound into program identity and evidence;
output exposes the retained items and boolean rather than omitted members.
