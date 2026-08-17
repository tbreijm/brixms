//! Frozen canonical ordinals for every `Expr` and `Ty` constructor —
//! closing [`Type_Realization_Contract.md`] §1.3.
//!
//! **What was unguarded.** §1.1, §1.3 and §1.4 require the canonical encoding
//! of typing inputs to be stable and its constructor ordinals append-only. The
//! `ordinal()` methods and doc comments said so; nothing tested it. No vector
//! covered `Expr` or `Ty`, so renumbering `Expr::Match` from 10 to 13, or
//! swapping `Ty::Rec` and `Ty::RecVar`, would have passed CI clean. #296–#298
//! added four constructors in a week, which is the rate that makes this matter.
//!
//! ADR-0025 Stage A narrowed it — ten specific *values* are pinned with
//! re-derivation tests — but a constructor none of those ten exercises was
//! still unguarded. This file is over the **constructor set**.
//!
//! **Two consumers, as `vectors/`'s discipline requires.**
//!
//! 1. `constructor_vectors_are_frozen` — the production encoder must keep
//!    reproducing the committed manifest (regenerate with `BLESS_VECTORS=1`).
//! 2. `every_digest_is_reproduced_by_primitive_canon_writes` — every digest is
//!    rebuilt from a **declared** ordinal using primitive `CanonWriter` calls,
//!    never calling `Expr`'s or `Ty`'s own `canon_write`. The ordinals are
//!    written out here as literals, so a renumbering in `type_realization.rs`
//!    disagrees with this file rather than silently travelling with it.
//!
//! **What forces a new constructor to appear here.** `constructor_name` is an
//! exhaustive `match`, so adding a variant to `Expr` or `Ty` **fails to
//! compile** until an arm is written — and the arm sits directly beside the
//! exemplar list. That is a compile error rather than a test failure, which is
//! the strongest guard available without a derive macro.
//!
//! The residual gap, stated rather than glossed: a developer who adds the arm
//! but not the exemplar gets a passing suite. The contiguity check below
//! narrows even that — a new constructor takes the next ordinal, so omitting
//! its exemplar leaves the covered set short of the maximum only if the
//! ordinal is skipped. Nothing forces the exemplar itself. §12.3 of the
//! contract records the obligation.

use std::path::PathBuf;

use brix_canon::{CanonWriter, Canonical};
use soc_regimes::type_realization::{ArithOp, CmpOp, Expr, Pattern, Ty};

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("constructor_ordinals_v1.json")
}

/// The constructor's name **and** its declared canonical ordinal.
///
/// Exhaustive on purpose: adding a variant to either enum breaks this match,
/// and the compiler points at the one place that must be updated. When you add
/// an arm here, add an exemplar to `EXPR_EXEMPLARS` / `TY_EXEMPLARS` below.
///
/// The ordinal is **declared here as a literal**, not read from the encoder.
/// That is the point — consumer 2 rebuilds each digest from this number, so if
/// `type_realization.rs` renumbers a constructor, the rebuilt digest stops
/// matching the real one and the disagreement surfaces here.
fn expr_constructor(e: &Expr) -> (&'static str, u64) {
    match e {
        Expr::Lit(_) => ("Expr::Lit", 0),
        Expr::Var(_) => ("Expr::Var", 1),
        Expr::App(_, _) => ("Expr::App", 2),
        Expr::Lam(_, _) => ("Expr::Lam", 3),
        Expr::Record(_) => ("Expr::Record", 4),
        Expr::Field(_, _) => ("Expr::Field", 5),
        Expr::StrLit(_) => ("Expr::StrLit", 6),
        Expr::FloatLit(_) => ("Expr::FloatLit", 7),
        Expr::Arith(_, _, _) => ("Expr::Arith", 8),
        Expr::Ctor(_, _, _) => ("Expr::Ctor", 9),
        Expr::Match(_, _) => ("Expr::Match", 10),
        Expr::BoolLit(_) => ("Expr::BoolLit", 11),
        Expr::Cmp(_, _, _) => ("Expr::Cmp", 12),
        Expr::Then(_, _) => ("Expr::Then", 13),
        Expr::And(_, _) => ("Expr::And", 14),
        Expr::LamAnn(_, _, _) => ("Expr::LamAnn", 15),
        Expr::Fix(_, _, _) => ("Expr::Fix", 16),
    }
}

fn ty_constructor(t: &Ty) -> (&'static str, u64) {
    match t {
        Ty::Con(_) => ("Ty::Con", 0),
        Ty::Fn(_, _) => ("Ty::Fn", 1),
        Ty::Var(_) => ("Ty::Var", 2),
        Ty::Record(_) => ("Ty::Record", 3),
        Ty::Sum(_, _) => ("Ty::Sum", 4),
        Ty::Rec(_, _) => ("Ty::Rec", 5),
        Ty::RecVar(_) => ("Ty::RecVar", 6),
        Ty::Param(_) => ("Ty::Param", 7),
        Ty::Prod(_, _) => ("Ty::Prod", 8),
    }
}

/// One exemplar per `Expr` constructor. Payloads are deliberately minimal —
/// this file freezes *ordinals and framing*, not the payload encodings, which
/// their own values' digests cover.
fn expr_exemplars() -> Vec<Expr> {
    vec![
        Expr::Lit(7),
        Expr::Var("x".into()),
        Expr::App(Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        Expr::Lam("p".into(), Box::new(Expr::Lit(1))),
        Expr::Record(vec![("a".into(), Expr::Lit(1))]),
        Expr::Field(Box::new(Expr::Var("r".into())), "a".into()),
        Expr::StrLit("s".into()),
        Expr::FloatLit("1.5".into()),
        Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        Expr::Ctor(sum_ty(), "None".into(), vec![]),
        Expr::Match(
            Box::new(Expr::Var("s".into())),
            vec![(Pattern::Wildcard, Expr::Lit(0))],
        ),
        Expr::BoolLit(true),
        Expr::Cmp(CmpOp::Eq, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        Expr::Then(Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        Expr::And(Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2))),
        Expr::LamAnn("p".into(), Ty::Con("Int"), Box::new(Expr::Lit(1))),
        Expr::Fix("f".into(), Ty::Con("Int"), Box::new(Expr::Lit(1))),
    ]
}

fn ty_exemplars() -> Vec<Ty> {
    vec![
        Ty::Con("Int"),
        Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))),
        Ty::Var(0),
        Ty::Record(vec![("a".into(), Ty::Con("Int"))]),
        sum_ty(),
        Ty::Rec("L".into(), Box::new(Ty::RecVar("L".into()))),
        Ty::RecVar("L".into()),
        Ty::Param("T".into()),
        Ty::Prod(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Str"))),
    ]
}

fn sum_ty() -> Ty {
    Ty::Sum(
        "Opt".into(),
        vec![
            ("None".into(), vec![]),
            ("Some".into(), vec![Ty::Con("Int")]),
        ],
    )
}

fn digest_of(v: &impl Canonical) -> String {
    let mut w = CanonWriter::new();
    v.canon_write(&mut w);
    brix_canon::Digest::of(brix_canon::Domain::Value, &w.finish()).to_hex()
}

fn build_manifest() -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.regime.constructor-ordinals\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"contract\": \"Type_Realization_Contract.md #1.3\",\n");
    out.push_str(
        "  \"note\": \"Canonical ordinals for every Expr and Ty constructor. These are \
         append-only ABI: an ordinal is never renumbered or reused. A diff here means an \
         encoding change, which invalidates every derivation digest and every kernel row \
         built from one.\",\n",
    );

    for (label, rows) in [
        (
            "expr",
            expr_exemplars()
                .iter()
                .map(|e| {
                    let (name, ord) = expr_constructor(e);
                    (name, ord, digest_of(e))
                })
                .collect::<Vec<_>>(),
        ),
        (
            "ty",
            ty_exemplars()
                .iter()
                .map(|t| {
                    let (name, ord) = ty_constructor(t);
                    (name, ord, digest_of(t))
                })
                .collect::<Vec<_>>(),
        ),
    ] {
        out.push_str(&format!("  \"{label}\": [\n"));
        for (i, (name, ord, digest)) in rows.iter().enumerate() {
            out.push_str("    {\n");
            out.push_str(&format!("      \"constructor\": \"{name}\",\n"));
            out.push_str(&format!("      \"ordinal\": {ord},\n"));
            out.push_str(&format!("      \"exemplar_config_id\": \"{digest}\"\n"));
            out.push_str(if i + 1 == rows.len() {
                "    }\n"
            } else {
                "    },\n"
            });
        }
        out.push_str(if label == "ty" { "  ]\n" } else { "  ],\n" });
    }
    out.push_str("}\n");
    out
}

/// Consumer 1: the production encoder still reproduces the committed manifest.
#[test]
fn constructor_vectors_are_frozen() {
    let path = manifest_path();
    let generated = build_manifest();
    let committed = std::fs::read_to_string(&path).unwrap_or_default();

    if generated == committed {
        return;
    }
    if std::env::var_os("BLESS_VECTORS").is_some() {
        std::fs::write(&path, &generated).expect("vector manifest is writable");
        return;
    }
    // Deliberately do NOT write on the failing path: the CI determinism job
    // re-runs the suite and requires a clean working tree.
    panic!(
        "constructor ordinals drifted from {}.\n\
         `Expr`/`Ty` canonical ordinals are append-only ABI. Renumbering one \
         invalidates every derivation digest and every kernel row built from a \
         type or expression identity, and old certificates stop verifying.\n\
         If this is a genuinely new constructor taking the next unused ordinal, \
         regenerate with BLESS_VECTORS=1 and say so loudly in the PR.\n\n\
         generated:\n{generated}",
        path.display()
    );
}

/// Consumer 2: **every** digest rebuilt from the **declared** ordinal with
/// primitive `CanonWriter` calls, never calling `Expr`/`Ty`'s `canon_write`.
///
/// This is what makes the ordinals load-bearing rather than decorative.
/// Consumer 1 catches a renumbering that was not blessed; this catches one that
/// *was* — where the encoder and the frozen file moved together because someone
/// ran `BLESS_VECTORS=1` on a change that should not have been blessed.
///
/// All twenty constructors, not a sample. An earlier draft of this test
/// reconstructed seven, which would have left thirteen constructors covered by
/// consumer 1 alone — the same partial-corpus failure this file exists to fix,
/// reproduced inside its own guard.
#[test]
fn every_digest_is_reproduced_by_primitive_canon_writes() {
    fn hash(w: CanonWriter) -> String {
        brix_canon::Digest::of(brix_canon::Domain::Value, &w.finish()).to_hex()
    }
    fn build(f: impl FnOnce(&mut CanonWriter)) -> String {
        let mut w = CanonWriter::new();
        f(&mut w);
        hash(w)
    }
    // The two payloads reused below, spelled once each.
    fn lit(w: &mut CanonWriter, v: i64) {
        w.write_enum(0, |w| w.write_int(v));
    }
    fn opt_sum(w: &mut CanonWriter) {
        w.write_enum(4, |w| {
            w.write_str("Opt");
            w.write_uint(2);
            w.write_str("None");
            w.write_uint(0);
            w.write_str("Some");
            w.write_uint(1);
            w.write_enum(0, |w| w.write_str("Int"));
        });
    }

    let expected: Vec<(&str, String)> = vec![
        ("Expr::Lit", build(|w| lit(w, 7))),
        (
            "Expr::Var",
            build(|w| w.write_enum(1, |w| w.write_str("x"))),
        ),
        (
            "Expr::App",
            build(|w| {
                w.write_enum(2, |w| {
                    lit(w, 1);
                    lit(w, 2);
                })
            }),
        ),
        (
            "Expr::Lam",
            build(|w| {
                w.write_enum(3, |w| {
                    w.write_str("p");
                    lit(w, 1);
                })
            }),
        ),
        (
            "Expr::Record",
            build(|w| {
                w.write_enum(4, |w| {
                    w.write_uint(1);
                    w.write_str("a");
                    lit(w, 1);
                })
            }),
        ),
        (
            "Expr::Field",
            build(|w| {
                w.write_enum(5, |w| {
                    w.write_enum(1, |w| w.write_str("r"));
                    w.write_str("a");
                })
            }),
        ),
        (
            "Expr::StrLit",
            build(|w| w.write_enum(6, |w| w.write_str("s"))),
        ),
        (
            "Expr::FloatLit",
            build(|w| w.write_enum(7, |w| w.write_str("1.5"))),
        ),
        (
            // The operator is a bare uint, which is also what `ArithOp`'s
            // standalone encoding must be — ⟨D-OPPROJECT⟩'s premise, pinned
            // from the expression side.
            "Expr::Arith",
            build(|w| {
                w.write_enum(8, |w| {
                    w.write_uint(0);
                    lit(w, 1);
                    lit(w, 2);
                })
            }),
        ),
        (
            "Expr::Ctor",
            build(|w| {
                w.write_enum(9, |w| {
                    opt_sum(w);
                    w.write_str("None");
                    w.write_uint(0);
                })
            }),
        ),
        (
            "Expr::Match",
            build(|w| {
                w.write_enum(10, |w| {
                    w.write_enum(1, |w| w.write_str("s"));
                    w.write_uint(1);
                    w.write_enum(0, |_w| {}); // Pattern::Wildcard
                    lit(w, 0);
                })
            }),
        ),
        (
            "Expr::BoolLit",
            build(|w| w.write_enum(11, |w| w.write_bool(true))),
        ),
        (
            "Expr::Cmp",
            build(|w| {
                w.write_enum(12, |w| {
                    w.write_uint(4); // CmpOp::Eq
                    lit(w, 1);
                    lit(w, 2);
                })
            }),
        ),
        (
            "Ty::Con",
            build(|w| w.write_enum(0, |w| w.write_str("Int"))),
        ),
        (
            "Ty::Fn",
            build(|w| {
                w.write_enum(1, |w| {
                    w.write_enum(0, |w| w.write_str("Int"));
                    w.write_enum(0, |w| w.write_str("Bool"));
                })
            }),
        ),
        ("Ty::Var", build(|w| w.write_enum(2, |w| w.write_uint(0)))),
        (
            "Ty::Record",
            build(|w| {
                w.write_enum(3, |w| {
                    w.write_uint(1);
                    w.write_str("a");
                    w.write_enum(0, |w| w.write_str("Int"));
                })
            }),
        ),
        ("Ty::Sum", build(opt_sum)),
        (
            "Ty::Rec",
            build(|w| {
                w.write_enum(5, |w| {
                    w.write_str("L");
                    w.write_enum(6, |w| w.write_str("L"));
                })
            }),
        ),
        (
            "Ty::RecVar",
            build(|w| w.write_enum(6, |w| w.write_str("L"))),
        ),
        (
            "Ty::Param",
            build(|w| w.write_enum(7, |w| w.write_str("T"))),
        ),
        (
            "Ty::Prod",
            build(|w| {
                w.write_enum(8, |w| {
                    w.write_enum(0, |w| w.write_str("Int"));
                    w.write_enum(0, |w| w.write_str("Str"));
                })
            }),
        ),
        (
            "Expr::Then",
            build(|w| {
                w.write_enum(13, |w| {
                    w.write_enum(0, |w| w.write_int(1));
                    w.write_enum(0, |w| w.write_int(2));
                })
            }),
        ),
        (
            "Expr::And",
            build(|w| {
                w.write_enum(14, |w| {
                    w.write_enum(0, |w| w.write_int(1));
                    w.write_enum(0, |w| w.write_int(2));
                })
            }),
        ),
        (
            "Expr::LamAnn",
            build(|w| {
                w.write_enum(15, |w| {
                    w.write_str("p");
                    w.write_enum(0, |w| w.write_str("Int"));
                    w.write_enum(0, |w| w.write_int(1));
                })
            }),
        ),
        (
            "Expr::Fix",
            build(|w| {
                w.write_enum(16, |w| {
                    w.write_str("f");
                    w.write_enum(0, |w| w.write_str("Int"));
                    w.write_enum(0, |w| w.write_int(1));
                })
            }),
        ),
    ];

    let actual: Vec<(&str, String)> = expr_exemplars()
        .iter()
        .map(|e| (expr_constructor(e).0, digest_of(e)))
        .chain(
            ty_exemplars()
                .iter()
                .map(|t| (ty_constructor(t).0, digest_of(t))),
        )
        .collect();

    // Every constructor is covered, so a new one cannot slip past this test by
    // being absent from the reconstruction list.
    assert_eq!(
        expected.len(),
        actual.len(),
        "the independent reconstruction must cover every constructor"
    );
    for (name, digest) in &actual {
        let (_, want) = expected
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} has no independent reconstruction"));
        assert_eq!(
            digest, want,
            "{name}: the encoder disagrees with the ordinal declared in this file"
        );
    }
}

/// Ordinals are contiguous from zero, with no gap and no duplicate.
///
/// A gap means an ordinal was skipped — usually a variant removed rather than
/// deprecated, which is how a future append reuses a retired number and
/// silently reinterprets old bytes.
#[test]
fn ordinals_are_contiguous_and_distinct() {
    for (label, mut ords) in [
        (
            "Expr",
            expr_exemplars()
                .iter()
                .map(|e| expr_constructor(e).1)
                .collect::<Vec<_>>(),
        ),
        (
            "Ty",
            ty_exemplars()
                .iter()
                .map(|t| ty_constructor(t).1)
                .collect::<Vec<_>>(),
        ),
    ] {
        let n = ords.len() as u64;
        ords.sort_unstable();
        ords.dedup();
        assert_eq!(
            ords.len() as u64,
            n,
            "{label}: two constructors declare the same ordinal"
        );
        assert_eq!(
            ords,
            (0..n).collect::<Vec<_>>(),
            "{label}: ordinals must be contiguous from 0 — a gap lets a future \
             append reuse a retired number"
        );
    }
}

/// Every exemplar is a distinct constructor, and every digest is distinct.
///
/// Guards the exemplar list itself: two exemplars of the same constructor would
/// leave another constructor unexercised while the counts still looked right.
#[test]
fn each_constructor_is_exercised_exactly_once() {
    let mut names: Vec<&str> = expr_exemplars()
        .iter()
        .map(|e| expr_constructor(e).0)
        .chain(ty_exemplars().iter().map(|t| ty_constructor(t).0))
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "a constructor has two exemplars");

    let mut digests: Vec<String> = expr_exemplars()
        .iter()
        .map(digest_of)
        .chain(ty_exemplars().iter().map(digest_of))
        .collect();
    digests.sort();
    digests.dedup();
    assert_eq!(digests.len(), total, "two exemplars share a digest");
}
