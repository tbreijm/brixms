//! A differential corpus for the checker's derivations.
//!
//! **Why this exists.** `infer_tree` is being restructured — arm splitting,
//! then an iterative traversal over an interned expression graph. Every one of
//! those changes is supposed to be behaviour-preserving, and the failure mode
//! if it is not is the worst kind this codebase has: not a crash, but *wrong
//! derivations* that still verify and still mint certificates. Running the
//! suite would not catch it, because a wrong derivation is a perfectly valid
//! derivation of something else.
//!
//! So the identities are pinned. A witness id is the composition of a
//! derivation's generators; a proposition id binds the expression and its
//! type. Between them, a restructuring that changes *which generators fire* or
//! *what they conclude* moves a digest here and fails.
//!
//! These values were captured before the restructuring began. If one changes,
//! the question is not "update the constant" — it is "which derivation
//! changed, and was that intended".

use brix_semantic::ContextId;
use soc_regimes::type_realization::*;

/// `(name, witness id, proposition id)` — captured 2026-08-18, before the
/// iterative-traversal work.
const EXPECTED: &[(&str, &str, &str)] = &[
    (
        "lit",
        "d6cad2cd8d1aab6b9c8225bf562de30b109fdf708796a41918af9fe8a445ecf8",
        "474b2f6f980df76ce06a2e8e040aefe1ce8c4943e63cac3c5eced1eb832c416f",
    ),
    (
        "str",
        "274126ce4fef77e63fce28f57e60914f817ba1fbce7e66fd4a0e4b2e75f9d558",
        "9b63927db029c215654df72b5548e8b400f449bf430e62651e68e425826fdf87",
    ),
    (
        "bool",
        "9a80c18a42512810ec4e8dbdca7c69f4162357798a318772ad1140285945008e",
        "030b1658ca46e27db7fd0e857ad9cc0fbd27e80b9ccbefbeb8e27a087ce2e5c7",
    ),
    (
        "arith",
        "49d159e8530bfb711e88a79f10239664bdc3a7ea147bab83cfc44ec832661544",
        "dfaac910c83d174b82546e40a6a135aad3861f7bc50353804b987772995155c6",
    ),
    (
        "cmp",
        "3aeb2cae1365e8dfb39c80ecb9102130d455a09d7902f047d93c7db157de5ee5",
        "7f49819b0541cd926ce239bd23e3d8acc60dabff3901e81aa9be84823d16be7e",
    ),
    (
        "record",
        "a5973b8a37a96167d42409a50166a0e155762aeff7c08eadf5257ffec429e364",
        "b22123fb9891fad5894f90fac6968ebb9bfef74574defbba2a33586c2992fc90",
    ),
    (
        "field",
        "347055f17527654fd18ea7fe4ce203755207ccec815bbb6cf16e0ae4de97351f",
        "c740836e148fd3658f79d11ddd140d69ad85e170f822d6e551cd188838a795dc",
    ),
    (
        "ctor_nullary",
        "78fb3dfe83460b0b0a9f3db5d428a71ce4027c1be4152715ecd17bdf4cdd5b64",
        "b69ac5d1ff83b971ccbfe5f9318910f8eda52e9c8361aba9baf536c543b1269c",
    ),
    (
        "ctor_payload",
        "eff4932515c2f8ad7972bbcfa1777108b91df9e1bf5ca9a6e2dcc836a643e7bf",
        "5bebcddb9dd121a8fd5c112d47bf6ad9d0cc3595464acf443be595fbca52f185",
    ),
    (
        "match",
        "f165d1cfe8858fb7b8922c1edd28e200294295b6eaccc6b6f85ba15a814f898e",
        "8f9b5c69d26f8f3bb613e023d6943211d68d4d4b4a8f939934b9ccfb8ee8ceda",
    ),
    (
        "lam_app",
        "0e1a11bc09ffc80e226bfef28e8df15f1cfca369fad512dd34f57193f4f5486d",
        "a5251dd38a9be5d8ac688baf744196ae20c8307b180279129e12559c653b2309",
    ),
    (
        "lamann",
        "0e1a11bc09ffc80e226bfef28e8df15f1cfca369fad512dd34f57193f4f5486d",
        "86221cfe5e62d3ce6b672aa2df2cbfa6f5837db8c60502f36ddc9e56884f5a40",
    ),
    (
        "and",
        "4c0a38a1f6c6c38d30f3f099dc3de4302045d0d76a6ece022893b562da090d3b",
        "f1dc1f5980590eaeaa251364bde23df2cd0810d302df47f2685cbf14e8015206",
    ),
    (
        "deep_list",
        "7a3eaffb4d8f4487a36da359a375994e0c84bce42c02827ff36ddb706c0a5d62",
        "ce10c6b3230962e906975f169c8cf4ef6803e2e2c44253f4cc9a17bc1743eff9",
    ),
];

fn opt_ty() -> Ty {
    Ty::Sum(
        "Opt".into(),
        vec![
            ("None".into(), vec![]),
            ("Some".into(), vec![Ty::Con("Int")]),
        ],
    )
}

fn list_ty() -> Ty {
    Ty::Rec(
        "L".into(),
        Box::new(Ty::Sum(
            "L".into(),
            vec![
                ("Nil".into(), vec![]),
                ("Cons".into(), vec![Ty::Con("Int"), Ty::RecVar("L".into())]),
            ],
        )),
    )
}

fn corpus() -> Vec<(&'static str, Expr)> {
    let opt = opt_ty();
    let lst = list_ty();
    let mut deep = Expr::Ctor(lst.clone(), "Nil".into(), vec![]);
    for _ in 0..6 {
        deep = Expr::Ctor(lst.clone(), "Cons".into(), vec![Expr::Lit(1), deep]);
    }
    vec![
        ("lit", Expr::Lit(42)),
        ("str", Expr::StrLit("x".into())),
        ("bool", Expr::BoolLit(true)),
        (
            "arith",
            Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        ),
        (
            "cmp",
            Expr::Cmp(CmpOp::Lt, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        ),
        (
            "record",
            Expr::Record(vec![
                ("a".into(), Expr::Lit(1)),
                ("b".into(), Expr::StrLit("s".into())),
            ]),
        ),
        (
            "field",
            Expr::Field(
                Box::new(Expr::Record(vec![("a".into(), Expr::Lit(7))])),
                "a".into(),
            ),
        ),
        (
            "ctor_nullary",
            Expr::Ctor(opt.clone(), "None".into(), vec![]),
        ),
        (
            "ctor_payload",
            Expr::Ctor(opt.clone(), "Some".into(), vec![Expr::Lit(3)]),
        ),
        (
            "match",
            Expr::Match(
                Box::new(Expr::Ctor(opt.clone(), "Some".into(), vec![Expr::Lit(3)])),
                vec![
                    (Pattern::Ctor("None".into(), vec![]), Expr::Lit(0)),
                    (
                        Pattern::Ctor("Some".into(), vec![Pattern::Var("k".into())]),
                        Expr::Var("k".into()),
                    ),
                ],
            ),
        ),
        (
            "lam_app",
            Expr::App(
                Box::new(Expr::Lam("x".into(), Box::new(Expr::Var("x".into())))),
                Box::new(Expr::Lit(5)),
            ),
        ),
        (
            "lamann",
            Expr::App(
                Box::new(Expr::LamAnn(
                    "x".into(),
                    Ty::Con("Int"),
                    Box::new(Expr::Var("x".into())),
                )),
                Box::new(Expr::Lit(5)),
            ),
        ),
        (
            "and",
            Expr::And(Box::new(Expr::Lit(1)), Box::new(Expr::StrLit("s".into()))),
        ),
        ("deep_list", deep),
    ]
}

#[test]
fn derivations_are_unchanged() {
    let corpus = corpus();
    assert_eq!(
        corpus.len(),
        EXPECTED.len(),
        "every corpus entry must be pinned; a new one needs its identities captured"
    );

    for ((name, expr), (expected_name, witness, proposition)) in corpus.iter().zip(EXPECTED) {
        assert_eq!(
            name, expected_name,
            "corpus and expectations must stay aligned"
        );

        let (judgement, derivation) =
            audited_type_check_tree(expr, &TyCtx::new(), ContextId::root())
                .unwrap_or_else(|e| panic!("'{name}' must still check: {e:?}"));

        assert_eq!(
            derivation.tree().witness_id().digest().to_hex(),
            *witness,
            "'{name}': the derivation's generator composition changed"
        );
        assert_eq!(
            judgement.proposition.digest().to_hex(),
            *proposition,
            "'{name}': what the derivation concludes changed"
        );
    }
}
