# Erratum 0001 — Normative formulas for MatchDigest and SupportRef

**Status:** proposed (awaiting Tony's ruling)
**Filed by:** Lane 5 (rt + delta ABI)
**Affects:** Part III §3 (surface identity and reference types), §9 (error
edges), §11 (provenance as a relation); Appendix A (sealed schemas);
Appendix G (canonical encoding).
**Conformance IDs:** I.1 (incremental = full recompute), I.2 (deterministic
identity), I.3 (support dynamics), I.10 (protocol lifecycle).

## The ambiguity

The spec names two engine-internal identities but gives no byte-level
formula for either, while simultaneously requiring that the incremental
engine (`brix-rt`) and the reference evaluator (`brix-oracle`) produce
**bit-identical** provenance answers under differential fuzz (I.1, I.2):

- `MatchDigest` — appears in `RuleError(rule, site, partialMatch:
  MatchDigest, error, atRevision)` (Part III §9, Appendix A) and is the
  natural key for one rule match. Part III §11's `Support(edge, rule, match,
  atRevision)` names a `match` component with no stated construction.
- `SupportRef` — appears in `KeyConflict(..., supports: Set<SupportRef>,
  ...)` (Part III §8, Appendix A). Part III §3 lists it among identities that
  are "engine-internal; their properties are observable only through sealed
  provenance relations" — i.e. their *observable* behaviour is specified, but
  their byte encoding is not.

Part III §3 gives explicit `Hash(...)` formulas for `NodeId`, `EdgeId`, and
`ClaimId`, and Appendix G fixes the encoding for everything that feeds a
hash. It is silent on `MatchDigest` and `SupportRef`. Because two conforming
implementations must agree on `Set<SupportRef>` membership *bit-for-bit*
(the `KeyConflict` row is canon-encoded and compared), the encoding cannot be
left implementation-defined without an interop hazard: the oracle and the
engine could each be internally consistent yet disagree on a settled dump.

## Why it cannot be guessed

The `match` component could reasonably be (a) a digest over the rule's
bound variables, (b) a digest including the rule's read-set edges, or (c) an
opaque per-run counter. Only (a)/(b) are retry- and machine-stable; (c)
fails I.2. Within (a)/(b), the *order* and *domain tag* of the encoded
bindings change the bytes. This is exactly the kind of choice CONTRIBUTING.md
says becomes an erratum rather than a guess, because both lanes that must
agree (oracle, rt) are being built in parallel against the same spec text.

## Proposed ruling

Add to Part III (or Appendix G) the following, versioned under `canon/1`:

1. **MatchDigest.** For a match of rule `R` binding variables
   `v_1..v_n` to canonical values, let the *binding record* be the canonical
   record `{ name_i : canon(value_i) }` sorted by binding-name bytes
   (Appendix G "records/rows"). Then

   ```
   MatchDigest = Hash(value domain, canon(R) ++ canon(binding_record))
   ```

   where `canon(R)` is the rule's stable identity encoding and `Hash(value
   domain, ·)` is the existing `Domain::Value` digest. Only the *bound*
   pattern variables enter — not read-set edges — so that two matches
   producing the same bindings via different join orders (I.1's "any physical
   plan") collide, which is required for support de-duplication.

2. **SupportRef.** A support instance is the triple `(edge, rule, match)`:

   ```
   SupportRef = Hash(value domain, canon(edge_id) ++ canon(R) ++ canon(MatchDigest))
   ```

   This makes `SupportRef` a pure function of the provenance triple, so
   `Set<SupportRef>` is order-independent and machine-independent, and two
   independent supports of the same edge (distinct matches) get distinct
   refs — the property I.3 ("shared supports removed in any order converge")
   and I.5 (`KeyConflict` candidate/support sets) rely on.

Both formulas reuse the single existing hash domain (`Domain::Value`) and the
single serializer (`brix-canon`); neither introduces a new canon rule, so
this does **not** require a `CANON_VERSION` bump — it fixes *which bytes* get
fed to the existing `value` domain, which was previously unspecified.

## Provisional implementation

`brix-rt` implements exactly the above (see
`crates/brix-rt/src/ids.rs`: `MatchDigest::of`, `SupportRef::of`) so this
lane is unblocked, clearly marked as tracking this erratum. If the ruling
differs, the change is localized to those two constructors and the
`sorted_bindings_canon` contract feeding them; no ABI shape changes.
