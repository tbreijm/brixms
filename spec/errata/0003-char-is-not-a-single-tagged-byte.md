# 0003 — `Char` is grouped with `Bool`/`Unit` as "single tagged bytes", but cannot be one

**Lane:** canon (brix-canon)
**Status:** ruled 2026-08-04
**Affected conformance:** Appendix G (Canonical encoding and identity)

## Provenance

Not one of the two errata cited from `crates/brix-canon`. Found while writing
`scripts/canon_crosscheck.py` (issue #237): implementing `Char` from Appendix G's
text produced one byte, and the frozen vector for `char_max` is four. Recorded
here rather than smoothed over in the script, since it is the same class of
defect as errata 0001 and 0002 — the sketch text underdetermining, or in this
case misdescribing, a byte layout.

## The tension

Appendix G groups three types in one bullet:

> Bool/Unit/Char: single tagged bytes;

For `Bool` and `Unit` this is accurate. For `Char` it is not achievable: a
Unicode scalar value ranges over `U+0000..=U+10FFFF` (excluding the surrogate
range), which needs 21 bits. No single byte encodes it.

The frozen vectors show what is actually implemented:

| case | code point | bytes |
|---|---|---|
| `char_A` | `U+0041` | `01 41` |
| `char_world` | `U+4E16` | `02 4e 16` |
| `char_max` | `U+10FFFF` | `03 10 ff ff` |

That is exactly erratum 0001's unsigned integer encoding applied to the scalar
value — `[len] ++ magnitude_be` — not a tagged byte.

Note also that `Bool` and `Unit` *are* single bytes but Appendix G does not say
**which** bytes. The discriminants in use are `Bool` → `0x00`/`0x01` and `Unit`
→ `0x00`. These are the obvious choices, but "obvious" is not "specified", and
a second implementation has nothing to derive them from.

## Ruling (adopted 2026-08-04)

Split the bullet. The three types have three different layouts:

- **`Bool`**: a single raw byte, `0x00` for false and `0x01` for true. Not
  length-prefixed.
- **`Unit`**: a single raw byte, `0x00`.
- **`Char`**: the Unicode scalar value encoded as a `canon/1` unsigned integer
  per erratum 0001 — `[len] ++ magnitude_be`. Surrogate code points
  (`U+D800..=U+DFFF`) are not scalar values and are rejected, not encoded.

`Char` is therefore order-preserving by inheritance from the integer encoding,
which is the property Appendix G's ordering guarantee needs and which a
fixed-width or tagged-byte layout would have had to establish separately.

### Why this is a clarification, not a change

Nothing in the toolchain changes. `canon/1` bytes are unaffected, no vector
moves, and no `CANON_VERSION` bump is required — this erratum records what the
frozen artifact already contains. It exists so that an independent
implementation has something to implement *from*, which is precisely what
`scripts/canon_crosscheck.py` needed and did not have.

## Conformance IDs affected

- **Appendix G**: replace the single "Bool/Unit/Char" bullet with the three
  layouts above, or cite this erratum.

## Implementation alignment

`crates/brix-canon/src/lib.rs`: `CanonWriter::write_bool`, `write_unit`, and
`write_char`. Frozen vectors `bool_false`, `bool_true`, `unit`, `char_A`,
`char_world`, `char_max`. `scripts/canon_crosscheck.py` reproduces all six from
this erratum's text.
