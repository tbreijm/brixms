#!/usr/bin/env python3
"""Independent from-spec cross-check of the canon/1 golden vectors.

`crates/brix-canon/tests/vectors.rs` freezes `vectors/canon_vectors.json`, but
its encoder *is* `brix-canon`'s `CanonWriter` — so that test cannot cross-check
the corpus, only guard it against drift. This script is the second consumer:
a separate implementation, in a different language, that replays each case's
**declarative spec** from the manifest and must reproduce the frozen bytes.

It deliberately never imports, links, or shells out to any Rust code. If the two
implementations ever disagree, exactly one of them is wrong and the disagreement
is visible rather than silent.

Sources, in order of authority:

1. `spec/BrixMS_v9_0.md` Appendix G — the normative sketch. It pins the *shape*
   of most encodings (length-prefixed bytes and strings, NFC for identifiers,
   sorted records/sets/maps, sequence-order lists, enum ordinal + payload,
   sorted bag pairs, currency + minor units, measure + value).
2. `spec/errata/0001-integer-canonical-encoding.md`,
   `spec/errata/0002-decimal-canonical-encoding.md`, and
   `spec/errata/0003-char-is-not-a-single-tagged-byte.md` — the byte layouts
   Appendix G's text underdetermines or misdescribes. The first two are ruled
   because "Ord = Appendix G byte order" forces them to be order-preserving;
   the third was found by writing this script.

Every layout below therefore has a written source. Where a rule comes from an
erratum rather than from Appendix G directly, the docstring says so.

Run:  python3 scripts/canon_crosscheck.py [--manifest PATH] [-v]
Exit: 0 if every case reproduces, 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import sys
import unicodedata
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = REPO_ROOT / "vectors" / "canon_vectors.json"

MAX_MAG_LEN = 16

# Decimal sign bytes. NOTE: Appendix G says only "scale byte + unscaled integer
# encoding"; erratum 0002 replaces that with a sign/exponent/digits layout and
# fixes these three discriminants so all negatives sort before zero sorts before
# all positives.
DEC_NEG, DEC_ZERO, DEC_POS = 0x00, 0x01, 0x02
DEC_TERMINATOR = 0x00


class CrosscheckError(Exception):
    pass


# ---------------------------------------------------------------------------
# Integers — erratum 0001.
# ---------------------------------------------------------------------------


def enc_uint(v: int) -> bytes:
    """`[len] ++ magnitude_be`, minimal magnitude, `len == 0` for zero.

    Order-preserving: a longer magnitude always denotes a larger value, so the
    length byte dominates; equal lengths compare as plain big-endian bytes.
    """
    if v < 0:
        raise CrosscheckError(f"uint cannot encode negative {v}")
    if v == 0:
        return bytes([0])
    mag = v.to_bytes((v.bit_length() + 7) // 8, "big")
    if len(mag) > MAX_MAG_LEN:
        raise CrosscheckError(f"magnitude too long for {v}")
    return bytes([len(mag)]) + mag


def enc_int(v: int) -> bytes:
    """`[0x80 + sign*len] ++ magnitude`, magnitude complemented when negative.

    The category byte makes the encoding prefix-free and order-preserving in
    both directions: negatives get categories below 0x80 (a *larger* magnitude
    giving a *smaller* category), positives above.
    """
    if v == 0:
        return bytes([0x80])
    negative = v < 0
    mag_int = -v if negative else v
    mag = mag_int.to_bytes((mag_int.bit_length() + 7) // 8, "big")
    if len(mag) > MAX_MAG_LEN:
        raise CrosscheckError(f"magnitude too long for {v}")
    if negative:
        category = 0x80 - len(mag)
        mag = bytes(b ^ 0xFF for b in mag)
    else:
        category = 0x80 + len(mag)
    return bytes([category]) + mag


# ---------------------------------------------------------------------------
# Primitives — Appendix G.
# ---------------------------------------------------------------------------


def enc_bytes(b: bytes) -> bytes:
    """Appendix G: "bytes: length-prefixed raw". The length is a canon uint."""
    return enc_uint(len(b)) + b


def enc_str(s: str) -> bytes:
    """Appendix G: "values as raw Unicode scalar sequences, length-prefixed".

    No normalization: a string *value* keeps its code points.
    """
    return enc_bytes(s.encode("utf-8"))


def enc_ident(s: str) -> bytes:
    """Appendix G: "NFC for identifiers"."""
    return enc_bytes(unicodedata.normalize("NFC", s).encode("utf-8"))


def enc_bool(b: bool) -> bytes:
    """Erratum 0003: a single raw byte, 0x00 false / 0x01 true. Not framed."""
    return bytes([1 if b else 0])


def enc_unit() -> bytes:
    """Erratum 0003: a single raw byte, 0x00."""
    return bytes([0])


def enc_char(codepoint: int) -> bytes:
    """Erratum 0003: the Unicode scalar value as a canon uint (erratum 0001).

    Appendix G groups `Char` with `Bool`/`Unit` as "single tagged bytes", which
    a 21-bit code point cannot be — `char_max` is four bytes. Erratum 0003
    splits that bullet; this implements its ruling.
    """
    if not (0 <= codepoint <= 0x10FFFF) or 0xD800 <= codepoint <= 0xDFFF:
        raise CrosscheckError(f"not a Unicode scalar value: {codepoint:#x}")
    return enc_uint(codepoint)


def enc_decimal(unscaled: int, scale: int) -> bytes:
    """Erratum 0002: `[sign] ++ magnitude`, magnitude complemented when negative.

    For nonzero values `magnitude = enc_int(exponent) ++ digits ++ [0x00]`,
    where `exponent` is the base-10 exponent of the most significant digit and
    each decimal digit `d` is the byte `1 + d` (so the 0x00 terminator is
    strictly below every digit byte, making the magnitude prefix-free).

    Normalization: trailing zero digits are dropped (`1.50` and `1.5` are the
    same value and therefore the same bytes).
    """
    if unscaled == 0:
        return bytes([DEC_ZERO])

    negative = unscaled < 0
    digits = str(abs(unscaled))

    # Drop trailing zeros, reducing the scale in step — this is Appendix G's
    # "normalized (no trailing zeros beyond declared scale)".
    while len(digits) > 1 and digits.endswith("0") and scale > 0:
        digits = digits[:-1]
        scale -= 1

    exponent = len(digits) - 1 - scale
    magnitude = enc_int(exponent) + bytes(1 + int(d) for d in digits) + bytes([DEC_TERMINATOR])

    if negative:
        return bytes([DEC_NEG]) + bytes(b ^ 0xFF for b in magnitude)
    return bytes([DEC_POS]) + magnitude


def enc_quantity(measure: str, unscaled: int, scale: int) -> bytes:
    """Appendix G: "value + measure identifier"."""
    return enc_str(measure) + enc_decimal(unscaled, scale)


def enc_money(currency: str, minor: int) -> bytes:
    """Appendix G: "currency code + minor-unit integer"."""
    return enc_str(currency) + enc_int(minor)


def enc_total_order_f64_bits(bits: int) -> bytes:
    """Appendix G's totalOrder key over an IEEE-754 double's bit pattern.

    Flip every bit when the sign bit is set, otherwise set the sign bit. The
    manifest supplies the *input* bit pattern directly, so NaN canonicalization
    (Part V §8: one NaN pattern) is the producer's concern and is visible in the
    frozen case rather than re-guessed here.

    Note this is not a `canon/1` value encoding at all — Appendix G admits it
    only as the final aggregation-order tiebreak, floats being inadmissible in
    key and identity positions.
    """
    if not (0 <= bits < 1 << 64):
        raise CrosscheckError(f"not a 64-bit pattern: {bits:#x}")
    if bits & 0x8000000000000000:
        bits ^= 0xFFFFFFFFFFFFFFFF
    else:
        bits |= 0x8000000000000000
    return bits.to_bytes(8, "big")


# ---------------------------------------------------------------------------
# Composites — Appendix G.
# ---------------------------------------------------------------------------


def enc_list(elements: list[bytes]) -> bytes:
    """"List/Vector in sequence order", count-prefixed, each element framed."""
    return enc_uint(len(elements)) + b"".join(enc_bytes(e) for e in elements)


def enc_set(elements: list[bytes]) -> bytes:
    """"Set entries sorted by canonical element bytes", deduplicated."""
    unique = sorted(set(elements))
    return enc_uint(len(unique)) + b"".join(enc_bytes(e) for e in unique)


def enc_bag(elements: list[bytes]) -> bytes:
    """"Bag as sorted (element, multiplicity) pairs"."""
    counts: dict[bytes, int] = {}
    for e in elements:
        counts[e] = counts.get(e, 0) + 1
    out = enc_uint(len(counts))
    for element in sorted(counts):
        out += enc_bytes(element) + enc_uint(counts[element])
    return out


def enc_map(entries: list[tuple[bytes, bytes]]) -> bytes:
    """"Map entries sorted by canonical key bytes", each entry key ++ value.

    On a duplicate key the last value wins, matching map construction.
    """
    collapsed: dict[bytes, bytes] = {}
    for k, v in entries:
        collapsed[k] = v
    out = enc_uint(len(collapsed))
    for key in sorted(collapsed):
        out += enc_bytes(key) + enc_bytes(collapsed[key])
    return out


def enc_record(fields: list[tuple[str, bytes]]) -> bytes:
    """"Fields sorted by canonical field-name bytes, each name-prefixed".

    Field names are identifiers, so they fold to NFC before sorting — the sort
    is over the *folded* bytes, or two spellings of one name could order
    differently in two records.
    """
    folded = [
        (unicodedata.normalize("NFC", name).encode("utf-8"), value) for name, value in fields
    ]
    folded.sort(key=lambda nv: nv[0])
    out = enc_uint(len(folded))
    for name_bytes, value in folded:
        # The name is length-framed; the value is written raw (already canonical).
        out += enc_bytes(name_bytes) + value
    return out


def enc_enum(ordinal: int, payload: bytes) -> bytes:
    """"Variant ordinal + payload encodings". The payload is written raw."""
    return enc_uint(ordinal) + payload


# ---------------------------------------------------------------------------
# Spec interpreter — mirrors the `Spec` enum in the Rust test one-for-one.
# ---------------------------------------------------------------------------


def encode_spec(spec: dict) -> bytes:
    kind = spec["kind"]

    if kind == "uint" or kind == "uint128":
        return enc_uint(int(spec["v"]))
    if kind == "int" or kind == "int128":
        return enc_int(int(spec["v"]))
    if kind == "bytes":
        return enc_bytes(bytes.fromhex(spec["v"]))
    if kind == "str":
        return enc_str(spec["v"])
    if kind == "ident":
        return enc_ident(spec["v"])
    if kind == "bool":
        return enc_bool(bool(spec["v"]))
    if kind == "unit":
        return enc_unit()
    if kind == "char":
        return enc_char(int(spec["cp"]))
    if kind == "decimal":
        return enc_decimal(int(spec["unscaled"]), int(spec["scale"]))
    if kind == "quantity":
        return enc_quantity(spec["measure"], int(spec["unscaled"]), int(spec["scale"]))
    if kind == "money":
        return enc_money(spec["currency"], int(spec["minor"]))
    if kind == "list":
        return enc_list([encode_spec(e) for e in spec["elems"]])
    if kind == "set":
        return enc_set([encode_spec(e) for e in spec["elems"]])
    if kind == "bag":
        return enc_bag([encode_spec(e) for e in spec["elems"]])
    if kind == "map":
        return enc_map([(encode_spec(k), encode_spec(v)) for k, v in spec["entries"]])
    if kind == "record":
        return enc_record([(name, encode_spec(v)) for name, v in spec["fields"]])
    if kind == "enum":
        payload = spec.get("payload")
        return enc_enum(int(spec["ordinal"]), encode_spec(payload) if payload else b"")
    if kind == "totalorder_f64":
        return enc_total_order_f64_bits(int(spec["bits"], 16))

    raise CrosscheckError(f"unknown spec kind {kind!r}")


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()

    try:
        manifest = json.loads(args.manifest.read_text())
    except FileNotFoundError:
        print(f"canon-crosscheck: manifest not found: {args.manifest}", file=sys.stderr)
        return 1

    version = manifest.get("canon_version")
    if version != "canon/1":
        print(
            f"canon-crosscheck: this script implements canon/1, manifest declares "
            f"{version!r}. A version bump needs a reviewed update here too.",
            file=sys.stderr,
        )
        return 1

    cases = manifest.get("cases", [])
    if not cases:
        print("canon-crosscheck: manifest has no cases", file=sys.stderr)
        return 1

    failures: list[str] = []
    skipped: list[str] = []

    for case in cases:
        name = case.get("name", "<unnamed>")
        expected = case.get("hex", "")
        try:
            produced = encode_spec(case).hex()
        except CrosscheckError as exc:
            skipped.append(f"{name}: {exc}")
            continue

        if produced != expected:
            failures.append(f"  {name}\n    expected {expected}\n    produced {produced}")
        elif args.verbose:
            print(f"  ok  {name}  {produced}")

    if skipped:
        print("canon-crosscheck: cases this script could not interpret:", file=sys.stderr)
        for s in skipped:
            print(f"  {s}", file=sys.stderr)

    if failures or skipped:
        print(
            f"\ncanon-crosscheck: FAILED — {len(failures)} mismatched, "
            f"{len(skipped)} uninterpretable, out of {len(cases)} cases.",
            file=sys.stderr,
        )
        for f in failures:
            print(f, file=sys.stderr)
        print(
            "\nThe Rust encoder and this independent implementation disagree. "
            "Exactly one of them is wrong; canon/1 is frozen ABI, so resolve it "
            "against Appendix G and spec/errata/0001-0002 before changing either.",
            file=sys.stderr,
        )
        return 1

    print(
        f"canon-crosscheck: all {len(cases)} canon/1 vectors reproduced "
        f"independently from Appendix G + errata 0001/0002/0003."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
