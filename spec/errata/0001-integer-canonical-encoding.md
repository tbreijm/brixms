# 0001 — What byte layout does "minimal-length big-endian two's complement with sign byte" actually mean?

**Lane:** canon (brix-canon)
**Status:** ruled 2026-08-04
**Affected conformance:** Appendix G (Canonical encoding and identity), Appendix I.8
(Numerics), and every identity derived through `canon/1`

## The tension

Appendix G's integer bullet is one clause:

> integers: minimal-length big-endian two's complement with sign byte; Nat unsigned;

Read completely literally, that describes a **length-free** encoding: the
minimal big-endian two's-complement bytes of the value, and nothing else. That
layout is well-defined and unambiguous — and it is **not order-preserving**.

Compare the encodings of `2` and `256` under the literal reading:

```
2    -> 0x02
256  -> 0x01 0x00
```

Byte-lexicographically `0x01 0x00 < 0x02`, so `256` sorts *before* `2`. Numeric
order and byte order disagree.

That matters because the spec relies on them agreeing, in at least three places:

1. The numerics section states **"`Ord` = Appendix G byte order"** for the
   static-Decimal-context ordering rules.
2. Part V §8: stock aggregates over floats reduce in **canonical row order**
   ("Appendix G ordering"), which is a byte order over encoded rows.
3. Appendix G's own collection rules — "Set/Map entries sorted by canonical
   element/key bytes" — define set and map *identity* in terms of byte order. If
   byte order does not track value order, then `{2, 256}` and `{256, 2}` still
   canonicalize identically (sorting is sorting), but the resulting sequence
   is not the sorted-by-value sequence a reader would reconstruct, and any
   range query or ordered merge over canonical bytes is wrong.

So the literal reading of the integer bullet contradicts the ordering guarantee
the rest of the spec depends on. Appendix G is labelled a *normative sketch*;
this is one of the places the sketch is genuinely underdetermined rather than
merely terse.

## Why brix-canon cannot leave this unresolved

`brix-canon` is the G0 freeze artifact — every hash, identity, log entry, and
aggregation order in the toolchain flows through it, and `vectors/canon_vectors.json`
freezes the bytes permanently. The encoding had to be chosen before the freeze,
and it was chosen to satisfy the ordering guarantee. This erratum records the
ruling that authorizes that choice, which until now lived only in a Rust module
comment (`crates/brix-canon/src/int_codec.rs`).

## Ruling (adopted 2026-08-04)

Read "minimal-length" as a constraint on the **magnitude**, and require the
encoding as a whole to be order-preserving. Concretely:

### Unsigned (`Nat`, uint)

```
[len] ++ magnitude_be
```

- `magnitude_be` is the minimal big-endian magnitude — no leading zero byte.
- `len` is the number of magnitude bytes, as a single byte.
- The value `0` encodes as the single byte `0x00` (`len = 0`, empty magnitude).

Order-preserving because a longer magnitude always denotes a larger value, so
the length byte dominates the comparison; within one length, magnitudes compare
correctly as plain big-endian bytes.

### Signed (`Int`)

```
[0x80 + sign*len] ++ magnitude
```

- Zero is the single byte `0x80`.
- For a positive value the category byte is `0x80 + len` and the magnitude is
  written plain.
- For a negative value the category byte is `0x80 - len` and the magnitude is
  written **bitwise complemented**.

Order-preserving in both directions. Positives get categories strictly above
`0x80`, ordered by magnitude length; negatives get categories strictly below it,
and because a *larger* negative magnitude means a *smaller* value, the category
descends as the magnitude grows. Complementing the magnitude reverses the
within-length comparison to match.

### Consequences

- **Prefix-free.** Both layouts are self-delimiting: the first byte determines
  exactly how many bytes follow. `Decimal` (erratum 0002) reuses this for its
  exponent field, and every length prefix in `canon/1` — for bytes, strings,
  and collection counts — is itself an unsigned integer in this encoding, so
  the whole format inherits self-delimitation from here.
- **Maximum magnitude is 16 bytes** (128-bit values). A longer magnitude is a
  decode error, not a wider integer.
- Two's complement does *not* appear in the final layout. The literal phrase in
  Appendix G is superseded: what the spec wanted was a minimal-length,
  sign-carrying, order-preserving integer encoding, and that is what this is.

## Conformance IDs affected

- **Appendix G**: the integer bullet should be rewritten to state the layout
  above, or to cite this erratum. As written it describes an encoding the
  toolchain does not implement and could not implement without breaking
  "`Ord` = Appendix G byte order".
- **Appendix I.8 (Numerics)**: fixtures should include an ordering case that
  would fail under the literal reading — the `2` versus `256` pair above is the
  minimal one, and its analogue for negatives is `-2` versus `-256`.

## Implementation alignment

`crates/brix-canon/src/int_codec.rs` implements this ruling
(`encode_uint`/`decode_uint`, `encode_int`/`decode_int`). Decoding rejects a
non-minimal magnitude (a leading zero byte) as `CanonError::NonMinimalInt`, so
the encoding is injective in both directions — one value, one byte string.

The frozen vectors `uint_*`, `uint128_*`, `int_*`, and `int128_*` in
`vectors/canon_vectors.json` pin every boundary case, including `int_i64min`
and `int128_min` where the magnitude is one bit wider than the positive range.
`scripts/canon_crosscheck.py` reproduces all of them from this erratum's text
alone, in a separate implementation that never reads the Rust.
