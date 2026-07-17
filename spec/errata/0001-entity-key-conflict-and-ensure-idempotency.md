# 0001 — Entity key-conflict semantics and `ensure` idempotency

**Lane:** oracle (brix-oracle)
**Status:** ruled 2026-07-17
**Affected sections:** Part III §8 (Key-conflict semantics), Part VII §2
(Transactions, `ensure`), Appendix A (`KeyConflict` schema)
**Affected conformance IDs:** Appendix I.5 (Key conflicts)

## The ambiguity

Part III §8 gives four per-kind conflict rules, headed "Ground `rel`.",
"`state rel`.", "`event rel`.", and "Derived relations.". `entity` is not one
of the four headings, yet:

1. Part VII §2 says `ensure` "returns existing identity or creates it" for a
   keyed entity, but does not say what happens when a later `ensure` call
   for the same key supplies **different non-key field values** than an
   earlier call (or than a value a rule concurrently derived for the same
   entity via `keyed by (...)`, which Part III §3 explicitly allows: "A rule
   that derives a node uses `keyed by (...)`: a deterministic Skolem
   identity over the rule and its key bindings.").
2. Since `NodeId` is a hash of key fields only (Appendix G: "entity keys:
   type compatibility domain digest + key fields in declaration order"),
   two different non-key-field values under one key produce **the same
   `NodeId`** but are, as row content, two different candidate values for
   that identity's attributes — exactly the shape of a key conflict, but
   Part III §8's four sub-cases don't name which one (if any) governs it.
3. Part III §8's opening sentence is unqualified: "Every relation with
   `key(...)` obeys a per-kind conflict rule. There is never a silent
   winner." Every `entity` declares `key(...)` fields (Appendix D:
   `EntityDecl := "entity" Ident "{" FieldDecl+ "}"`, `FieldDecl := "key"?
   Ident ":" Type`), so the opening sentence's guarantee appears to bind
   entities too, even though no sub-case is written for them.

## Why this matters for the oracle

The oracle is the reference implementation of "never a silent winner." If it
picks *any* behavior for entities without a ruling, that behavior freezes at
G1. Two candidate readings:

- **(a) Ground-like:** treat `ensure`/`fresh` field disagreement as a
  transaction-time conflict (reject the second `ensure` unless it matches
  the first), analogous to Ground `rel`'s "asserting a tuple whose key
  matches a live tuple with a different complete role tuple is a
  transaction conflict."
- **(b) Derived-like:** treat any two candidate rows under one entity key —
  whether transaction-minted or rule-derived via `keyed by (...)` — as
  inputs to the same `KeyConflict` exposure Part III §8 already defines for
  "Derived relations," since a rule can target an `Entity` relation and
  transactions can too, so entities are structurally a shared surface, not
  purely ground.

## Ruling (adopted 2026-07-17)

Adopt **(b)**: extend `KeyConflict(relation, key, candidates, supports,
atRevision)` to cover `Entity` relations uniformly, regardless of whether a
candidate row originated from a transaction (`ensure`) or a rule
(`keyed by (...)`). Rationale: `KeyConflict`'s schema is already keyed by
`relation` generically, non-arbitration ("expose the disagreement, not
arbitrate it") is stated as the kernel's general default rather than a
Derived-only default, and this is the only reading under which "never a
silent winner" holds for entities without inventing a fifth, unwritten
transaction-conflict rule specific to non-key entity fields.

Corollary for `ensure`: disagreeing `ensure` calls for one key, including calls
staged by one transaction, contribute competing entity candidates and surface as
`KeyConflict` at settlement. Repeating the complete same row is idempotent.

## Implementation alignment

`crates/brix-oracle/src/eval.rs::refresh_live` runs the same key-conflict
grouping-and-withdrawal pass for `Entity` relations as for `Derived`
relations. Within one oracle transaction, two disagreeing `ensure` calls for
the same key are staged as separate candidate rows and surface as an ordinary
`KeyConflict` at the next settlement, the same as a cross-transaction
disagreement.
