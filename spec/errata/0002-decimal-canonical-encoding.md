# 0002 — "scale byte + unscaled integer encoding" is not order-preserving across scales

**Lane:** canon (brix-canon)
**Status:** ruled 2026-08-04
**Affected conformance:** Appendix G (Canonical encoding and identity), Appendix I.8
(Numerics), static per-package Decimal contexts

## The tension

Appendix G's decimal bullet:

> Decimal<P,S>: scale byte + unscaled integer encoding; normalized (no trailing
> zeros beyond declared scale);

Taken literally as `[scale] ++ unscaled_int`, the scale byte is the *most
significant* part of the comparison. So values are ordered by scale first and
magnitude second, which is not numeric order:

```
2    = unscaled 2,  scale 0  ->  [0x00] ++ enc_int(2)
1.5  = unscaled 15, scale 1  ->  [0x01] ++ enc_int(15)
```

`0x00 … < 0x01 …`, so `2` sorts before `1.5`. Same defect as erratum 0001 and
the same consequence: the numerics section's "`Ord` = Appendix G byte order"
requirement fails, and canonical row order (Part V §8) stops being value order.

Fixing the integer encoding (erratum 0001) does **not** fix this. The problem is
structural: any layout that puts scale before magnitude compares scales first,
however well the magnitude itself is encoded.

Note also that Appendix G's normalization clause and its literal layout are in
mild tension with each other. "No trailing zeros beyond declared scale" means
`1.50` and `1.5` must encode identically — but under `[scale] ++ unscaled` they
have different scale bytes (`2` vs `1`) and different unscaled values (`150` vs
`15`), so the layout cannot express the normalization the same bullet demands
unless the scale is *derived* rather than *declared*.

## Why brix-canon cannot leave this unresolved

Same reason as erratum 0001: `Decimal` is a `canon/1` value type, its bytes are
frozen in `vectors/canon_vectors.json`, and money and quantity encodings embed
it. The layout had to be settled before the G0 freeze. This erratum records the
ruling, which until now lived only in `crates/brix-canon/src/decimal.rs`'s
module comment.

## Ruling (adopted 2026-08-04)

Replace the literal layout with a normalized sign/exponent/digit-string
encoding — the same shape a scientific-notation representation has, chosen
because it is order-preserving by construction.

```
[sign] ++ magnitude
```

### Sign

One byte, one of:

| byte | meaning |
|---|---|
| `0x00` | negative |
| `0x01` | zero |
| `0x02` | positive |

So every negative sorts before zero, which sorts before every positive, before
any magnitude byte is examined. **Zero has no magnitude at all** — it is the
single byte `0x01` — which also disposes of the `0` versus `0.00` question:
they are the same value and therefore the same bytes.

### Magnitude (nonzero values)

```
enc_int(exponent) ++ digit_bytes ++ [0x00]
```

- **Normalize first**: strip trailing zero digits, decrementing the scale in
  step, until the digit string has no trailing zero (or one digit remains).
  This is Appendix G's normalization clause, applied to the digits rather than
  to a declared scale.
- **`exponent`** is the base-10 exponent of the most significant digit:
  `ndigits - 1 - scale`, encoded with erratum 0001's order-preserving signed
  integer encoding. Comparing exponents first is what makes magnitudes compare
  by size.
- **`digit_bytes`**: each decimal digit `d` (0–9) as the single byte `1 + d`,
  most significant first. The `+1` bias is what makes the terminator work.
- **terminator `0x00`**: strictly less than every digit byte (which start at
  `0x01`). This makes the magnitude prefix-free, so comparing two magnitudes
  always reaches a definite first differing byte — without it, `1.5` and `1.55`
  would compare as a string and its prefix, with no byte to decide on.

### Negation

For a negative value the **entire magnitude is bitwise complemented** after
construction. This reverses both the exponent ordering and the digit ordering,
so that among negatives a larger magnitude (a smaller value) sorts first —
which is correct, and is why the sign byte alone is not enough.

### Worked check

```
 1.5   -> 02 | enc_int(0)=80  | 02 06 | 00       = 02 80 02 06 00
 15    -> 02 | enc_int(1)=8101| 02 06 | 00       = 02 81 01 02 06 00
 0.05  -> 02 | enc_int(-2)=7ffd| 06   | 00       = 02 7f fd 06 00
-1.5   -> 00 | ~(80 02 06 00)                    = 00 7f fd f9 ff
 1.50  -> normalizes to 1.5                      = 02 80 02 06 00
```

Byte order: `-1.5 < 0.05 < 1.5 < 15`, matching numeric order.

## Conformance IDs affected

- **Appendix G**: the `Decimal<P,S>` bullet should be rewritten to the layout
  above or cite this erratum. As written it specifies an encoding that violates
  the ordering guarantee stated elsewhere in the same document.
- **Appendix I.8 (Numerics)**: fixtures should include a cross-scale ordering
  case (`2` versus `1.5` is minimal), a normalization case (`1.50` ≡ `1.5`), a
  negative-ordering case, and the zero/`0.00` identity.

## Implementation alignment

`crates/brix-canon/src/decimal.rs` implements this ruling. The frozen vectors
`dec_zero`, `dec_zero_scaled`, `dec_15`, `dec_1_5`, `dec_1_50_normalizes`,
`dec_0_05`, `dec_neg_1_5`, `dec_neg_2`, and `dec_big` pin the sign discriminants,
the exponent field, normalization, and the complement-on-negative rule.
`scripts/canon_crosscheck.py` reproduces all of them from this erratum's text
alone, in an implementation that never reads the Rust.

`Quantity` and `Money` embed this encoding directly (`qty_*`, `money_*`), so a
change here would silently re-identify every quantity and money value —
hence the `CANON_VERSION` bump requirement in `crates/brix-canon/OWNER.md`.
