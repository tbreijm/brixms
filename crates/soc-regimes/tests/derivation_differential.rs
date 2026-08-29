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
//!
//! **Re-captured once, deliberately, for ADR-0028.** Putting the context into
//! the emitted endpoint (⟨D-CTXENDPOINT⟩) changes what every typing derivation
//! is *about*, so every proposition here moved. The ADR says so up front (§4)
//! rather than discovering it here, which is the only thing that makes a
//! re-capture legitimate — otherwise this file is a rubber stamp.
//!
//! What did **not** move is the more telling half: all fourteen witness ids
//! then pinned survived the migration byte-for-byte. The same generators fire
//! in the same composition; only the claim's subject gained its assumption
//! scope. That is the evidence that ADR-0028 was an endpoint change and not a
//! behaviour change — and it is why the two ids are pinned separately rather
//! than as one digest over the pair.
//!
//! **Extended before the traversal rewrite.** The original fourteen left the
//! arms a traversal rewrite is *most* likely to break unguarded: no `Var` under
//! a real scope, no binder whose body is judged under an extended one (`Fix`,
//! nested `Lam`), no `FloatLit`, and nothing deep enough for the order of
//! operations to matter. Those were added and captured against the recursive
//! implementation first, so the rewrite has something to be measured against.
//! Adding them moved none of the existing entries.

use soc_regimes::type_realization::*;

/// `(name, witness id, proposition id)`.
///
/// The original fourteen: witness ids captured 2026-08-18 and unchanged since;
/// proposition ids re-captured 2026-08-19 for ADR-0028 (see the module doc).
/// `float`, `var`, `fix`, `nested_lam`, `deep_spine`: captured 2026-08-20
/// against the recursive traversal, before the iterative rewrite.
const EXPECTED: &[(&str, &str, &str)] = &[
    (
        "lit",
        "d6cad2cd8d1aab6b9c8225bf562de30b109fdf708796a41918af9fe8a445ecf8",
        "a707cca14c12eb75526c195f75366def2610b94b866ce0c222963a095f5488d2",
    ),
    (
        "str",
        "274126ce4fef77e63fce28f57e60914f817ba1fbce7e66fd4a0e4b2e75f9d558",
        "b3354c796e5f612cffd739fa4da081414cf92f619ed2a5391a114110a314f5f8",
    ),
    (
        "bool",
        "9a80c18a42512810ec4e8dbdca7c69f4162357798a318772ad1140285945008e",
        "117ba31c1ac7950329340be076003335b3ab4d287a6d4869fe64236ed16daa14",
    ),
    (
        "float",
        "84d5f0c7a0e9b49eaf9941af22a4a433df415eab2dfcf0b4b6eb6f491ea923bb",
        "873c32f7cad0479ba709fe3355d1560fee06fa1e89687b1e1ed4a191ce6ed409",
    ),
    (
        "var",
        "4b7c492c3fafc0673d9b352fbda1657baec68ffd24794bc728caa6896a2c6a39",
        "7d0bb9831bcf20fe0d38467560247e3d79a59c1bf0d27706c39041f3ef2d4203",
    ),
    (
        "fix",
        "60399c40b7a2edc4af18f9be0712da7c8165465787ffa5f85765ba5637c369b8",
        "abbe70d1dede14c2ef80d268a72754e99e53652f5afb8aa6f4d3c0314b42ad52",
    ),
    (
        "nested_lam",
        "7cae7dc0fccb16ab47aa385cc45e25c961c7d7aa553670f3b73c9e50004d00f6",
        "1b000c577185fd5c1650335d8cb6c36beab2aba98c001efb0fcbebfcdd270137",
    ),
    (
        "deep_spine",
        "84aa9b0982273cca22166b294492a08734a53fd4cd60026aa3156d8a2914effd",
        "6e9232fc02a1365b9503009c8b615f94b3577c795a548c9972e4cd74590d4f68",
    ),
    (
        "arith",
        "49d159e8530bfb711e88a79f10239664bdc3a7ea147bab83cfc44ec832661544",
        "848e923798854c93643a89429bca841d4829f1312cf2d3d0ce850ddc31fad846",
    ),
    (
        "cmp",
        "3aeb2cae1365e8dfb39c80ecb9102130d455a09d7902f047d93c7db157de5ee5",
        "a1d91142c0e408595a369924162243e906b45435d9ce51d30999000794befce0",
    ),
    (
        "record",
        "a5973b8a37a96167d42409a50166a0e155762aeff7c08eadf5257ffec429e364",
        "e405b4978389e974395fc832f61993f7e90df79843cfbc56224e92b54045320c",
    ),
    (
        "field",
        "347055f17527654fd18ea7fe4ce203755207ccec815bbb6cf16e0ae4de97351f",
        "964301913af460366895081993603339a98b1286ddb75f54ba546093cf3017fe",
    ),
    (
        "ctor_nullary",
        "78fb3dfe83460b0b0a9f3db5d428a71ce4027c1be4152715ecd17bdf4cdd5b64",
        "c66b4452bdb358a899966c3d43dc0c875d1c125eca75d64a24e09afd5791bc54",
    ),
    (
        "ctor_payload",
        "eff4932515c2f8ad7972bbcfa1777108b91df9e1bf5ca9a6e2dcc836a643e7bf",
        "9fd07a9e1ea3a8fd68af5dcced7fed6285b6f0900bea7aaabaa2220750c57001",
    ),
    (
        "match",
        "f165d1cfe8858fb7b8922c1edd28e200294295b6eaccc6b6f85ba15a814f898e",
        "ae189bf689ac2c78b8540cd1b0317b2256468bbdb614d5437402187d018074e5",
    ),
    (
        "lam_app",
        "0e1a11bc09ffc80e226bfef28e8df15f1cfca369fad512dd34f57193f4f5486d",
        "ef733d14b0d78c5568a14f7254cdc5e051b6d30fe68916295f1c80c032244553",
    ),
    (
        "lamann",
        "0e1a11bc09ffc80e226bfef28e8df15f1cfca369fad512dd34f57193f4f5486d",
        "ec7ce681eba49f0e9322d30ff0b69523ed0d0ac930e2219522ce720a1143b4f5",
    ),
    (
        "and",
        "4c0a38a1f6c6c38d30f3f099dc3de4302045d0d76a6ece022893b562da090d3b",
        "5803acef92540ae168b5bb1600dc8160c6dc35ba5283f571df6b3826f0ba5b4e",
    ),
    (
        "deep_list",
        "7a3eaffb4d8f4487a36da359a375994e0c84bce42c02827ff36ddb706c0a5d62",
        "8d22dccaa35b1a3154cc1a3dacafb7d228b3593c0c09645432102f4779228031",
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

/// The scope an entry is checked under.
///
/// Most entries assume nothing, but `Var` and the binder forms are exactly the
/// arms where a traversal rewrite is most likely to go wrong — and since
/// ADR-0028 they are also where the *endpoints* carry something other than the
/// root. An entry that could only ever be checked under an empty scope could
/// not guard them.
fn bound_ctx() -> TyCtx {
    TyCtx::new()
        .extend("n", Ty::Con("Int"))
        .extend("s", Ty::Con("Str"))
}

fn corpus() -> Vec<(&'static str, Expr, TyCtx)> {
    let opt = opt_ty();
    let lst = list_ty();
    let mut deep = Expr::Ctor(lst.clone(), "Nil".into(), vec![]);
    for _ in 0..6 {
        deep = Expr::Ctor(lst.clone(), "Cons".into(), vec![Expr::Lit(1), deep]);
    }
    // Nested binders: the context extends at each level, so every inner
    // endpoint names a different scope. A traversal that threaded contexts
    // wrongly — reusing a parent's scope for a child, or vice versa — moves
    // these and nothing else.
    let nested_lam = Expr::Lam(
        "f".into(),
        Box::new(Expr::Lam(
            "g".into(),
            Box::new(Expr::App(
                Box::new(Expr::Var("f".into())),
                Box::new(Expr::Var("g".into())),
            )),
        )),
    );
    // A left-nested spine. Deliberately kept at a depth the *recursive*
    // implementation survives, so these identities can be captured before the
    // traversal is rewritten — pinning behaviour at depth is this entry's job.
    // Proving the depth limit is gone is a different claim and gets its own
    // test, so that an abort is never reported as an identity change.
    let mut spine = Expr::Lit(0);
    for _ in 0..20 {
        spine = Expr::Arith(ArithOp::Add, Box::new(spine), Box::new(Expr::Lit(1)));
    }
    vec![
        ("lit", Expr::Lit(42), TyCtx::new()),
        ("str", Expr::StrLit("x".into()), TyCtx::new()),
        ("bool", Expr::BoolLit(true), TyCtx::new()),
        ("float", Expr::FloatLit("1.5".into()), TyCtx::new()),
        // A bound variable, under a scope that is not the root.
        ("var", Expr::Var("n".into()), bound_ctx()),
        // A fixpoint: the body is judged under a scope assuming the
        // definition's own type, which is what keeps the derivation finite.
        (
            "fix",
            Expr::Fix(
                "loop".into(),
                Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Int"))),
                Box::new(Expr::LamAnn(
                    "k".into(),
                    Ty::Con("Int"),
                    Box::new(Expr::Var("k".into())),
                )),
            ),
            TyCtx::new(),
        ),
        ("nested_lam", nested_lam, TyCtx::new()),
        ("deep_spine", spine, TyCtx::new()),
        (
            "arith",
            Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
            TyCtx::new(),
        ),
        (
            "cmp",
            Expr::Cmp(CmpOp::Lt, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
            TyCtx::new(),
        ),
        (
            "record",
            Expr::Record(vec![
                ("a".into(), Expr::Lit(1)),
                ("b".into(), Expr::StrLit("s".into())),
            ]),
            TyCtx::new(),
        ),
        (
            "field",
            Expr::Field(
                Box::new(Expr::Record(vec![("a".into(), Expr::Lit(7))])),
                "a".into(),
            ),
            TyCtx::new(),
        ),
        (
            "ctor_nullary",
            Expr::Ctor(opt.clone(), "None".into(), vec![]),
            TyCtx::new(),
        ),
        (
            "ctor_payload",
            Expr::Ctor(opt.clone(), "Some".into(), vec![Expr::Lit(3)]),
            TyCtx::new(),
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
            TyCtx::new(),
        ),
        (
            "lam_app",
            Expr::App(
                Box::new(Expr::Lam("x".into(), Box::new(Expr::Var("x".into())))),
                Box::new(Expr::Lit(5)),
            ),
            TyCtx::new(),
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
            TyCtx::new(),
        ),
        (
            "and",
            Expr::And(Box::new(Expr::Lit(1)), Box::new(Expr::StrLit("s".into()))),
            TyCtx::new(),
        ),
        ("deep_list", deep, TyCtx::new()),
    ]
}

/// `then` on value expressions is refused, and that is the specification.
///
/// ADR-0010 gives `then` as sequential composition (∘, the kernel's
/// `RealizesComp`) with the precondition stated outright: *endpoints must
/// meet*. Every typing derivation runs `Judged(Γ, e) → Type(t)`, so for two
/// value expressions the left lands on a type and the right starts at a
/// judgement — they cannot meet, and `CompositionEndpointMismatch` is the
/// right answer rather than a defect.
///
/// It is pinned **negatively** because the alternative readings are both bad.
/// If a later change makes this succeed, either `then` acquired a meaning
/// nobody decided on, or some coercion leaf was inserted to make the endpoints
/// meet — the padded step #287 removed as unsound. `then` becomes usable when
/// a regime exists whose derivations do not start at their own expression (an
/// ordering witness starts at a *value*), which is what `g_then_split` landing
/// on `da.src()` rather than the operand already anticipates.
#[test]
fn then_on_values_is_refused_because_the_endpoints_cannot_meet() {
    let e = Expr::Then(
        Box::new(Expr::Var("n".into())),
        Box::new(Expr::Var("s".into())),
    );
    assert!(matches!(
        audited_type_check_tree(&e, &bound_ctx()),
        Err(TypeError::CompositionEndpointMismatch)
    ));
}

#[test]
fn derivations_are_unchanged() {
    let corpus = corpus();
    assert_eq!(
        corpus.len(),
        EXPECTED.len(),
        "every corpus entry must be pinned; a new one needs its identities captured"
    );

    for ((name, expr, ctx), (expected_name, witness, proposition)) in corpus.iter().zip(EXPECTED) {
        assert_eq!(
            name, expected_name,
            "corpus and expectations must stay aligned"
        );

        let (judgement, derivation) = audited_type_check_tree(expr, ctx)
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
