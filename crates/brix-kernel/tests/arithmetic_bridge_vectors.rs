//! New bridge rows must agree with the independent, frozen v2 arithmetic matrix.
use brix_canon::{CanonWriter, Digest, Domain};
use brix_kernel::{
    resolve_primitive_relation, typing_arith_input_v1, typing_arith_v3, PinnedArithOp,
    PinnedNumericTy,
};
use brix_semantic::PropositionId;
use std::collections::BTreeSet;

fn digest(hex: &str) -> Digest {
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    Digest::from_bytes(bytes)
}

#[test]
fn bridge_rows_match_the_frozen_operator_operand_and_promotion_matrix() {
    let manifest = include_str!("../../../vectors/primitive_relation_typing_arith_v2.json");
    // Read only literal string fields from this frozen, checked-in fixture.
    let quoted = |s: &str| {
        s.trim_start()
            .strip_prefix('"')
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned()
    };
    let mut inputs = BTreeSet::new();
    let mut results = BTreeSet::new();
    for row in manifest.split("\"operator\":").skip(1) {
        let field = |name: &str| quoted(row.split_once(&format!("\"{name}\":")).unwrap().1);
        let ty = |name: &str| {
            PinnedNumericTy::ALL
                .into_iter()
                .find(|t| t.lattice_name() == name)
                .unwrap()
                .proposition_id()
        };
        let op = match quoted(row).as_str() {
            "Add" => PinnedArithOp::Add,
            "Sub" => PinnedArithOp::Sub,
            "Mul" => PinnedArithOp::Mul,
            "Div" => PinnedArithOp::Div,
            _ => unreachable!(),
        };
        // Independent object preimage: Tensor(Const(op), Tensor(Const(lhs), Const(rhs))).
        // Does not invoke ObjectTerm's encoder or the production row builders.
        let mut w = CanonWriter::new();
        w.write_uint(3);
        w.write_uint(0);
        w.write_bytes(op.proposition_id().digest().as_bytes());
        w.write_uint(3);
        for name in ["lhs", "rhs"] {
            w.write_uint(0);
            w.write_bytes(ty(&field(name)).digest().as_bytes());
        }
        let source = PropositionId(Digest::of(Domain::Value, &w.finish()));
        let input = PropositionId(digest(&field("src")));
        inputs.insert((source, input));
        results.insert((input, ty(&field("result"))));
    }
    assert_eq!(inputs.len(), 120);
    assert_eq!(
        resolve_primitive_relation(&typing_arith_input_v1())
            .unwrap()
            .rows,
        inputs
    );
    assert_eq!(
        resolve_primitive_relation(&typing_arith_v3()).unwrap().rows,
        results
    );
    assert_eq!(
        typing_arith_input_v1().to_hex(),
        "ee4153aae1f4823b334aa00c53a82e388abe567d6655b88bf89163d4b315a335"
    );
    assert_eq!(
        typing_arith_v3().to_hex(),
        "bd491826d28757ec86505e208bfe4c62050a5b88df71f4716473b5b07a6dc2cc"
    );
}
