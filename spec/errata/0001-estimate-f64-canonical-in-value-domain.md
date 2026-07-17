# 0001 — Is `Estimate<F64>` `Canonical`, given floats are banned from key/identity positions?

**Lane:** ir
**Status:** proposed, awaiting ruling
**Affected conformance:** Appendix I.14 (Policy determinism), Appendix I.8 (Numerics),
Appendix E `Key` judgment

## The tension

Three normative statements do not obviously reconcile:

1. Part III §3: "Canonical encodings are normative (Appendix G); **floats are
   inadmissible in keys**."
2. Part IV §5: "`Rel<S>`... comparable and hashable only when `S: Canonical`
   (**which excludes raw floats** — wrap them or keep such relations out of
   identity positions)."
3. Part XII §2: policy declaration typing rules — "the candidate row and
   suggestion row **require `S: Canonical`**" — and the very next clause of the
   same paragraph: "scores are `Estimate<F64>`, honest about approximation by
   construction." A policy's `suggestion` row is exactly `{ vehicle: Vehicle;
   score: Estimate<F64> }` in the worked example (Part XII §2), and its
   `candidatesDigest` (Part XII §3) is a digest of the candidate row, which the
   same section requires to be `Canonical`.

Read literally, (1) and (2) say a row containing an `F64` (directly or via
`Estimate<F64>`) is *not* `Canonical`, while (3) requires exactly such a row to
*be* `Canonical` so it can be digested into `candidatesDigest`. Appendix E's
`Key` judgment ("`Γ ⊢ T : Canonical` for every type in any key position") only
governs *key* positions, not general `S: Canonical` bounds used for hashing a
`Rel<S>` value or a digest payload — so the apparent conflict may be resolved
by distinguishing two different uses of `Canonical`, but the spec does not say
this explicitly anywhere, and Appendix G's own text is what motivates the
distinction:

> Appendix G: "floats: NOT encodable in `canon/1` **key positions**; canonical
> row order for **aggregation** uses the non-float remainder of the row, with
> full-row totalOrder tiebreak defined over canonicalized bit patterns for the
> float components."

This shows App. G already has a defined, deterministic byte encoding for
floats *outside* key positions (the totalOrder tiebreak used for aggregate row
ordering). It is a short step from there to "floats have a canonical
value-domain encoding but are barred specifically from key positions" — but
that step is not spelled out as a general rule, only as an aggregation-ordering
special case, and Part IV §5's "which excludes raw floats" reads as an
unqualified blanket exclusion.

## Why brix-ir cannot leave this unresolved

`crates/brix-ir/src/types.rs` implements the Appendix E `Key` judgment
(`check_key_canonical`) and needs a second, distinct check for the general
`S: Canonical` bound used by `Rel<S>` hashing and Part XII §2's candidate/
suggestion rows (`check_value_canonical`). Whether `Estimate<F64>` passes the
second check is exactly the fork in the road: reject it, and the flagship's
own worked policy example (`AssignVehicle`) is inexpressible; accept it, and
"which excludes raw floats" needs a narrower reading than its plain text.

## Proposed ruling

Adopt the two-domain reading implied by Appendix G's own totalOrder clause,
made explicit:

- **Key domain** (`Γ ⊢ T : Canonical` in a key position — Appendix E `Key`
  judgment; entity/relation key fields; anything feeding `NodeId`/`EdgeId`):
  floats are **unconditionally excluded**, full stop, including inside
  `Estimate<T>` or any other wrapper. This is what Part III §3 states and it
  should stay absolute — key identity must never depend on IEEE
  bit-pattern/NaN-canonicalization edge cases.
- **Value domain** (`S: Canonical` used only to make a `Rel<S>` hashable/
  comparable for **non-key** purposes — `Rel<S>` equality checks, provenance
  logging, and digesting a payload like `candidatesDigest`): floats **are**
  admissible, encoded per Appendix G's existing totalOrder/bit-pattern rule
  (NaN canonicalized to one bit pattern, per Part V §8). `Estimate<T>` is
  `Canonical` in the value domain whenever `T` is (structurally: encode
  `value`, `error`, `confidence`, `method` in field-name order like any other
  record).
- Part IV §5's "`S: Canonical`... which excludes raw floats" is read as
  shorthand for "excludes raw floats **from key/identity positions**," which
  is the sentence's actual context ("keep such relations **out of identity
  positions**"). The erratum's ruling, once merged, should tighten that
  sentence's wording so a future reader does not hit the same fork.

## Conformance IDs affected

- Appendix I.14 (Policy determinism): `(version, snapshot, candidatesDigest,
  seed)` reproduces the identical decision — requires `candidatesDigest` to be
  computable over a row containing `Estimate<F64>`-shaped fields; this ruling
  is what makes that digest well-defined.
- Appendix I.8 (Numerics): NaN canonicalization / totalOrder sorting fixtures
  should include a case with `Estimate<F64>` inside a digested `Rel<S>` row.
- Appendix E `Key` judgment: fixture should include a rejected program that
  puts `Estimate<F64>` (or bare `F64`) in a `key(...)` position, to pin the
  key-domain side of the ruling.

## What brix-ir does until ruled

`crates/brix-ir/src/types.rs` implements the ruling above as
`check_key_canonical` (strict, floats always rejected) and
`check_value_canonical` (floats admitted). Both are unit-tested
(`estimate_f64_is_value_canonical_but_not_key_canonical`). If the ruling comes
back different, only those two functions and their doc comments need to
change — no other lane depends on this distinction yet.
