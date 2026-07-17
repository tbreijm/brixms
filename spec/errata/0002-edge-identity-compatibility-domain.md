# Erratum 0002 — How the relation compatibility domain enters EdgeId

**Status:** proposed (awaiting Tony's ruling)
**Filed by:** Lane 5 (rt + delta ABI)
**Affects:** Part III §3 (surface identity), Appendix G (canonical encoding,
"relation tuples" and "entity keys" clauses), Part XXVIII §28.3 ("identity
compatibility domains survive semver except through declared migrations").
**Conformance IDs:** I.2 (deterministic identity).

## The ambiguity

Part III §3 gives `EdgeId = Hash(relation compatibility domain, canonical
role tuple)`. Appendix G restates it as "relation tuples: relation
compatibility domain **digest** + roles sorted by role name" (and, for nodes,
"entity keys: type compatibility domain **digest** + key fields in
declaration order"). Two things are underspecified:

1. **What the "compatibility domain digest" is a digest _of_.** Part XXVIII
   §28.3 says compatibility domains "survive semver except through declared
   migrations," so the domain is clearly a schema-versioning artifact keyed
   to the relation's *identity*, not merely its current source name. But the
   spec never says whether the digest is over the relation's fully-qualified
   name, over a declared `compatibility domain N` annotation, or over some
   structural fingerprint of its role schema.

2. **Whether the domain enters as a hash _domain-separator_ or as _payload_.**
   `brix-canon`'s `Digest::of(domain, payload)` already provides one axis of
   domain separation (`Domain::Edge`). The spec's "relation compatibility
   domain" is a *second, per-relation* separation that must live inside the
   payload, but the encoding of that payload prefix is unstated.

Without a ruling, two implementations could hash byte-differently while both
reading the spec faithfully, breaking I.2 across implementations (the
oracle/engine differential compares `EdgeId`s inside every settled dump).

## Why it cannot be guessed

The choice interacts with migrations: if the domain is the raw
fully-qualified name, then any rename is an identity break (which may be
intended — "declared migrations" — or may not). If it is a separate declared
domain token, renames can preserve identity. This is a semantics decision
about identity stability across program evolution, squarely Tony's to rule,
not a lane's to assume.

## Proposed ruling (for the v0 blitz)

Until `brix-ir` carries declared compatibility-domain tokens, rule:

- the relation compatibility domain is realized as the relation's
  **stable fully-qualified name**, canon-encoded as an identifier (Appendix G
  identifier rules, NFC), written as the payload prefix ahead of the
  canonical role tuple, all fed to the existing `Domain::Edge` digest:

  ```
  EdgeId = Digest::of(Edge, canon_ident(relation_fqn) ++ canon(role_tuple))
  ```

- a future `compatibility domain` declaration, when `brix-ir` gains one,
  overrides the fqn as the prefix source; adopting it for an existing
  relation is a *declared migration*, not an ambient rename.

This keeps one serializer and one hash domain, matches Appendix G's
"digest + roles" shape (the "digest" being the canon-encoded domain token,
here the fqn), and defers the migration-token machinery to when the surface
that declares it exists — recorded as deferred-not-dropped.

## Provisional implementation

`brix-rt` implements exactly this (see `crates/brix-rt/src/graph.rs`,
`edge_id`): relation name canon-encoded, then the role tuple, into
`Domain::Edge`. If the ruling adds a declared domain token, the change is
localized to `edge_id` and its callers; the delta-ABI shape is unaffected
(it transports the already-resolved `EdgeId`).
