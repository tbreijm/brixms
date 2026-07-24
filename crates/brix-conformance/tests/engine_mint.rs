//! #15 value-construction primitives — the byte-identity proof (Part D of
//! the mint-primitives slice).
//!
//! Pure Rust, no `.brix` rule surface involved: proves that `brix-rt`'s
//! `engine::builtin_total("brix.ty.mint_unary"/"brix.ty.mint_binary")` +
//! `"brix.canon.digest"` reproduce `brix_ir::types::Ty::canon_write`'s
//! inline enum framing byte-for-byte — `write_enum(ctor, |w|
//! child.canon_write(w))` is `uint(ctor) ++ child_raw_bytes` in one buffer,
//! hashed once (`Digest::of(Domain::Value, ...)`). So minting
//! `Ty::option(Int)`/`Ty::Result(Int, Str)` from the child's/children's raw
//! canonical bytes must land on the exact same token
//! `crates/brix-conformance/src/typefacts.rs`'s exporter (`digest_hex`)
//! would compute for that same composite `Ty` directly.
//!
//! `digest_hex` itself is private to `typefacts`, but its entire body is
//! three public calls — `CanonWriter::new()`, `Ty::canon_write`,
//! `CanonWriter::digest(Domain::Value)` (itself just `Digest::of(domain,
//! &self.buf)`) — so `exporter_token` below reproduces it exactly via the
//! same public API, with no logic of its own that could diverge from it.
//! The builtins under test, by contrast, are called through the real
//! `brix_rt::engine::builtin_total` dispatch — the exact function pointers
//! `Expr::Call` resolves at rule-evaluation time — not a re-implementation.
//!
//! This test failing on the first ordinal/framing mismatch is exactly the
//! tripwire the mint primitives exist to catch.

use brix_canon::{CanonWriter, Canonical, Domain};
use brix_ir::types::{IntWidth, Ty};
use brix_rt::engine::{builtin_total, Value};

/// The exporter's own tokenization logic (`typefacts::digest_hex`),
/// reproduced via public API — see the module doc for why this is provably
/// identical rather than a parallel surrogate.
fn exporter_token(ty: &Ty) -> String {
    let mut w = CanonWriter::new();
    ty.canon_write(&mut w);
    w.digest(Domain::Value).to_hex()
}

/// A `Ty`'s raw canonical bytes — what a `TyBytes` Ground fact carries as
/// its `Value::Bytes` payload (`typefacts::ty_canon_bytes`).
fn canon_bytes(ty: &Ty) -> Vec<u8> {
    let mut w = CanonWriter::new();
    ty.canon_write(&mut w);
    w.finish()
}

fn mint_unary(ctor: i64, child_bytes: Vec<u8>) -> Value {
    let mint = builtin_total("brix.ty.mint_unary").expect("brix.ty.mint_unary is registered");
    mint(&[Value::Int(ctor), Value::Bytes(child_bytes)])
}

fn mint_binary(ctor: i64, a_bytes: Vec<u8>, b_bytes: Vec<u8>) -> Value {
    let mint = builtin_total("brix.ty.mint_binary").expect("brix.ty.mint_binary is registered");
    mint(&[
        Value::Int(ctor),
        Value::Bytes(a_bytes),
        Value::Bytes(b_bytes),
    ])
}

fn digest_of(bytes: Value) -> String {
    let digest = builtin_total("brix.canon.digest").expect("brix.canon.digest is registered");
    match digest(&[bytes]) {
        Value::Str(hex) => hex,
        other => panic!("brix.canon.digest returned a non-Str Value: {other:?}"),
    }
}

/// `Ty::Option`'s canon ordinal — `Ty::canon_write`, `crates/brix-ir/src/types.rs:402`.
const OPTION_CTOR: i64 = 14;
/// `Ty::Result`'s canon ordinal — `Ty::canon_write`, `crates/brix-ir/src/types.rs:403`.
const RESULT_CTOR: i64 = 15;

#[test]
fn mint_unary_reproduces_option_of_int_byte_for_byte() {
    let child = Ty::Int(IntWidth::Int);
    let parent = Ty::option(child.clone());

    let minted = mint_unary(OPTION_CTOR, canon_bytes(&child));
    let Value::Bytes(minted_bytes) = &minted else {
        panic!("brix.ty.mint_unary must return Value::Bytes, got {minted:?}")
    };
    // The strongest check: the minted raw bytes equal `Ty::canon_write`'s
    // own raw bytes for `Option<Int>`, not merely a digest collision.
    assert_eq!(
        minted_bytes,
        &canon_bytes(&parent),
        "mint_unary(Option ctor, Int's raw bytes) must equal Ty::option(Int).canon_write()'s raw bytes exactly"
    );

    let minted_hex = digest_of(minted);
    let expected_hex = exporter_token(&parent);
    assert_eq!(
        minted_hex, expected_hex,
        "digest of the minted bytes must equal the exporter's own token for Ty::option(Int)"
    );
}

#[test]
fn mint_binary_reproduces_result_of_int_str_byte_for_byte() {
    let ok = Ty::Int(IntWidth::Int);
    let err = Ty::Str;
    let parent = Ty::Result(Box::new(ok.clone()), Box::new(err.clone()));

    let minted = mint_binary(RESULT_CTOR, canon_bytes(&ok), canon_bytes(&err));
    let Value::Bytes(minted_bytes) = &minted else {
        panic!("brix.ty.mint_binary must return Value::Bytes, got {minted:?}")
    };
    assert_eq!(
        minted_bytes,
        &canon_bytes(&parent),
        "mint_binary(Result ctor, Int's/Str's raw bytes) must equal Ty::Result(Int, Str).canon_write()'s raw bytes exactly"
    );

    let minted_hex = digest_of(minted);
    let expected_hex = exporter_token(&parent);
    assert_eq!(
        minted_hex, expected_hex,
        "digest of the minted bytes must equal the exporter's own token for Ty::Result(Int, Str)"
    );
}
