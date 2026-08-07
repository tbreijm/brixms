//! SLICE 2 of the native type-realization regime (ADR-0005 Stage 2).
//!
//! Native type inference as SOC realization: produces real `HasType` `Derived`
//! judgements through the SOC proof substrate with App/Lam typing, declarative
//! unification, and multi-step composed derivations.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical};
use brix_elaborate::{RealizesTree, TreeObj};
use brix_semantic::{
    compose_chain, ConfigId, ContextId, Decomposition, Evidence, GeneratorId, Judgement, Outcome,
    Realizes, WitnessId,
};

/// Native representation of types in the type-realization regime (ADR-0005).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ty {
    Con(&'static str),
    Fn(Box<Ty>, Box<Ty>),
    Var(u32),
    Record(Vec<(String, Ty)>),
    Sum(String, Vec<(String, Vec<Ty>)>),
}

impl Canonical for Ty {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Ty::Con(name) => {
                w.write_enum(0, |w| w.write_str(name));
            }
            Ty::Fn(param, ret) => {
                w.write_enum(1, |w| {
                    param.canon_write(w);
                    ret.canon_write(w);
                });
            }
            Ty::Var(v) => {
                w.write_enum(2, |w| w.write_uint(*v as u64));
            }
            Ty::Record(fields) => {
                w.write_enum(3, |w| {
                    let mut sorted = fields.clone();
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                    sorted.dedup_by(|a, b| a.0 == b.0);
                    w.write_uint(sorted.len() as u64);
                    for (name, ty) in &sorted {
                        w.write_str(name);
                        ty.canon_write(w);
                    }
                });
            }
            Ty::Sum(sum_name, variants) => {
                w.write_enum(4, |w| {
                    w.write_str(sum_name);
                    w.write_uint(variants.len() as u64);
                    for (vname, fields) in variants {
                        w.write_str(vname);
                        w.write_uint(fields.len() as u64);
                        for f in fields {
                            f.canon_write(w);
                        }
                    }
                });
            }
        }
    }
}

impl Ty {
    /// Content-addressed type identity (`ConfigId`).
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

/// Binary numeric arithmetic operators. All four share one typing rule
/// (numeric operands in, numeric result out) — the op only affects runtime
/// value, not the type — so they carry a single `g_arith` generator.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOp {
    fn ordinal(self) -> u64 {
        match self {
            ArithOp::Add => 0,
            ArithOp::Sub => 1,
            ArithOp::Mul => 2,
            ArithOp::Div => 3,
        }
    }
}

/// Pattern representation for match expressions (ADR-0011 Slice 2).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Ctor(String, Vec<Pattern>),
}

impl Canonical for Pattern {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Pattern::Wildcard => {
                w.write_enum(0, |_w| {});
            }
            Pattern::Var(name) => {
                w.write_enum(1, |w| w.write_str(name));
            }
            Pattern::Ctor(variant, subpats) => {
                w.write_enum(2, |w| {
                    w.write_str(variant);
                    w.write_uint(subpats.len() as u64);
                    for p in subpats {
                        p.canon_write(w);
                    }
                });
            }
        }
    }
}

/// Expression AST for the native type-realization regime (ADR-0005).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Expr {
    Lit(i64),
    Var(String),
    App(Box<Expr>, Box<Expr>),
    Lam(String, Box<Expr>),
    Record(Vec<(String, Expr)>),
    Field(Box<Expr>, String),
    /// String literal (append-only ordinal 6). Types to `Str`.
    StrLit(String),
    /// Floating-point literal, stored as its source text for canonical identity
    /// (append-only ordinal 7). Types to `Float`.
    FloatLit(String),
    /// Binary numeric arithmetic (append-only ordinal 8). Types to `Int`,
    /// `Float`, or — on a mixed `Int`/`Float` — `Float` via a promotion witness.
    Arith(ArithOp, Box<Expr>, Box<Expr>),
    /// Sum constructor application (append-only ordinal 9).
    Ctor(Ty, String, Vec<Expr>),
    /// Pattern match expression (append-only ordinal 10).
    Match(Box<Expr>, Vec<(Pattern, Expr)>),
}

impl Canonical for Expr {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Expr::Lit(val) => {
                w.write_enum(0, |w| w.write_int(*val));
            }
            Expr::Var(name) => {
                w.write_enum(1, |w| w.write_str(name));
            }
            Expr::App(f, x) => {
                w.write_enum(2, |w| {
                    f.canon_write(w);
                    x.canon_write(w);
                });
            }
            Expr::Lam(param, body) => {
                w.write_enum(3, |w| {
                    w.write_str(param);
                    body.canon_write(w);
                });
            }
            Expr::Record(fields) => {
                w.write_enum(4, |w| {
                    let mut sorted = fields.clone();
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                    sorted.dedup_by(|a, b| a.0 == b.0);
                    w.write_uint(sorted.len() as u64);
                    for (name, val) in &sorted {
                        w.write_str(name);
                        val.canon_write(w);
                    }
                });
            }
            Expr::Field(base, fname) => {
                w.write_enum(5, |w| {
                    base.canon_write(w);
                    w.write_str(fname);
                });
            }
            Expr::StrLit(s) => {
                w.write_enum(6, |w| w.write_str(s));
            }
            Expr::FloatLit(s) => {
                w.write_enum(7, |w| w.write_str(s));
            }
            Expr::Arith(op, a, b) => {
                w.write_enum(8, |w| {
                    w.write_uint(op.ordinal());
                    a.canon_write(w);
                    b.canon_write(w);
                });
            }
            Expr::Ctor(sum_ty, variant, args) => {
                w.write_enum(9, |w| {
                    sum_ty.canon_write(w);
                    w.write_str(variant);
                    w.write_uint(args.len() as u64);
                    for a in args {
                        a.canon_write(w);
                    }
                });
            }
            Expr::Match(scrutinee, arms) => {
                w.write_enum(10, |w| {
                    scrutinee.canon_write(w);
                    w.write_uint(arms.len() as u64);
                    for (p, b) in arms {
                        p.canon_write(w);
                        b.canon_write(w);
                    }
                });
            }
        }
    }
}

impl Expr {
    /// Content-addressed expression identity (`ConfigId`).
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

/// Typing-rule generator for literal constants (`"type.rule.lit@1"`).
pub fn g_lit() -> GeneratorId {
    GeneratorId::named("type.rule.lit@1")
}

/// Typing-rule generator for string literals (`"type.rule.strlit@1"`).
pub fn g_str_lit() -> GeneratorId {
    GeneratorId::named("type.rule.strlit@1")
}

/// Typing-rule generator for variable lookups (`"type.rule.var@1"`).
pub fn g_var() -> GeneratorId {
    GeneratorId::named("type.rule.var@1")
}

/// Typing-rule generator for function application (`"type.rule.app@1"`).
pub fn g_app() -> GeneratorId {
    GeneratorId::named("type.rule.app@1")
}

/// Typing-rule generator for lambda abstraction (`"type.rule.lam@1"`).
pub fn g_lam() -> GeneratorId {
    GeneratorId::named("type.rule.lam@1")
}

/// Typing-rule generator for lambda abstraction introduction (`"type.rule.lam.intro@1"`).
pub fn g_lam_intro() -> GeneratorId {
    GeneratorId::named("type.rule.lam.intro@1")
}

/// Typing-rule generator for lambda abstraction closure (`"type.rule.lam.close@1"`).
pub fn g_lam_close() -> GeneratorId {
    GeneratorId::named("type.rule.lam.close@1")
}

/// Typing-rule generator for application splitting (`"type.rule.app.split@1"`).
pub fn g_split() -> GeneratorId {
    GeneratorId::named("type.rule.app.split@1")
}

/// Typing-rule generator for binary application (`"type.rule.app@2"`).
pub fn g_app2() -> GeneratorId {
    GeneratorId::named("type.rule.app@2")
}

/// Typing-rule generator for record literals (`"type.rule.record@1"`).
pub fn g_record() -> GeneratorId {
    GeneratorId::named("type.rule.record@1")
}

/// Typing-rule generator for zero-field record literals (`"type.rule.record.empty@1"`).
///
/// This remains undischarged: the current kernel profile has binary products but
/// no terminal/unit proposition, so `{}` is not an instance of product
/// introduction yet.
pub fn g_record_empty() -> GeneratorId {
    GeneratorId::named("type.rule.record.empty@1")
}

/// Typing-rule generator for record field access (`"type.rule.field@1"`).
pub fn g_field() -> GeneratorId {
    GeneratorId::named("type.rule.field@1")
}

/// Typing-rule generator for field-expression decomposition (`"type.rule.field.split@1"`).
pub fn g_field_split() -> GeneratorId {
    GeneratorId::named("type.rule.field.split@1")
}

/// Typing-rule generator for record splitting (`"type.rule.record.split@1"`).
pub fn g_record_split() -> GeneratorId {
    GeneratorId::named("type.rule.record.split@1")
}

/// Typing-rule generator for sum constructor application (`"type.rule.ctor@1"`).
pub fn g_ctor() -> GeneratorId {
    GeneratorId::named("type.rule.ctor@1")
}

/// Typing-rule generator for nullary sum constructors (`"type.rule.ctor.nullary@1"`).
///
/// This remains undischarged: there is no payload proof to inject, and the
/// kernel profile has no nullary/zero coproduct introduction rule.
pub fn g_ctor_nullary() -> GeneratorId {
    GeneratorId::named("type.rule.ctor.nullary@1")
}

/// Typing-rule generator for constructor splitting (`"type.rule.ctor.split@1"`).
pub fn g_ctor_split() -> GeneratorId {
    GeneratorId::named("type.rule.ctor.split@1")
}

/// Typing-rule generator for match expressions (`"type.rule.match@1"`).
pub fn g_match() -> GeneratorId {
    GeneratorId::named("type.rule.match@1")
}

/// Typing-rule generator for a top-level wildcard/variable catch-all match arm
/// (`"type.rule.match.catchall@1"`).
///
/// This remains undischarged. A catch-all arm can stand for several coproduct
/// branches, but the current realization tree records that arm only once; it
/// therefore is not yet the kernel's explicit `Case` premise structure.
pub fn g_match_catchall() -> GeneratorId {
    GeneratorId::named("type.rule.match.catchall@1")
}

/// Typing-rule generator for match arm splitting (`"type.rule.match.split@1"`).
pub fn g_match_split() -> GeneratorId {
    GeneratorId::named("type.rule.match.split@1")
}

/// Typing-rule generator for float literals (`"type.rule.floatlit@1"`).
pub fn g_float_lit() -> GeneratorId {
    GeneratorId::named("type.rule.floatlit@1")
}

/// Typing-rule generator for binary numeric arithmetic (`"type.rule.arith@1"`).
pub fn g_arith() -> GeneratorId {
    GeneratorId::named("type.rule.arith@1")
}

/// Typing-rule generator for arithmetic operand splitting (`"type.rule.arith.split@1"`).
pub fn g_arith_split() -> GeneratorId {
    GeneratorId::named("type.rule.arith.split@1")
}

/// Typing-rule generator for unification steps (`"type.rule.unify@1"`).
pub fn g_unify() -> GeneratorId {
    GeneratorId::named("type.rule.unify@1")
}

// ---------------------------------------------------------------------------
// Honest outcome propagation (the SOC tight-generator obligation, ADR-0009/0010).
//
// The proof kernel certifies the *composition* theorem: given the primitive
// typing-rule leaves as generators, the derivation establishes `e : T`. It does
// NOT yet prove the semantic validity of those leaves themselves. In SOC an
// outcome is monotone over composition — a judgement is only as strong as its
// weakest generator — so the honest status of the typing *result* is the meet of
// the composition outcome and every leaf generator's discharge status. Until a
// leaf is discharged to "tight" (a kernel proof of its realization semantics),
// it caps the result at the replay-verified `Audited`. Discharging generators
// one at a time then lifts real programs to `Proven` automatically, with no
// change to this reporting code.
// ---------------------------------------------------------------------------

/// Whether a typing-rule generator's *realization semantics* has been discharged
/// to "tight" — its soundness established, not merely asserted (the
/// tight-generator obligation).
///
/// Discharged:
///
/// 1. The **literal introduction rules** `g_lit`/`g_str_lit`/`g_float_lit` — an
///    introduction rule *is* the definition of its type (`42 : Int`), the
///    irreducible axiom a type theory rests on. Emission-pinned by
///    `literal_intro_generators_are_faithful`.
/// 2. The **simply-typed λ-calculus core** `g_var` (hypothesis), `g_lam_intro` /
///    `g_lam_close` (→I), `g_split` / `g_app2` (→E). These are the *defining
///    rules* of the STLC — likewise the axiomatic base of the type theory — and
///    each corresponds directly to a **primitive natural-deduction rule the
///    proof kernel already accepts as sound**: `g_var` ↔ `Hyp`, `g_lam_*` ↔
///    `Lam` (→I), `g_split`/`g_app2` ↔ product elimination + `App` (→E, i.e.
///    modus ponens `(A→B)×A → B`, a kernel theorem — see
///    `application_rule_is_a_kernel_theorem`). Discharging them is honest: their
///    realization *is* the kernel's own primitive, not a fresh assertion. As a
///    result the pure λ-calculus fragment (`id(42)` etc.) reaches genuine
///    `Proven`.
///
/// 3. The **structural product/coproduct fragment**: `g_record*`/`g_field*`
///    (product introduction/elimination) and `g_ctor*`/`g_match*` (coproduct
///    introduction/elimination). The `*_split` leaves are the canonical
///    structural packaging of an expression's premises into the same
///    right-nested product shape used by the kernel rule; they carry no
///    operation or representation claim of their own. Their emissions, and the
///    four corresponding kernel theorems, are pinned by
///    `structural_generators_are_faithful_kernel_rules`. Consequently records,
///    field projections, constructors, and well-typed matches made solely from
///    this fragment can earn `Proven`.
///    Top-level wildcard/variable catch-all matches are excluded: they emit the
///    distinct, undischarged `g_match_catchall` until their repeated branch
///    premises are represented explicitly.
///
/// Deliberately **NOT** discharged — these assert *operation/representation*
/// semantics, not logical rules, and have no established value semantics yet
/// (they wait for the execution layer): `g_arith` (a primitive operation is
/// total & type-preserving) and the coercion-edge promotions `g_promote_edge`
/// (a real numeric embedding). Any program touching an undischarged generator
/// stays `Audited`; e.g. `1 + 2` remains `Audited` because `g_arith` is not
/// tight.
pub fn generator_is_tight(g: &GeneratorId) -> bool {
    *g == g_lit()
        || *g == g_str_lit()
        || *g == g_float_lit()
        || *g == g_var()
        || *g == g_lam_intro()
        || *g == g_lam_close()
        || *g == g_split()
        || *g == g_app2()
        || *g == g_record_split()
        || *g == g_record()
        || *g == g_field_split()
        || *g == g_field()
        || *g == g_ctor_split()
        || *g == g_ctor()
        || *g == g_match_split()
        || *g == g_match()
}

/// Whether every leaf generator in an elaborated derivation is discharged tight.
fn all_generators_tight(tree: &RealizesTree) -> bool {
    match tree {
        RealizesTree::Leaf { generator, .. } => generator_is_tight(generator),
        RealizesTree::Seq { left, right } | RealizesTree::Tensor { left, right } => {
            all_generators_tight(left) && all_generators_tight(right)
        }
    }
}

/// The honest epistemic status of a typing *result* `e : T`: the composition
/// outcome capped by the least-discharged leaf. The kernel proves `composition`
/// (e.g. `Proven`) *conditional on* the primitive typing-rule leaves, so the
/// result is only `composition` when every leaf is tight; otherwise it is the
/// replay-verified `Audited`.
pub fn honest_result_outcome(composition: Outcome, tree: &RealizesTree) -> Outcome {
    if all_generators_tight(tree) {
        composition
    } else {
        Outcome::Audited
    }
}

/// A short, human-readable name for a typing-rule generator (ADR-0010 L4:
/// `brix why`/`brix whynot`, issue #43). `GeneratorId` is a one-way content
/// hash (`GeneratorId::named`), so there is no way back from a digest to a
/// name in general; this is a reverse lookup over the *closed* set of
/// generators this regime is known to mint — exactly the set
/// `generator_is_tight` above classifies, plus the open-ended `NUMERIC`/
/// `GRADE` coercion edges. Returns `None` for a generator this module did not
/// mint (a caller should fall back to the raw digest).
pub fn generator_name(g: &GeneratorId) -> Option<String> {
    let named: &[(&str, GeneratorId)] = &[
        ("g_lit", g_lit()),
        ("g_str_lit", g_str_lit()),
        ("g_float_lit", g_float_lit()),
        ("g_var", g_var()),
        ("g_app", g_app()),
        ("g_lam", g_lam()),
        ("g_lam_intro", g_lam_intro()),
        ("g_lam_close", g_lam_close()),
        ("g_split", g_split()),
        ("g_app2", g_app2()),
        ("g_record", g_record()),
        ("g_record_empty", g_record_empty()),
        ("g_field", g_field()),
        ("g_field_split", g_field_split()),
        ("g_record_split", g_record_split()),
        ("g_ctor", g_ctor()),
        ("g_ctor_nullary", g_ctor_nullary()),
        ("g_ctor_split", g_ctor_split()),
        ("g_match", g_match()),
        ("g_match_catchall", g_match_catchall()),
        ("g_match_split", g_match_split()),
        ("g_arith", g_arith()),
        ("g_arith_split", g_arith_split()),
        ("g_unify", g_unify()),
    ];
    if let Some((name, _)) = named.iter().find(|(_, id)| id == g) {
        return Some((*name).to_string());
    }
    for (from, to) in NUMERIC.edges.iter().copied() {
        if NUMERIC.promote_generator(from, to) == *g {
            return Some(format!("promote({from}->{to})"));
        }
    }
    for (from, to) in GRADE.edges.iter().copied() {
        if GRADE.promote_generator(from, to) == *g {
            return Some(format!("weaken({from}->{to})"));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Coercion lattices (ADR-0010): the general type-normalization mechanism.
//
// A `CoercionLattice` is a declared category of witnessed coercions over one
// *sort* of type name — objects = node names, morphisms = the *safe*
// (information-preserving) coercion generators on its edges, each `from ↪ to`.
// Normalization at a multi-input site = `join` (least upper bound in the ↪
// order) + the composed witness-path from each input up to the join. An edge is
// always the SAFE direction; a required coercion with NO up-path is a real
// conflict (numeric: incomparable types; epistemic: illegal strengthening =
// erasure). Only upward moves are ever inserted implicitly.
//
// The numeric tower and the epistemic-grade modality are two instances of the
// SAME code — `Int ↪ Float` and `Proven ↪ Audited` run through one mechanism.
// Up-paths are unique here (the coherence condition), so composed witnesses are
// well-defined; adding a class is a data change to the `edges`, not new logic.
// ---------------------------------------------------------------------------

/// A declared category of witnessed coercions over one sort of type name.
pub struct CoercionLattice {
    /// Hasse edges `(from, to)` in the *safe* coercion direction; each names a
    /// promotion generator via `generator_prefix`.
    edges: &'static [(&'static str, &'static str)],
    /// Prefix for per-edge generator ids, e.g. `"type.rule.num.promote"`.
    generator_prefix: &'static str,
}

impl CoercionLattice {
    /// Whether `name` is a node of this lattice.
    fn contains(&self, name: &str) -> bool {
        self.edges.iter().any(|(a, b)| *a == name || *b == name)
    }

    /// The canonical `&'static str` node for `name`, if present.
    fn node(&self, name: &str) -> Option<&'static str> {
        self.edges.iter().find_map(|(a, b)| {
            if *a == name {
                Some(*a)
            } else if *b == name {
                Some(*b)
            } else {
                None
            }
        })
    }

    /// The witnessed-coercion generator for the edge `from ↪ to`.
    pub fn promote_generator(&self, from: &str, to: &str) -> GeneratorId {
        GeneratorId::named(&format!("{}.{from}_{to}@1", self.generator_prefix))
    }

    /// Reflexive–transitive upward closure of `name` (includes `name`).
    fn ancestors(&self, name: &'static str) -> Vec<&'static str> {
        let mut out = vec![name];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i];
            for (a, b) in self.edges {
                if *a == cur && !out.contains(b) {
                    out.push(*b);
                }
            }
            i += 1;
        }
        out
    }

    /// `a ≤ b`: a value of type `a` safely coerces to `b`.
    fn le(&self, a: &'static str, b: &'static str) -> bool {
        self.ancestors(a).contains(&b)
    }

    /// The join (least upper bound), or `None` if incomparable — a real
    /// "cannot coerce" error (numeric mismatch or epistemic erasure).
    fn join(&self, a: &'static str, b: &'static str) -> Option<&'static str> {
        let aa = self.ancestors(a);
        let bb = self.ancestors(b);
        let common: Vec<&'static str> = aa.into_iter().filter(|x| bb.contains(x)).collect();
        common
            .iter()
            .copied()
            .find(|&x| common.iter().all(|&y| self.le(x, y)))
    }

    /// The ordered edge path `(from, to)` from `from` up to `to`
    /// (empty if equal). Up-paths are unique, so the composed witness is unique.
    fn edge_path(&self, from: &'static str, to: &'static str) -> Vec<(&'static str, &'static str)> {
        if from == to {
            return Vec::new();
        }
        let mut reached_via: BTreeMap<&'static str, (&'static str, &'static str)> = BTreeMap::new();
        let mut queue = vec![from];
        let mut i = 0;
        while i < queue.len() {
            let cur = queue[i];
            i += 1;
            for (a, b) in self.edges {
                if *a == cur && *b != from && !reached_via.contains_key(b) {
                    reached_via.insert(*b, (*a, *b));
                    if *b == to {
                        queue.clear();
                        break;
                    }
                    queue.push(*b);
                }
            }
        }
        let mut path = Vec::new();
        let mut node = to;
        while node != from {
            let edge = reached_via[node];
            path.push(edge);
            node = edge.0;
        }
        path.reverse();
        path
    }

    /// Wrap a derivation `d` with the witnessed coercion path `from ↪ … ↪ to`,
    /// one `Seq`-composed embedding leaf per edge (identity when `from == to`).
    fn coerce(&self, d: TyTree, from: &'static str, to: &'static str) -> TyTree {
        let mut tree = d;
        for (a, b) in self.edge_path(from, to) {
            tree = TyTree::Seq {
                left: Box::new(tree),
                right: Box::new(TyTree::Leaf {
                    generator: self.promote_generator(a, b),
                    src: TyObj::Atom(CfgAtom::Type(Ty::Con(a))),
                    dst: TyObj::Atom(CfgAtom::Type(Ty::Con(b))),
                }),
            };
        }
        tree
    }
}

/// The numeric coercion tower ℕ⊂ℤ⊂ℚ⊂ℝ⊂ℂ (safe = widening) plus the pragmatic
/// lossy `Int ↪ Float` branch — `Float` is incomparable to the exact ℚ/ℝ/ℂ
/// nodes, so `join(Float, Rat)` is `None` (mixing them is a type error).
pub static NUMERIC: CoercionLattice = CoercionLattice {
    edges: &[
        ("Nat", "Int"),
        ("Int", "Rat"),
        ("Rat", "Real"),
        ("Real", "Complex"),
        ("Int", "Float"),
    ],
    generator_prefix: "type.rule.num.promote",
};

/// The epistemic-grade modality as a coercion lattice. The *safe* direction is
/// weakening certainty (`Proven ↪ Audited ↪ Derived` — a stronger guarantee may
/// always be forgotten). The forbidden strengthening (`Derived → Proven`
/// without evidence) has no up-path, so `join`/`le` reject it: that is exactly
/// **epistemic erasure**, caught by the same code as a numeric mismatch.
pub static GRADE: CoercionLattice = CoercionLattice {
    edges: &[("Proven", "Audited"), ("Audited", "Derived")],
    generator_prefix: "epistemic.grade.weaken",
};

/// Whether an *actual* epistemic grade satisfies an *asserted* one (both as
/// GRADE-lattice node names `"Derived"`/`"Audited"`/`"Proven"`).
///
/// A grade assertion is discharged iff the actual grade may **safely weaken** to
/// the assertion along the GRADE coercion lattice (`Proven ↪ Audited ↪
/// Derived`): you may assert a *weaker-or-equal* grade than you earned
/// (downgrade is free), but asserting a *stronger* grade has no up-path — that is
/// **epistemic erasure** and fails. An `actual` outside the lattice (e.g.
/// `Unknown`) satisfies nothing.
pub fn grade_assertion_satisfied(actual: &str, asserted: &str) -> bool {
    match (GRADE.node(actual), GRADE.node(asserted)) {
        (Some(a), Some(b)) => GRADE.le(a, b),
        // An unknown grade name (e.g. a non-grade outcome) satisfies nothing.
        _ => false,
    }
}

/// The least numeric *field* (closed under division) at or above `name`. The
/// exact field of fractions of `Nat`/`Int` is `Rat`; here it is `Float`, the
/// reachable representation (a fuller tower would return `Rat`).
fn field_of(name: &'static str) -> &'static str {
    match name {
        "Nat" | "Int" => "Float",
        other => other,
    }
}

/// The plan for typing a binary arithmetic node: where operands meet (`base`),
/// the result type (`base`, or its field of fractions for `Div`), each
/// operand's effective type, and whether an operand was an unbound var to unify.
struct ArithPlan {
    base: &'static str,
    result: &'static str,
    eff_a: &'static str,
    eff_b: &'static str,
    unify_a: bool,
    unify_b: bool,
}

/// Classify a *resolved* operand type: `Ok(Some(node))` for a concrete numeric
/// type, `Ok(None)` for an unbound var (defaults to `base`), `Err(Mismatch)`
/// for a concrete non-numeric type (`Str`/`Fn`/`Record`).
fn arith_operand(resolved: &Ty) -> Result<Option<&'static str>, TypeError> {
    match resolved {
        Ty::Con(n) if NUMERIC.contains(n) => Ok(NUMERIC.node(n)),
        Ty::Var(_) => Ok(None),
        _ => Err(TypeError::Mismatch),
    }
}

/// Compute the arithmetic plan for `op` over resolved operand types `ra`, `rb`.
fn plan_arith(op: ArithOp, ra: &Ty, rb: &Ty) -> Result<ArithPlan, TypeError> {
    let na = arith_operand(ra)?;
    let nb = arith_operand(rb)?;
    let base = match (na, nb) {
        (Some(a), Some(b)) => NUMERIC.join(a, b).ok_or(TypeError::Mismatch)?,
        (Some(a), None) => a,
        (None, Some(b)) => b,
        // Both operands are unbound type vars: default the arithmetic to `Int`.
        (None, None) => "Int",
    };
    let result = if op == ArithOp::Div {
        field_of(base)
    } else {
        base
    };
    Ok(ArithPlan {
        base,
        result,
        eff_a: na.unwrap_or(base),
        eff_b: nb.unwrap_or(base),
        unify_a: na.is_none(),
        unify_b: nb.is_none(),
    })
}

/// Immutable typing context mapping variable names to types for variable lookup.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TyCtx(pub BTreeMap<String, Ty>);

impl TyCtx {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn extend(&self, var: impl Into<String>, ty: Ty) -> Self {
        let mut map = self.0.clone();
        map.insert(var.into(), ty);
        Self(map)
    }

    pub fn get(&self, var: &str) -> Option<&Ty> {
        self.0.get(var)
    }
}

/// Errors during type checking in slice 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    Unbound(String),
    Mismatch,
    InfiniteType,
    Unsupported,
    NoField(String),
    /// The built derivation tree is not well-formed (a `Seq` middle does not match),
    /// so it cannot be honestly labelled `Audited`. Arises when a leaf endpoint config
    /// captured at sub-inference time is later refined by unification (ADR-0007 §7 limitation).
    IllFormedDerivation,
    /// Non-exhaustive match expression with list of uncovered variant names.
    NonExhaustive(Vec<String>),
}

/// Binds pattern variables against a scrutinee type under substitution.
pub fn bind_pattern(
    pat: &Pattern,
    scrutinee_ty: &Ty,
    subst: &BTreeMap<u32, Ty>,
) -> Result<Vec<(String, Ty)>, TypeError> {
    match pat {
        Pattern::Wildcard => Ok(vec![]),
        Pattern::Var(x) => Ok(vec![(x.clone(), scrutinee_ty.clone())]),
        Pattern::Ctor(vname, subpats) => {
            let resolved = resolve(scrutinee_ty, subst);
            if let Ty::Sum(_sum_name, variants) = resolved {
                let (_, declared_fields) = variants
                    .iter()
                    .find(|(name, _)| name == vname)
                    .ok_or(TypeError::Mismatch)?;
                if subpats.len() != declared_fields.len() {
                    return Err(TypeError::Mismatch);
                }
                let mut bindings = Vec::new();
                for (subp, f_ty) in subpats.iter().zip(declared_fields.iter()) {
                    match subp {
                        Pattern::Wildcard => {}
                        Pattern::Var(x) => {
                            bindings.push((x.clone(), zonk(f_ty, subst)));
                        }
                        Pattern::Ctor(_, _) => {
                            return Err(TypeError::Unsupported);
                        }
                    }
                }
                Ok(bindings)
            } else {
                Err(TypeError::Mismatch)
            }
        }
    }
}

/// Checks structural pattern-matrix coverage of match arms against a scrutinee type.
pub fn check_coverage(
    scrutinee_ty: &Ty,
    arms: &[(Pattern, Expr)],
    subst: &BTreeMap<u32, Ty>,
) -> Result<(), TypeError> {
    let resolved = resolve(scrutinee_ty, subst);
    if let Ty::Sum(_sum_name, variants) = resolved {
        let mut uncovered: Vec<String> = variants.iter().map(|(vname, _)| vname.clone()).collect();
        for (pat, _) in arms {
            match pat {
                Pattern::Wildcard | Pattern::Var(_) => {
                    uncovered.clear();
                }
                Pattern::Ctor(vname, _) => {
                    if !variants.iter().any(|(n, _)| n == vname) {
                        return Err(TypeError::Mismatch);
                    }
                    if let Some(pos) = uncovered.iter().position(|n| n == vname) {
                        uncovered.remove(pos);
                    }
                }
            }
        }
        if !uncovered.is_empty() {
            Err(TypeError::NonExhaustive(uncovered))
        } else {
            Ok(())
        }
    } else {
        Err(TypeError::Mismatch)
    }
}

/// Immutable inference state containing substitution map and fresh variable counter.
///
/// NOTE (ADR-0005): Unification threads plain `BTreeMap<u32, Ty>` without hashing or
/// `ConfigId` materialization inside the inference loop.
/// A persistent HAMT (e.g. `im::HashMap`) is the future performance path for cheap structural
/// sharing; `BTreeMap` clone-extend is used here for slice-2 correctness without external dependencies.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Infer {
    pub subst: BTreeMap<u32, Ty>,
    pub next_var: u32,
}

impl Infer {
    pub fn new() -> Self {
        Self {
            subst: BTreeMap::new(),
            next_var: 0,
        }
    }

    /// Returns a fresh type variable index and an updated `Infer` state with `next_var` incremented.
    pub fn fresh_var(&self) -> (u32, Self) {
        let v = self.next_var;
        let next_st = Self {
            subst: self.subst.clone(),
            next_var: v + 1,
        };
        (v, next_st)
    }
}

/// Resolves top-level type variable indirections in `ty` under `subst`.
pub fn resolve<'a>(ty: &'a Ty, subst: &'a BTreeMap<u32, Ty>) -> &'a Ty {
    let mut curr = ty;
    while let Ty::Var(v) = curr {
        if let Some(next) = subst.get(v) {
            curr = next;
        } else {
            break;
        }
    }
    curr
}

/// Fully resolves (zonks) a type, recursively replacing all bound variables.
pub fn zonk(ty: &Ty, subst: &BTreeMap<u32, Ty>) -> Ty {
    match resolve(ty, subst) {
        Ty::Con(name) => Ty::Con(name),
        Ty::Var(v) => Ty::Var(*v),
        Ty::Fn(a, b) => Ty::Fn(Box::new(zonk(a, subst)), Box::new(zonk(b, subst))),
        Ty::Record(fields) => {
            let mut sorted: Vec<(String, Ty)> = fields
                .iter()
                .map(|(name, t)| (name.clone(), zonk(t, subst)))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            sorted.dedup_by(|a, b| a.0 == b.0);
            Ty::Record(sorted)
        }
        Ty::Sum(sum_name, variants) => {
            let zonked_vars = variants
                .iter()
                .map(|(vname, fields)| {
                    let z_fields = fields.iter().map(|f| zonk(f, subst)).collect();
                    (vname.clone(), z_fields)
                })
                .collect();
            Ty::Sum(sum_name.clone(), zonked_vars)
        }
    }
}

/// Occurs-check: checks if type variable `v` occurs free in `ty` under `subst`.
pub fn occurs(v: u32, ty: &Ty, subst: &BTreeMap<u32, Ty>) -> bool {
    match resolve(ty, subst) {
        Ty::Var(v2) => v == *v2,
        Ty::Con(_) => false,
        Ty::Fn(a, b) => occurs(v, a, subst) || occurs(v, b, subst),
        Ty::Record(fields) => fields.iter().any(|(_, t)| occurs(v, t, subst)),
        Ty::Sum(_, variants) => variants
            .iter()
            .any(|(_, fields)| fields.iter().any(|f| occurs(v, f, subst))),
    }
}

/// Declarative unification as SOC context narrowing over explicit immutable substitution `subst`.
///
/// Returns the updated substitution and recorded `g_unify()` generator steps.
/// Does NOT mutate state or perform hashing inside the unification loop.
pub fn unify(
    t1: &Ty,
    t2: &Ty,
    subst: &BTreeMap<u32, Ty>,
) -> Result<(BTreeMap<u32, Ty>, Vec<GeneratorId>), TypeError> {
    let r1 = resolve(t1, subst);
    let r2 = resolve(t2, subst);

    match (r1, r2) {
        (Ty::Var(v1), Ty::Var(v2)) if v1 == v2 => Ok((subst.clone(), vec![g_unify()])),
        (Ty::Var(v1), t) => {
            if occurs(*v1, t, subst) {
                Err(TypeError::InfiniteType)
            } else {
                let mut next_subst = subst.clone();
                next_subst.insert(*v1, t.clone());
                Ok((next_subst, vec![g_unify()]))
            }
        }
        (t, Ty::Var(v2)) => {
            if occurs(*v2, t, subst) {
                Err(TypeError::InfiniteType)
            } else {
                let mut next_subst = subst.clone();
                next_subst.insert(*v2, t.clone());
                Ok((next_subst, vec![g_unify()]))
            }
        }
        (Ty::Con(a), Ty::Con(b)) => {
            if a == b {
                Ok((subst.clone(), vec![g_unify()]))
            } else {
                Err(TypeError::Mismatch)
            }
        }
        (Ty::Fn(a1, b1), Ty::Fn(a2, b2)) => {
            let (s1, mut steps1) = unify(a1, a2, subst)?;
            let (s2, steps2) = unify(b1, b2, &s1)?;
            let mut steps = vec![g_unify()];
            steps.append(&mut steps1);
            steps.extend(steps2);
            Ok((s2, steps))
        }
        (Ty::Record(f1), Ty::Record(f2)) => {
            let mut s1_fields = f1.clone();
            s1_fields.sort_by(|a, b| a.0.cmp(&b.0));
            s1_fields.dedup_by(|a, b| a.0 == b.0);

            let mut s2_fields = f2.clone();
            s2_fields.sort_by(|a, b| a.0.cmp(&b.0));
            s2_fields.dedup_by(|a, b| a.0 == b.0);

            if s1_fields.len() != s2_fields.len() {
                return Err(TypeError::Mismatch);
            }
            for (a, b) in s1_fields.iter().zip(s2_fields.iter()) {
                if a.0 != b.0 {
                    return Err(TypeError::Mismatch);
                }
            }
            let mut curr_subst = subst.clone();
            let mut all_steps = vec![g_unify()];
            for (a, b) in s1_fields.iter().zip(s2_fields.iter()) {
                let (next_subst, steps) = unify(&a.1, &b.1, &curr_subst)?;
                curr_subst = next_subst;
                all_steps.extend(steps);
            }
            Ok((curr_subst, all_steps))
        }
        (Ty::Sum(n1, vs1), Ty::Sum(n2, vs2)) => {
            if n1 != n2 || vs1.len() != vs2.len() {
                return Err(TypeError::Mismatch);
            }
            for ((v1_name, v1_fields), (v2_name, v2_fields)) in vs1.iter().zip(vs2.iter()) {
                if v1_name != v2_name || v1_fields.len() != v2_fields.len() {
                    return Err(TypeError::Mismatch);
                }
            }
            let mut curr_subst = subst.clone();
            let mut all_steps = vec![g_unify()];
            for ((_, v1_fields), (_, v2_fields)) in vs1.iter().zip(vs2.iter()) {
                for (f1, f2) in v1_fields.iter().zip(v2_fields.iter()) {
                    let (next_subst, steps) = unify(f1, f2, &curr_subst)?;
                    curr_subst = next_subst;
                    all_steps.extend(steps);
                }
            }
            Ok((curr_subst, all_steps))
        }
        _ => Err(TypeError::Mismatch),
    }
}

/// Core inference procedure threading immutable `Infer` state.
pub fn infer(
    expr: &Expr,
    ctx: &TyCtx,
    st: Infer,
) -> Result<(Ty, Vec<GeneratorId>, Infer), TypeError> {
    match expr {
        Expr::Lit(_) => Ok((Ty::Con("Int"), vec![g_lit()], st)),
        Expr::StrLit(_) => Ok((Ty::Con("Str"), vec![g_str_lit()], st)),
        Expr::FloatLit(_) => Ok((Ty::Con("Float"), vec![g_float_lit()], st)),
        Expr::Arith(op, a, b) => {
            let (ta, da, s1) = infer(a, ctx, st)?;
            let (tb, db, s2) = infer(b, ctx, s1)?;
            let ra = resolve(&ta, &s2.subst).clone();
            let rb = resolve(&tb, &s2.subst).clone();
            let plan = plan_arith(*op, &ra, &rb)?;

            // Bind any type-variable operand to the meet point `base`.
            let mut subst = s2.subst.clone();
            if plan.unify_a {
                let (s, _) = unify(&ra, &Ty::Con(plan.base), &subst)?;
                subst = s;
            }
            if plan.unify_b {
                let (s, _) = unify(&rb, &Ty::Con(plan.base), &subst)?;
                subst = s;
            }

            let mut deriv = vec![g_arith()];
            deriv.extend(da);
            deriv.extend(db);
            for (c, p) in NUMERIC.edge_path(plan.eff_a, plan.result) {
                deriv.push(NUMERIC.promote_generator(c, p));
            }
            for (c, p) in NUMERIC.edge_path(plan.eff_b, plan.result) {
                deriv.push(NUMERIC.promote_generator(c, p));
            }

            Ok((
                Ty::Con(plan.result),
                deriv,
                Infer {
                    subst,
                    next_var: s2.next_var,
                },
            ))
        }
        Expr::Var(name) => {
            let ty = ctx
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::Unbound(name.clone()))?;
            Ok((ty, vec![g_var()], st))
        }
        Expr::Lam(p, body) => {
            let (alpha, st_alpha) = st.fresh_var();
            let ctx_ext = ctx.extend(p.clone(), Ty::Var(alpha));
            let (tb, dv, st_prime) = infer(body, &ctx_ext, st_alpha)?;
            let param_ty = resolve(&Ty::Var(alpha), &st_prime.subst).clone();
            let fn_ty = Ty::Fn(Box::new(param_ty), Box::new(tb));
            let mut deriv = vec![g_lam()];
            deriv.extend(dv);
            Ok((fn_ty, deriv, st_prime))
        }
        Expr::App(f, x) => {
            let (tf, df, s1) = infer(f, ctx, st)?;
            let (tx, dx, s2) = infer(x, ctx, s1)?;
            let (beta, s_beta) = s2.fresh_var();

            let target_fn = Ty::Fn(
                Box::new(resolve(&tx, &s_beta.subst).clone()),
                Box::new(Ty::Var(beta)),
            );

            let (s3_subst, unify_steps) =
                unify(resolve(&tf, &s_beta.subst), &target_fn, &s_beta.subst)?;

            let res_ty = resolve(&Ty::Var(beta), &s3_subst).clone();

            let mut deriv = vec![g_app()];
            deriv.extend(df);
            deriv.extend(dx);
            deriv.extend(unify_steps);

            let st_final = Infer {
                subst: s3_subst,
                next_var: s_beta.next_var,
            };

            Ok((res_ty, deriv, st_final))
        }
        Expr::Record(fields) => {
            let mut sorted_fields = fields.clone();
            sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
            sorted_fields.dedup_by(|a, b| a.0 == b.0);

            let mut res_fields = Vec::new();
            let mut deriv = vec![if sorted_fields.is_empty() {
                g_record_empty()
            } else {
                g_record()
            }];
            let mut curr_st = st;
            for (name, val) in sorted_fields {
                let (t_i, d_i, next_st) = infer(&val, ctx, curr_st)?;
                res_fields.push((name, t_i));
                deriv.extend(d_i);
                curr_st = next_st;
            }
            Ok((Ty::Record(res_fields), deriv, curr_st))
        }
        Expr::Field(base, fname) => {
            let (t_base, d_base, st1) = infer(base, ctx, st)?;
            let zonked_base = zonk(&t_base, &st1.subst);
            if let Ty::Record(fields) = zonked_base {
                if let Some((_, t_f)) = fields.iter().find(|(n, _)| n == fname) {
                    let mut deriv = vec![g_field()];
                    deriv.extend(d_base);
                    Ok((t_f.clone(), deriv, st1))
                } else {
                    Err(TypeError::NoField(fname.clone()))
                }
            } else {
                Err(TypeError::Mismatch)
            }
        }
        Expr::Ctor(sum_ty, variant, args) => {
            let resolved = resolve(sum_ty, &st.subst);
            if let Ty::Sum(_sum_name, variants) = resolved {
                let (_, declared_fields) = variants
                    .iter()
                    .find(|(vname, _)| vname == variant)
                    .ok_or(TypeError::Mismatch)?;
                if args.len() != declared_fields.len() {
                    return Err(TypeError::Mismatch);
                }
                let declared_fields = declared_fields.clone();
                let mut deriv = vec![if args.is_empty() {
                    g_ctor_nullary()
                } else {
                    g_ctor()
                }];
                let mut curr_st = st;
                for (arg_expr, declared_field_ty) in args.iter().zip(declared_fields.iter()) {
                    let (t_i, d_i, next_st) = infer(arg_expr, ctx, curr_st)?;
                    deriv.extend(d_i);
                    let (next_subst, _) = unify(&t_i, declared_field_ty, &next_st.subst)?;
                    curr_st = Infer {
                        subst: next_subst,
                        next_var: next_st.next_var,
                    };
                }
                Ok((sum_ty.clone(), deriv, curr_st))
            } else {
                Err(TypeError::Mismatch)
            }
        }
        Expr::Match(scrutinee, arms) => {
            let (t_s, d_s, s1) = infer(scrutinee, ctx, st)?;
            check_coverage(&t_s, arms, &s1.subst)?;
            let match_generator = if arms
                .iter()
                .any(|(pattern, _)| matches!(pattern, Pattern::Wildcard | Pattern::Var(_)))
            {
                g_match_catchall()
            } else {
                g_match()
            };
            let mut deriv = vec![match_generator];
            deriv.extend(d_s);
            let mut curr_st = s1;
            let mut res_ty: Option<Ty> = None;
            for (pat, body) in arms {
                let bindings = bind_pattern(pat, &t_s, &curr_st.subst)?;
                let mut arm_ctx = ctx.clone();
                for (x, t) in bindings {
                    arm_ctx = arm_ctx.extend(x, t);
                }
                let (t_i, d_i, next_st) = infer(body, &arm_ctx, curr_st)?;
                deriv.extend(d_i);
                if let Some(ref r_ty) = res_ty {
                    let (next_subst, _) = unify(r_ty, &t_i, &next_st.subst)?;
                    curr_st = Infer {
                        subst: next_subst,
                        next_var: next_st.next_var,
                    };
                } else {
                    res_ty = Some(t_i);
                    curr_st = next_st;
                }
            }
            let result_ty = res_ty.ok_or(TypeError::Mismatch)?;
            Ok((result_ty, deriv, curr_st))
        }
    }
}

/// Type-checks `expr` in environment `ctx` under context `context`.
///
/// Produces a native SOC `Judgement` with `Outcome::Derived` asserting
/// `HasType(expr, ty)` = `Realizes(derivation_witness, expr_config, ty_config)`.
pub fn type_check(expr: &Expr, ctx: &TyCtx, context: ContextId) -> Result<Judgement, TypeError> {
    let st = Infer::new();
    let (ty, derivation, final_st) = infer(expr, ctx, st)?;

    // Fully resolve (zonk) all bound type variables in the resulting type.
    let final_ty = zonk(&ty, &final_st.subst);

    let derivation_witness: WitnessId = compose_chain(&derivation)
        .expect("derivation chain must contain at least one generator step");

    // CRITICAL DISCIPLINE (ADR-0005): Materialize canonical digests / ConfigIds ONLY here
    // at the commit boundary — NOT per unify/infer step.
    let src = expr.config_id();
    let dst = final_ty.config_id();

    let prop = Realizes::new(derivation_witness, src, dst).proposition_id();

    // INTERMEDIATE-CONFIG CHAIN DISCIPLINE:
    // For multi-step derivations in slice 2, we use endpoint padding (`[src, dst, ..., dst]`)
    // of length `derivation.len() + 1` to fulfill Decomposition's structural invariant
    // (`configs.len() == generators.len() + 1`) without creating intermediate ConfigIds during inference.
    let mut configs = vec![src];
    configs.resize(derivation.len() + 1, dst);

    let decomp = Decomposition::recorded(derivation, configs)
        .expect("multi-step derivation decomposition is valid");
    let evidence = Evidence::SettlementReplay {
        body: decomp.id().digest(),
    }
    .id();

    Ok(Judgement::new(context, prop, Outcome::Derived, evidence))
}

/// Upgrades a native `type_check` derivation to an `Audited` `Judgement` and
/// verified `Decomposition` (ADR-0005 Stage 2 depth slice).
///
/// Produces a replay-verified decomposition and an `Outcome::Audited` judgement,
/// suitable for proof kernel elaboration via `brix_elaborate::elaborate_decomposition`.
pub fn audited_type_check(
    expr: &Expr,
    ctx: &TyCtx,
    context: ContextId,
) -> Result<(Judgement, Decomposition), TypeError> {
    let st = Infer::new();
    let (ty, derivation, final_st) = infer(expr, ctx, st)?;
    let final_ty = zonk(&ty, &final_st.subst);

    let derivation_witness: WitnessId = compose_chain(&derivation)
        .expect("derivation chain must contain at least one generator step");

    let src = expr.config_id();
    let dst = final_ty.config_id();

    let prop = Realizes::new(derivation_witness, src, dst).proposition_id();

    let mut configs = vec![src];
    configs.resize(derivation.len() + 1, dst);

    let verified_decomp =
        Decomposition::replay_verified(derivation, configs).expect("replay verified decomposition");
    let evidence = Evidence::SettlementReplay {
        body: verified_decomp.id().digest(),
    }
    .id();

    let audited = Judgement::new(context, prop, Outcome::Audited, evidence);
    Ok((audited, verified_decomp))
}

/// Deferred-materialization atom for tree leaf endpoints (ADR-0008).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CfgAtom {
    Expr(Expr),
    Type(Ty),
}

/// Deferred-materialization tree object (ADR-0008).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TyObj {
    Atom(CfgAtom),
    Prod(Box<TyObj>, Box<TyObj>),
}

/// Deferred-materialization derivation tree (ADR-0008).
#[derive(Clone, PartialEq, Eq, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum TyTree {
    Leaf {
        generator: GeneratorId,
        src: TyObj,
        dst: TyObj,
    },
    Seq {
        left: Box<TyTree>,
        right: Box<TyTree>,
    },
    Tensor {
        left: Box<TyTree>,
        right: Box<TyTree>,
    },
}

fn materialize_obj(obj: &TyObj, subst: &BTreeMap<u32, Ty>) -> TreeObj {
    match obj {
        TyObj::Atom(CfgAtom::Expr(e)) => TreeObj::Atom(e.config_id()),
        TyObj::Atom(CfgAtom::Type(t)) => TreeObj::Atom(zonk(t, subst).config_id()),
        TyObj::Prod(a, b) => TreeObj::Prod(
            Box::new(materialize_obj(a, subst)),
            Box::new(materialize_obj(b, subst)),
        ),
    }
}

/// Materializes a `TyTree` into a `RealizesTree` by zonking all type endpoints against `subst` (ADR-0008).
pub fn materialize(tree: &TyTree, subst: &BTreeMap<u32, Ty>) -> RealizesTree {
    match tree {
        TyTree::Leaf {
            generator,
            src,
            dst,
        } => RealizesTree::Leaf {
            generator: *generator,
            src: materialize_obj(src, subst),
            dst: materialize_obj(dst, subst),
        },
        TyTree::Seq { left, right } => RealizesTree::Seq {
            left: Box::new(materialize(left, subst)),
            right: Box::new(materialize(right, subst)),
        },
        TyTree::Tensor { left, right } => RealizesTree::Tensor {
            left: Box::new(materialize(left, subst)),
            right: Box::new(materialize(right, subst)),
        },
    }
}

/// Infer tree-structured realization derivation with deferred config materialization (ADR-0008).
pub fn infer_tree(expr: &Expr, ctx: &TyCtx, st: Infer) -> Result<(Ty, TyTree, Infer), TypeError> {
    match expr {
        Expr::Lit(n) => Ok((
            Ty::Con("Int"),
            TyTree::Leaf {
                generator: g_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::Lit(*n))),
                dst: TyObj::Atom(CfgAtom::Type(Ty::Con("Int"))),
            },
            st,
        )),
        Expr::StrLit(s) => Ok((
            Ty::Con("Str"),
            TyTree::Leaf {
                generator: g_str_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::StrLit(s.clone()))),
                dst: TyObj::Atom(CfgAtom::Type(Ty::Con("Str"))),
            },
            st,
        )),
        Expr::FloatLit(s) => Ok((
            Ty::Con("Float"),
            TyTree::Leaf {
                generator: g_float_lit(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::FloatLit(s.clone()))),
                dst: TyObj::Atom(CfgAtom::Type(Ty::Con("Float"))),
            },
            st,
        )),
        Expr::Arith(op, a, b) => {
            let (ta, da, s1) = infer_tree(a, ctx, st)?;
            let (tb, db, s2) = infer_tree(b, ctx, s1)?;
            let ra = resolve(&ta, &s2.subst).clone();
            let rb = resolve(&tb, &s2.subst).clone();
            let plan = plan_arith(*op, &ra, &rb)?;

            let mut subst = s2.subst.clone();
            if plan.unify_a {
                let (s, _) = unify(&ra, &Ty::Con(plan.base), &subst)?;
                subst = s;
            }
            if plan.unify_b {
                let (s, _) = unify(&rb, &Ty::Con(plan.base), &subst)?;
                subst = s;
            }

            let result_ty = Ty::Con(plan.result);

            // Splice each operand's witnessed embedding path up to the result
            // type (empty when the operand is already there).
            let da = NUMERIC.coerce(da, plan.eff_a, plan.result);
            let db = NUMERIC.coerce(db, plan.eff_b, plan.result);

            let split = TyTree::Leaf {
                generator: g_arith_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Expr((**a).clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Expr((**b).clone()))),
                ),
            };
            let tensor = TyTree::Tensor {
                left: Box::new(da),
                right: Box::new(db),
            };
            let arith_leaf = TyTree::Leaf {
                generator: g_arith(),
                src: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Type(result_ty.clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Type(result_ty.clone()))),
                ),
                dst: TyObj::Atom(CfgAtom::Type(result_ty.clone())),
            };
            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(tensor),
                    right: Box::new(arith_leaf),
                }),
            };

            Ok((
                result_ty,
                tree,
                Infer {
                    subst,
                    next_var: s2.next_var,
                },
            ))
        }
        Expr::Var(name) => {
            let t = ctx
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::Unbound(name.clone()))?;
            Ok((
                t.clone(),
                TyTree::Leaf {
                    generator: g_var(),
                    src: TyObj::Atom(CfgAtom::Expr(Expr::Var(name.clone()))),
                    dst: TyObj::Atom(CfgAtom::Type(t.clone())),
                },
                st,
            ))
        }
        Expr::Lam(p, body) => {
            let (alpha, st_alpha) = st.fresh_var();
            let ctx_ext = ctx.extend(p.clone(), Ty::Var(alpha));
            let (tb, d_body, st_prime) = infer_tree(body, &ctx_ext, st_alpha)?;
            let param_ty = resolve(&Ty::Var(alpha), &st_prime.subst).clone();
            let fn_ty = Ty::Fn(Box::new(param_ty.clone()), Box::new(tb.clone()));

            let intro = TyTree::Leaf {
                generator: g_lam_intro(),
                src: TyObj::Atom(CfgAtom::Expr(Expr::Lam(p.clone(), body.clone()))),
                dst: TyObj::Atom(CfgAtom::Expr((**body).clone())),
            };
            let close = TyTree::Leaf {
                generator: g_lam_close(),
                src: TyObj::Atom(CfgAtom::Type(tb.clone())),
                dst: TyObj::Atom(CfgAtom::Type(fn_ty.clone())),
            };
            let tree = TyTree::Seq {
                left: Box::new(intro),
                right: Box::new(TyTree::Seq {
                    left: Box::new(d_body),
                    right: Box::new(close),
                }),
            };
            Ok((fn_ty, tree, st_prime))
        }
        Expr::App(f, x) => {
            let (tf, df, s1) = infer_tree(f, ctx, st)?;
            let (tx, dx, s2) = infer_tree(x, ctx, s1)?;
            let (beta, s_beta) = s2.fresh_var();

            let target = Ty::Fn(
                Box::new(resolve(&tx, &s_beta.subst).clone()),
                Box::new(Ty::Var(beta)),
            );

            let (s3, _) = unify(resolve(&tf, &s_beta.subst), &target, &s_beta.subst)?;

            let a = zonk(&tx, &s3);
            let b = zonk(&Ty::Var(beta), &s3);
            let fn_ty = Ty::Fn(Box::new(a.clone()), Box::new(b.clone()));

            let split = TyTree::Leaf {
                generator: g_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Expr((**f).clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Expr((**x).clone()))),
                ),
            };

            let tensor = TyTree::Tensor {
                left: Box::new(df),
                right: Box::new(dx),
            };

            let app = TyTree::Leaf {
                generator: g_app2(),
                src: TyObj::Prod(
                    Box::new(TyObj::Atom(CfgAtom::Type(fn_ty.clone()))),
                    Box::new(TyObj::Atom(CfgAtom::Type(a.clone()))),
                ),
                dst: TyObj::Atom(CfgAtom::Type(b.clone())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(tensor),
                    right: Box::new(app),
                }),
            };

            Ok((
                b,
                tree,
                Infer {
                    subst: s3,
                    next_var: s_beta.next_var,
                },
            ))
        }
        Expr::Record(fields) => {
            let mut sorted_fields = fields.clone();
            sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
            sorted_fields.dedup_by(|a, b| a.0 == b.0);

            if sorted_fields.is_empty() {
                let rec_ty = Ty::Record(vec![]);
                let split = TyTree::Leaf {
                    generator: g_record_split(),
                    src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                    dst: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                };
                let record_leaf = TyTree::Leaf {
                    generator: g_record_empty(),
                    src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                    dst: TyObj::Atom(CfgAtom::Type(rec_ty.clone())),
                };
                let tree = TyTree::Seq {
                    left: Box::new(split),
                    right: Box::new(record_leaf),
                };
                return Ok((rec_ty, tree, st));
            }

            let mut sorted_types = Vec::new();
            let mut expr_atoms = Vec::new();
            let mut type_atoms = Vec::new();
            let mut d_trees = Vec::new();
            let mut curr_st = st;

            for (name, val_expr) in sorted_fields {
                let (t_i, d_i, next_st) = infer_tree(&val_expr, ctx, curr_st)?;
                sorted_types.push((name, t_i.clone()));
                expr_atoms.push(TyObj::Atom(CfgAtom::Expr(val_expr)));
                type_atoms.push(TyObj::Atom(CfgAtom::Type(t_i)));
                d_trees.push(d_i);
                curr_st = next_st;
            }

            let result_ty = Ty::Record(sorted_types);

            let split = TyTree::Leaf {
                generator: g_record_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: right_nest_prod(expr_atoms),
            };

            let fields_tensor = right_nest_tensor(d_trees);

            let record_leaf = TyTree::Leaf {
                generator: g_record(),
                src: right_nest_prod(type_atoms),
                dst: TyObj::Atom(CfgAtom::Type(result_ty.clone())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(fields_tensor),
                    right: Box::new(record_leaf),
                }),
            };

            Ok((result_ty, tree, curr_st))
        }
        Expr::Field(base, fname) => {
            let (t_base, d_base, st1) = infer_tree(base, ctx, st)?;
            let zonked_base = zonk(&t_base, &st1.subst);
            if let Ty::Record(fields) = &zonked_base {
                if let Some((_, t_f)) = fields.iter().find(|(n, _)| n == fname) {
                    let split = TyTree::Leaf {
                        generator: g_field_split(),
                        src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                        dst: TyObj::Atom(CfgAtom::Expr((**base).clone())),
                    };
                    let field_leaf = TyTree::Leaf {
                        generator: g_field(),
                        src: TyObj::Atom(CfgAtom::Type(zonked_base.clone())),
                        dst: TyObj::Atom(CfgAtom::Type(t_f.clone())),
                    };
                    let tree = TyTree::Seq {
                        left: Box::new(split),
                        right: Box::new(TyTree::Seq {
                            left: Box::new(d_base),
                            right: Box::new(field_leaf),
                        }),
                    };
                    Ok((t_f.clone(), tree, st1))
                } else {
                    Err(TypeError::NoField(fname.clone()))
                }
            } else {
                Err(TypeError::Mismatch)
            }
        }
        Expr::Ctor(sum_ty, variant, args) => {
            let resolved = resolve(sum_ty, &st.subst);
            if let Ty::Sum(_sum_name, variants) = resolved {
                let (_, declared_fields) = variants
                    .iter()
                    .find(|(vname, _)| vname == variant)
                    .ok_or(TypeError::Mismatch)?;
                if args.len() != declared_fields.len() {
                    return Err(TypeError::Mismatch);
                }
                let declared_fields = declared_fields.clone();
                if args.is_empty() {
                    let split = TyTree::Leaf {
                        generator: g_ctor_split(),
                        src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                        dst: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                    };
                    let ctor_leaf = TyTree::Leaf {
                        generator: g_ctor_nullary(),
                        src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                        dst: TyObj::Atom(CfgAtom::Type(sum_ty.clone())),
                    };
                    let tree = TyTree::Seq {
                        left: Box::new(split),
                        right: Box::new(ctor_leaf),
                    };
                    return Ok((sum_ty.clone(), tree, st));
                }

                let mut expr_atoms = Vec::new();
                let mut type_atoms = Vec::new();
                let mut d_trees = Vec::new();
                let mut curr_st = st;

                for (arg_expr, declared_field_ty) in args.iter().zip(declared_fields.iter()) {
                    let (t_i, d_i, next_st) = infer_tree(arg_expr, ctx, curr_st)?;
                    let (next_subst, _) = unify(&t_i, declared_field_ty, &next_st.subst)?;
                    expr_atoms.push(TyObj::Atom(CfgAtom::Expr(arg_expr.clone())));
                    let zonked_field = zonk(declared_field_ty, &next_subst);
                    type_atoms.push(TyObj::Atom(CfgAtom::Type(zonked_field)));
                    d_trees.push(d_i);
                    curr_st = Infer {
                        subst: next_subst,
                        next_var: next_st.next_var,
                    };
                }

                let split = TyTree::Leaf {
                    generator: g_ctor_split(),
                    src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                    dst: right_nest_prod(expr_atoms),
                };

                let args_tensor = right_nest_tensor(d_trees);

                let ctor_leaf = TyTree::Leaf {
                    generator: g_ctor(),
                    src: right_nest_prod(type_atoms),
                    dst: TyObj::Atom(CfgAtom::Type(sum_ty.clone())),
                };

                let tree = TyTree::Seq {
                    left: Box::new(split),
                    right: Box::new(TyTree::Seq {
                        left: Box::new(args_tensor),
                        right: Box::new(ctor_leaf),
                    }),
                };

                Ok((sum_ty.clone(), tree, curr_st))
            } else {
                Err(TypeError::Mismatch)
            }
        }
        Expr::Match(scrutinee, arms) => {
            let (t_s, d_s, s1) = infer_tree(scrutinee, ctx, st)?;
            check_coverage(&t_s, arms, &s1.subst)?;
            let mut curr_st = s1;
            let mut parts_exprs = vec![TyObj::Atom(CfgAtom::Expr((**scrutinee).clone()))];
            let mut parts_derivs = vec![d_s];
            let mut res_ty: Option<Ty> = None;

            for (pat, body) in arms {
                let bindings = bind_pattern(pat, &t_s, &curr_st.subst)?;
                let mut arm_ctx = ctx.clone();
                for (x, t) in bindings {
                    arm_ctx = arm_ctx.extend(x, t);
                }
                let (t_i, d_i, next_st) = infer_tree(body, &arm_ctx, curr_st)?;
                parts_exprs.push(TyObj::Atom(CfgAtom::Expr(body.clone())));
                parts_derivs.push(d_i);
                if let Some(ref r_ty) = res_ty {
                    let (next_subst, _) = unify(r_ty, &t_i, &next_st.subst)?;
                    curr_st = Infer {
                        subst: next_subst,
                        next_var: next_st.next_var,
                    };
                } else {
                    res_ty = Some(t_i);
                    curr_st = next_st;
                }
            }

            let result_ty = res_ty.ok_or(TypeError::Mismatch)?;
            let zonked_t_s = zonk(&t_s, &curr_st.subst);
            let zonked_t_res = zonk(&result_ty, &curr_st.subst);

            let mut parts_types = vec![TyObj::Atom(CfgAtom::Type(zonked_t_s))];
            for _ in arms {
                parts_types.push(TyObj::Atom(CfgAtom::Type(zonked_t_res.clone())));
            }

            let split = TyTree::Leaf {
                generator: g_match_split(),
                src: TyObj::Atom(CfgAtom::Expr(expr.clone())),
                dst: right_nest_prod(parts_exprs),
            };

            let parts_tensor = right_nest_tensor(parts_derivs);

            let match_generator = if arms
                .iter()
                .any(|(pattern, _)| matches!(pattern, Pattern::Wildcard | Pattern::Var(_)))
            {
                g_match_catchall()
            } else {
                g_match()
            };
            let match_leaf = TyTree::Leaf {
                generator: match_generator,
                src: right_nest_prod(parts_types),
                dst: TyObj::Atom(CfgAtom::Type(result_ty.clone())),
            };

            let tree = TyTree::Seq {
                left: Box::new(split),
                right: Box::new(TyTree::Seq {
                    left: Box::new(parts_tensor),
                    right: Box::new(match_leaf),
                }),
            };

            Ok((result_ty, tree, curr_st))
        }
    }
}

fn right_nest_prod(items: Vec<TyObj>) -> TyObj {
    assert!(
        !items.is_empty(),
        "right_nest_prod requires at least 1 element"
    );
    let mut iter = items.into_iter().rev();
    let last = iter.next().unwrap();
    iter.fold(last, |acc, elem| TyObj::Prod(Box::new(elem), Box::new(acc)))
}

fn right_nest_tensor(items: Vec<TyTree>) -> TyTree {
    assert!(
        !items.is_empty(),
        "right_nest_tensor requires at least 1 element"
    );
    let mut iter = items.into_iter().rev();
    let last = iter.next().unwrap();
    iter.fold(last, |acc, elem| TyTree::Tensor {
        left: Box::new(elem),
        right: Box::new(acc),
    })
}

/// Upgrades a native `infer_tree` derivation to an `Audited` `Judgement` and `RealizesTree` (ADR-0008).
pub fn audited_type_check_tree(
    expr: &Expr,
    ctx: &TyCtx,
    context: ContextId,
) -> Result<(Judgement, RealizesTree), TypeError> {
    let (ty, ty_tree, st) = infer_tree(expr, ctx, Infer::new())?;
    let tree = materialize(&ty_tree, &st.subst);
    let final_ty = zonk(&ty, &st.subst);
    if !tree.well_formed()
        || tree.src() != TreeObj::Atom(expr.config_id())
        || tree.dst() != TreeObj::Atom(final_ty.config_id())
    {
        return Err(TypeError::IllFormedDerivation);
    }
    let witness_id = tree.witness_object().witness_digest();
    let prop = Realizes::new(witness_id, expr.config_id(), final_ty.config_id()).proposition_id();
    let evidence = Evidence::SettlementReplay {
        body: brix_canon::Digest::of(brix_canon::Domain::Value, prop.digest().as_bytes()),
    }
    .id();
    let audited = Judgement::new(context, prop, Outcome::Audited, evidence);
    Ok((audited, tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_elaborate::{elaborate_decomposition, ElaborationResult};
    use brix_kernel::Budget;
    use brix_semantic::{Authority, EdgeKind, GeneratorRegistry};

    #[test]
    fn generator_name_finds_tight_and_untight_generators() {
        assert_eq!(generator_name(&g_lit()).as_deref(), Some("g_lit"));
        assert_eq!(generator_name(&g_var()).as_deref(), Some("g_var"));
        // The honest holdouts named in ADR-0010/#43: g_arith, the coercion
        // edges, and g_match_catchall.
        assert_eq!(generator_name(&g_arith()).as_deref(), Some("g_arith"));
        assert_eq!(
            generator_name(&g_match_catchall()).as_deref(),
            Some("g_match_catchall")
        );
        assert_eq!(
            generator_name(&NUMERIC.promote_generator("Int", "Float")).as_deref(),
            Some("promote(Int->Float)")
        );
    }

    #[test]
    fn generator_name_is_none_for_an_unminted_generator() {
        assert_eq!(
            generator_name(&GeneratorId::named("not-a-real-generator@1")),
            None
        );
    }

    #[test]
    fn test_lit_type_check() {
        let expr = Expr::Lit(42);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let judgement = type_check(&expr, &ctx, context).expect("type check lit");
        assert_eq!(judgement.outcome, Outcome::Derived);
        assert_eq!(judgement.context, context);

        let expected_prop = Realizes::new(
            g_lit().witness_id(),
            expr.config_id(),
            Ty::Con("Int").config_id(),
        )
        .proposition_id();

        assert_eq!(judgement.proposition, expected_prop);
    }

    #[test]
    fn test_var_type_check() {
        let expr = Expr::Var("x".to_string());
        let ctx = TyCtx::new().extend("x", Ty::Con("Bool"));
        let context = ContextId::root();

        let judgement = type_check(&expr, &ctx, context).expect("type check var");
        assert_eq!(judgement.outcome, Outcome::Derived);

        let expected_prop = Realizes::new(
            g_var().witness_id(),
            expr.config_id(),
            Ty::Con("Bool").config_id(),
        )
        .proposition_id();

        assert_eq!(judgement.proposition, expected_prop);
    }

    #[test]
    fn test_unbound_var() {
        let expr = Expr::Var("y".to_string());
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let res = type_check(&expr, &ctx, context);
        assert_eq!(res, Err(TypeError::Unbound("y".to_string())));
    }

    #[test]
    fn test_identity_applied() {
        // App(Lam("x", Var("x")), Lit(42))
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, derivation, st) = infer(&expr, &ctx, Infer::new()).expect("infer identity app");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, Ty::Con("Int"));

        // Multi-generator derivation
        assert!(derivation.contains(&g_app()));
        assert!(derivation.contains(&g_lam()));
        assert!(derivation.contains(&g_var()));
        assert!(derivation.contains(&g_lit()));
        assert!(derivation.contains(&g_unify()));

        let witness = compose_chain(&derivation).expect("witness composition");

        let judgement = type_check(&expr, &ctx, context).expect("type check identity app");
        assert_eq!(judgement.outcome, Outcome::Derived);

        let expected_prop =
            Realizes::new(witness, expr.config_id(), Ty::Con("Int").config_id()).proposition_id();
        assert_eq!(judgement.proposition, expected_prop);
    }

    #[test]
    fn test_const_function() {
        // Lam("x", Lit(7))
        let expr = Expr::Lam("x".to_string(), Box::new(Expr::Lit(7)));
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let judgement = type_check(&expr, &ctx, context).expect("type check const fn");
        assert_eq!(judgement.outcome, Outcome::Derived);

        let (ty, _, st) = infer(&expr, &ctx, Infer::new()).expect("infer const fn");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(
            final_ty,
            Ty::Fn(Box::new(Ty::Var(0)), Box::new(Ty::Con("Int")))
        );
    }

    #[test]
    fn test_mismatch_non_function_application() {
        // App(Lit(1), Lit(2)) -> applying non-function Int
        let expr = Expr::App(Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let res = type_check(&expr, &ctx, context);
        assert_eq!(res, Err(TypeError::Mismatch));
    }

    #[test]
    fn test_occurs_check_via_app() {
        // ctx: f : Var(0)
        // expr: App(Var("f"), Var("f")) -> unifying Var(0) with Fn(Var(0), Var(beta)) triggers InfiniteType
        let expr = Expr::App(
            Box::new(Expr::Var("f".to_string())),
            Box::new(Expr::Var("f".to_string())),
        );
        let ctx = TyCtx::new().extend("f", Ty::Var(0));
        let context = ContextId::root();

        let res = type_check(&expr, &ctx, context);
        assert_eq!(res, Err(TypeError::InfiniteType));
    }

    #[test]
    fn test_determinism() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let j1 = type_check(&expr, &ctx, context).unwrap();
        let j2 = type_check(&expr, &ctx, context).unwrap();

        assert_eq!(j1, j2);
        assert_eq!(j1.id(), j2.id());
    }

    #[test]
    fn test_decomposition_round_trip() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();

        let (ty, derivation, st) = infer(&expr, &ctx, Infer::new()).unwrap();
        let final_ty = zonk(&ty, &st.subst);

        let src = expr.config_id();
        let dst = final_ty.config_id();
        let mut configs = vec![src];
        configs.resize(derivation.len() + 1, dst);

        let decomp = Decomposition::recorded(derivation.clone(), configs);
        assert!(decomp.is_ok());

        let witness = compose_chain(&derivation).unwrap();
        let j = type_check(&expr, &ctx, ContextId::root()).unwrap();

        let expected_prop = Realizes::new(witness, src, dst).proposition_id();
        assert_eq!(j.proposition, expected_prop);
    }

    #[test]
    fn test_literal_elaboration_to_proven() {
        let expr = Expr::Lit(42);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (audited_judgement, verified_decomp) =
            audited_type_check(&expr, &ctx, context).expect("audited type check lit");

        assert_eq!(audited_judgement.outcome, Outcome::Audited);

        let budget = Budget::new(1000, 1000);
        let res = elaborate_decomposition(&audited_judgement, &verified_decomp, budget);

        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
                assert_eq!(judgement.context, context);

                // Edge assertion: ElaborationBoundary pointing to audited_judgement's id
                assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, audited_judgement.id().digest());

                // Evidence assertion: KernelCertificate
                let expected_verifier = brix_kernel::native_verifier();
                let g1 = g_lit();
                let src = expr.config_id();
                let dst = Ty::Con("Int").config_id();

                let h1 = brix_kernel::Prop::Realizes(
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(g1.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(src.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(dst.digest())),
                );
                let goal_prop = brix_kernel::Prop::Realizes(
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(g1.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(src.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(dst.digest())),
                );
                let implication_prop = brix_kernel::Prop::Impl(Box::new(h1), Box::new(goal_prop));

                let explicit_term = brix_kernel::ExplicitTerm::new(
                    context,
                    brix_kernel::TermKind::Lam {
                        var_name: Some("h1".to_string()),
                        body: Box::new(brix_kernel::TermKind::Hyp(brix_kernel::Var::Index(0))),
                    },
                );

                let cert_id =
                    brix_kernel::certificate_id_v1(&brix_kernel::CertificateMaterialV1::new(
                        &context,
                        &implication_prop,
                        &explicit_term,
                    ));
                let expected_evidence = Evidence::KernelCertificate {
                    verifier: expected_verifier,
                    certificate: cert_id,
                };

                assert_eq!(judgement.evidence, expected_evidence.id());
                assert_eq!(judgement.proposition, implication_prop.proposition_id());
            }
            ElaborationResult::NotElaborated(verdict) => {
                panic!("Expected Proven, got NotElaborated({verdict:?})");
            }
        }
    }

    #[test]
    fn test_multi_step_elaboration_tree_vs_linear_tension() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (audited_judgement, verified_decomp) =
            audited_type_check(&expr, &ctx, context).expect("audited type check identity app");

        // 1. Syntactic elaboration passes via endpoints-only padding because dst == dst in RealizesComp
        let budget = Budget::new(1000, 1000);
        let res = elaborate_decomposition(&audited_judgement, &verified_decomp, budget);
        assert!(
            matches!(res, ElaborationResult::Proven { .. }),
            "Padded configs pass syntactic RealizesComp because dst == dst middle match holds"
        );

        // 2. But padded configs fail semantic audit because g_lam, g_var, g_lit, etc. do NOT realize (dst, dst)
        let mut registry = GeneratorRegistry::new();
        registry.insert(g_app());
        registry.insert(g_lam());
        registry.insert(g_var());
        registry.insert(g_lit());
        registry.insert(g_unify());

        struct NonPaddedSemantics;
        impl soc_core::GeneratorSemantics for NonPaddedSemantics {
            fn realizes(&self, _g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
                src != dst
            }
        }

        let unverified_decomp = Decomposition::recorded(
            verified_decomp.generators.clone(),
            verified_decomp.configs.clone(),
        )
        .unwrap();

        let derived_evidence = Evidence::SettlementReplay {
            body: unverified_decomp.id().digest(),
        }
        .id();
        let derived_id = Judgement::new(
            context,
            audited_judgement.proposition,
            Outcome::Derived,
            derived_evidence,
        )
        .id();

        let step = soc_core::CommittedStep {
            key: soc_core::Key::new(
                0,
                0,
                brix_canon::Digest::of(brix_canon::Domain::Value, b"app_tiebreak"),
            ),
            observation: soc_core::Observation {
                outcome_class: Outcome::Derived,
                judgement_digest: derived_id.digest(),
            },
            decomposition: unverified_decomp,
            src: expr.config_id(),
            dst: Ty::Con("Int").config_id(),
            witness: compose_chain(&verified_decomp.generators).unwrap(),
        };

        let audit_res = soc_core::audit_step(&step, context, &registry, &NonPaddedSemantics);
        assert!(
            matches!(audit_res, soc_core::AuditResult::Unknown(_)),
            "Audit MUST fail for endpoints-padded configs under sound generator semantics"
        );
    }

    #[test]
    fn test_tree_elaboration_end_to_end() {
        let expr = Expr::App(Box::new(Expr::Var("f".to_string())), Box::new(Expr::Lit(1)));
        let ctx = TyCtx::new().extend(
            "f",
            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))),
        );
        let context = ContextId::root();

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);

        // Inspect tree: g_app2 leaf src config equals Prod(Atom(Fn(Int,Bool).config_id()), Atom(Int.config_id()))
        if let RealizesTree::Seq { right, .. } = &tree {
            if let RealizesTree::Seq {
                right: app_leaf, ..
            } = right.as_ref()
            {
                if let RealizesTree::Leaf { generator, src, .. } = app_leaf.as_ref() {
                    assert_eq!(*generator, g_app2());
                    let expected_src = TreeObj::Prod(
                        Box::new(TreeObj::Atom(
                            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))).config_id(),
                        )),
                        Box::new(TreeObj::Atom(Ty::Con("Int").config_id())),
                    );
                    assert_eq!(*src, expected_src);
                } else {
                    panic!("Expected app leaf");
                }
            } else {
                panic!("Expected inner Seq");
            }
        } else {
            panic!("Expected outer Seq");
        }

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(
                    judgement.outcome.authority(),
                    brix_semantic::Authority::ProofKernel
                );
                assert_eq!(edge.kind, brix_semantic::EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, aud.id().digest());
            }
            other => panic!("Expected Proven, got {:?}", other),
        }

        // Determinism check
        let (aud2, tree2) = audited_type_check_tree(&expr, &ctx, context).unwrap();
        assert_eq!(aud, aud2);
        assert_eq!(tree, tree2);
    }

    #[test]
    fn test_lambda_end_to_end() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let (aud, tree) = audited_type_check_tree(&expr, &ctx, ContextId::root()).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
                assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, aud.id().digest());
            }
            other => panic!("Expected Proven, got {:?}", other),
        }

        let expected_prop = Realizes::new(
            tree.witness_object().witness_digest(),
            expr.config_id(),
            Ty::Con("Int").config_id(),
        )
        .proposition_id();
        assert_eq!(aud.proposition, expected_prop);
    }

    #[test]
    fn test_lambda_zonk_fired() {
        let expr = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let (_, ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).unwrap();
        let tree = materialize(&ty_tree, &st.subst);

        if let RealizesTree::Seq { right: app_seq, .. } = &tree {
            if let RealizesTree::Seq { left: tensor, .. } = app_seq.as_ref() {
                if let RealizesTree::Tensor { left: df, .. } = tensor.as_ref() {
                    if let RealizesTree::Seq { right: lam_seq, .. } = df.as_ref() {
                        if let RealizesTree::Seq {
                            right: close_box, ..
                        } = lam_seq.as_ref()
                        {
                            if let RealizesTree::Leaf { generator, dst, .. } = close_box.as_ref() {
                                assert_eq!(*generator, g_lam_close());
                                let expected_fn_config =
                                    Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Int")))
                                        .config_id();
                                assert_eq!(*dst, TreeObj::Atom(expected_fn_config));
                                return;
                            }
                        }
                    }
                }
            }
        }
        panic!("Failed to navigate tree to g_lam_close leaf");
    }

    #[test]
    fn test_bare_lambda_audited() {
        let expr = Expr::Lam("x".to_string(), Box::new(Expr::Var("x".to_string())));
        let ctx = TyCtx::new();
        let (aud, tree) = audited_type_check_tree(&expr, &ctx, ContextId::root()).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.well_formed());
    }

    #[test]
    fn test_record_2_fields_proven() {
        // Record literal with 2 fields in reverse order: {y: 2, x: 1}
        let expr = Expr::Record(vec![
            ("y".to_string(), Expr::Lit(2)),
            ("x".to_string(), Expr::Lit(1)),
        ]);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer record");
        let final_ty = zonk(&ty, &st.subst);
        let expected_ty = Ty::Record(vec![
            ("x".to_string(), Ty::Con("Int")),
            ("y".to_string(), Ty::Con("Int")),
        ]);
        assert_eq!(final_ty, expected_ty);

        // Canonical identity check: {y: 2, x: 1} and {x: 1, y: 2} have the same config_id
        let expr_sorted = Expr::Record(vec![
            ("x".to_string(), Expr::Lit(1)),
            ("y".to_string(), Expr::Lit(2)),
        ]);
        assert_eq!(expr.config_id(), expr_sorted.config_id());

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited record");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_record_1_field_and_nested_proven() {
        // 1-field record: {a: 42}
        let expr1 = Expr::Record(vec![("a".to_string(), Expr::Lit(42))]);
        let ctx = TyCtx::new();
        let (aud1, tree1) = audited_type_check_tree(&expr1, &ctx, ContextId::root()).unwrap();
        assert!(tree1.well_formed());
        assert!(matches!(
            brix_elaborate::elaborate_tree(&aud1, &tree1, Budget::new(1000, 1000)),
            ElaborationResult::Proven { .. }
        ));

        // Nested record: {inner: {val: 7}}
        let expr_nested = Expr::Record(vec![(
            "inner".to_string(),
            Expr::Record(vec![("val".to_string(), Expr::Lit(7))]),
        )]);
        let (aud2, tree2) = audited_type_check_tree(&expr_nested, &ctx, ContextId::root()).unwrap();
        assert!(tree2.well_formed());
        let res2 = brix_elaborate::elaborate_tree(&aud2, &tree2, Budget::new(2000, 2000));
        assert!(matches!(res2, ElaborationResult::Proven { .. }));
    }

    #[test]
    fn test_field_access_proven() {
        // r.base where r: {base: Int, name: Int}
        let r_ty = Ty::Record(vec![
            ("base".to_string(), Ty::Con("Int")),
            ("name".to_string(), Ty::Con("Int")),
        ]);
        let ctx = TyCtx::new().extend("r", r_ty);
        let expr = Expr::Field(Box::new(Expr::Var("r".to_string())), "base".to_string());
        let context = ContextId::root();

        let (ty, _tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer field access");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, Ty::Con("Int"));

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited field");
        assert!(tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_field_access_missing_field_type_error() {
        let r_ty = Ty::Record(vec![("x".to_string(), Ty::Con("Int"))]);
        let ctx = TyCtx::new().extend("r", r_ty);
        let expr = Expr::Field(Box::new(Expr::Var("r".to_string())), "y".to_string());

        let res = audited_type_check_tree(&expr, &ctx, ContextId::root());
        assert_eq!(res, Err(TypeError::NoField("y".to_string())));
    }

    #[test]
    fn numeric_lattice_join_paths_and_fields() {
        // Joins along the chain and across the Int↪Float branch.
        assert_eq!(NUMERIC.join("Int", "Float"), Some("Float"));
        assert_eq!(NUMERIC.join("Int", "Rat"), Some("Rat"));
        assert_eq!(NUMERIC.join("Nat", "Complex"), Some("Complex"));
        assert_eq!(NUMERIC.join("Rat", "Real"), Some("Real"));
        assert_eq!(NUMERIC.join("Int", "Int"), Some("Int"));
        // Float is incomparable to the exact ℚ/ℝ/ℂ nodes → no join (a type error).
        assert_eq!(NUMERIC.join("Float", "Rat"), None);
        assert_eq!(NUMERIC.join("Float", "Real"), None);
        assert_eq!(NUMERIC.join("Float", "Complex"), None);

        // Witness paths: empty when already there, one edge per hop.
        assert_eq!(NUMERIC.edge_path("Int", "Int"), Vec::new());
        assert_eq!(NUMERIC.edge_path("Int", "Float"), vec![("Int", "Float")]);
        assert_eq!(
            NUMERIC.edge_path("Nat", "Real"),
            vec![("Nat", "Int"), ("Int", "Rat"), ("Rat", "Real")]
        );

        // Div forces at least the field of fractions.
        assert_eq!(field_of("Int"), "Float");
        assert_eq!(field_of("Nat"), "Float");
        assert_eq!(field_of("Rat"), "Rat");
        assert_eq!(field_of("Complex"), "Complex");
    }

    #[test]
    fn epistemic_grades_ride_the_same_coercion_lattice() {
        // Weakening certainty is the safe direction: Proven ↪ Audited ↪ Derived.
        assert!(GRADE.le("Proven", "Derived"));
        assert!(GRADE.le("Audited", "Derived"));
        assert!(GRADE.le("Proven", "Proven"));

        // Combining two grades meets at the weaker guarantee (their join).
        assert_eq!(GRADE.join("Proven", "Audited"), Some("Audited"));
        assert_eq!(GRADE.join("Derived", "Proven"), Some("Derived"));
        assert_eq!(GRADE.join("Audited", "Audited"), Some("Audited"));

        // Strengthening without evidence has NO up-path — this is epistemic
        // erasure, rejected by the very same code as a numeric mismatch.
        assert!(!GRADE.le("Derived", "Proven"));
        assert!(!GRADE.le("Audited", "Proven"));
        assert_eq!(GRADE.edge_path("Proven", "Derived").len(), 2);

        // Erasure surfaces as "no join in the strengthening direction": there is
        // no witness from Derived up to Proven, so a Proven-demanding site fed a
        // Derived value cannot be coerced. (join is symmetric; the *directed*
        // check `le(from, to)` is the erasure guard.)
        assert!(!GRADE.le("Derived", "Proven"));
    }

    #[test]
    fn arith_over_exact_tower_nodes_promotes_and_proves() {
        // x:Rat, y:Int → x + y : Rat, splicing a witnessed Int↪Rat promotion,
        // and the resulting tree elaborates to Proven. Exercises lattice nodes
        // not yet reachable from surface literals.
        let ctx = TyCtx::new()
            .extend("x", Ty::Con("Rat"))
            .extend("y", Ty::Con("Int"));
        let expr = Expr::Arith(
            ArithOp::Add,
            Box::new(Expr::Var("x".to_string())),
            Box::new(Expr::Var("y".to_string())),
        );

        let (ty, _tree, _st) = infer_tree(&expr, &ctx, Infer::new()).unwrap();
        assert_eq!(ty, Ty::Con("Rat"));

        let (judgement, tree) =
            audited_type_check_tree(&expr, &ctx, ContextId::root()).expect("audited arith");
        assert_eq!(judgement.outcome, Outcome::Audited);

        let res = brix_elaborate::elaborate_tree(&judgement, &tree, Budget::new(2000, 2000));
        assert!(
            matches!(res, ElaborationResult::Proven { .. }),
            "expected Proven, got {res:?}"
        );
    }

    #[test]
    fn integer_division_promotes_to_the_field_of_fractions() {
        // 7 / 2 : Float (Div forces the field of fractions), reaching Proven.
        let expr = Expr::Arith(ArithOp::Div, Box::new(Expr::Lit(7)), Box::new(Expr::Lit(2)));
        let ctx = TyCtx::new();
        let (ty, _, _) = infer_tree(&expr, &ctx, Infer::new()).unwrap();
        assert_eq!(ty, Ty::Con("Float"));

        let (judgement, tree) =
            audited_type_check_tree(&expr, &ctx, ContextId::root()).expect("audited div");
        assert_eq!(judgement.outcome, Outcome::Audited);
        let res = brix_elaborate::elaborate_tree(&judgement, &tree, Budget::new(2000, 2000));
        assert!(
            matches!(res, ElaborationResult::Proven { .. }),
            "expected Proven, got {res:?}"
        );
    }

    #[test]
    fn literal_intro_generators_are_faithful() {
        // Each discharged literal generator is emitted at exactly one site with a
        // fixed `literal → Con` endpoint. This pins the discharge: if a literal
        // arm ever emitted a different generator or result type, tightness would
        // no longer be honest and this test would catch it.
        let cases: [(Expr, GeneratorId, Ty); 3] = [
            (Expr::Lit(7), g_lit(), Ty::Con("Int")),
            (Expr::StrLit("hi".to_string()), g_str_lit(), Ty::Con("Str")),
            (
                Expr::FloatLit("1.5".to_string()),
                g_float_lit(),
                Ty::Con("Float"),
            ),
        ];
        for (expr, gen, ty) in cases {
            assert!(generator_is_tight(&gen), "{gen:?} should be discharged");
            let (_, tree, _) = infer_tree(&expr, &TyCtx::new(), Infer::new()).unwrap();
            match tree {
                TyTree::Leaf {
                    generator,
                    src,
                    dst,
                } => {
                    assert_eq!(generator, gen);
                    assert_eq!(src, TyObj::Atom(CfgAtom::Expr(expr)));
                    assert_eq!(dst, TyObj::Atom(CfgAtom::Type(ty)));
                }
                other => panic!("expected a single faithful leaf, got {other:?}"),
            }
        }
    }

    #[test]
    fn literal_binding_earns_proven_but_composite_stays_audited() {
        // A pure literal rests only on a discharged (tight) generator, so its
        // honest result outcome is the kernel's Proven composition.
        let (_, lit_tree) =
            audited_type_check_tree(&Expr::Lit(42), &TyCtx::new(), ContextId::root()).unwrap();
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &lit_tree),
            Outcome::Proven,
            "a discharged literal should earn Proven"
        );

        // `(λx.x) 42` rests on the discharged λ-calculus core (g_var/g_lam_*/
        // g_split/g_app2) + g_lit — all tight — so it now earns Proven too.
        let app = Expr::App(
            Box::new(Expr::Lam(
                "x".to_string(),
                Box::new(Expr::Var("x".to_string())),
            )),
            Box::new(Expr::Lit(42)),
        );
        let (_, app_tree) =
            audited_type_check_tree(&app, &TyCtx::new(), ContextId::root()).unwrap();
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &app_tree),
            Outcome::Proven,
            "the discharged λ-calculus core should earn Proven"
        );

        // `1 + 2` uses g_arith, which asserts operation semantics and is NOT
        // discharged, so it is honestly capped at Audited.
        let arith = Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let (_, arith_tree) =
            audited_type_check_tree(&arith, &TyCtx::new(), ContextId::root()).unwrap();
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &arith_tree),
            Outcome::Audited,
            "an undischarged operation generator (g_arith) must cap at Audited"
        );
    }

    #[test]
    fn application_rule_is_a_kernel_theorem() {
        // Soundness evidence for discharging g_app2: its realization is modus
        // ponens `((A→B) × A) → B`, a kernel theorem witnessed by λp.(π₁p)(π₂p).
        use brix_kernel::{ExplicitTerm, Prop, TermKind, Var, Verdict};
        use brix_semantic::PropositionId;

        let a = Prop::Atom(PropositionId::from_canon(b"A"));
        let b = Prop::Atom(PropositionId::from_canon(b"B"));
        let a_to_b = Prop::Impl(Box::new(a.clone()), Box::new(b.clone()));
        let goal = Prop::Impl(
            Box::new(Prop::Prod(Box::new(a_to_b), Box::new(a))),
            Box::new(b),
        );
        let term = TermKind::Lam {
            var_name: Some("p".to_string()),
            body: Box::new(TermKind::App {
                function: Box::new(TermKind::Proj1(Box::new(TermKind::Hyp(Var::Named(
                    "p".to_string(),
                ))))),
                argument: Box::new(TermKind::Proj2(Box::new(TermKind::Hyp(Var::Named(
                    "p".to_string(),
                ))))),
            }),
        };
        let ctx = ContextId::root();
        let explicit = ExplicitTerm::new(ctx, term);
        assert!(
            matches!(
                brix_kernel::acceptance(&ctx, &goal, &explicit, Budget::new(1000, 1000)),
                Verdict::Accepted(_)
            ),
            "modus ponens must be a kernel theorem"
        );
    }

    #[test]
    fn structural_generators_are_faithful_kernel_rules() {
        // The discharged structural rules are precisely the primitive rules the
        // kernel accepts: product introduction/projection and coproduct
        // introduction/elimination.  The n-ary Brix forms use right-nested
        // binary products/coproducts, so the examples below pin that encoding
        // rather than assuming an n-ary rule in the kernel.
        use brix_kernel::{ExplicitTerm, Prop, TermKind, Var, Verdict};
        use brix_semantic::PropositionId;

        let atom = |name| Prop::Atom(PropositionId::from_canon(name));
        let a = atom(b"A");
        let b = atom(b"B");
        let c = atom(b"C");
        let r = atom(b"R");
        let ctx = ContextId::root();
        let accepts = |goal, term| {
            matches!(
                brix_kernel::acceptance(
                    &ctx,
                    &goal,
                    &ExplicitTerm::new(ctx, term),
                    Budget::new(1_000, 1_000),
                ),
                Verdict::Accepted(_)
            )
        };

        // g_record: A -> B -> A × B (product introduction).
        let product_intro = Prop::Impl(
            Box::new(a.clone()),
            Box::new(Prop::Impl(
                Box::new(b.clone()),
                Box::new(Prop::Prod(Box::new(a.clone()), Box::new(b.clone()))),
            )),
        );
        assert!(accepts(
            product_intro,
            TermKind::Lam {
                var_name: Some("a".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("b".into()),
                    body: Box::new(TermKind::Pair {
                        fst: Box::new(TermKind::Hyp(Var::Named("a".into()))),
                        snd: Box::new(TermKind::Hyp(Var::Named("b".into()))),
                    }),
                }),
            },
        ));

        // g_field: the final field of A × (B × C) is selected with the
        // right-nested product projection π₂(π₂ p).
        let nested_product = Prop::Prod(
            Box::new(a.clone()),
            Box::new(Prop::Prod(Box::new(b.clone()), Box::new(c.clone()))),
        );
        assert!(accepts(
            Prop::Impl(Box::new(nested_product), Box::new(c.clone())),
            TermKind::Lam {
                var_name: Some("p".into()),
                body: Box::new(TermKind::Proj2(Box::new(TermKind::Proj2(Box::new(
                    TermKind::Hyp(Var::Named("p".into())),
                ))))),
            },
        ));

        // g_ctor: inject the middle alternative into A + (B + C), matching
        // Brix's right-nested nominal-sum representation.
        let nested_sum = Prop::Sum(
            Box::new(a.clone()),
            Box::new(Prop::Sum(Box::new(b.clone()), Box::new(c.clone()))),
        );
        assert!(accepts(
            Prop::Impl(Box::new(b.clone()), Box::new(nested_sum.clone())),
            TermKind::Lam {
                var_name: Some("b".into()),
                body: Box::new(TermKind::Inr(Box::new(TermKind::Inl(Box::new(
                    TermKind::Hyp(Var::Named("b".into())),
                ))))),
            },
        ));

        // g_match: (A -> R) -> (B -> R) -> (A + B -> R), the kernel's
        // coproduct eliminator. Pattern coverage is deliberately separate: a
        // `proving exhaustive` annotation may still have no coverage
        // certificate even when ordinary match typing is `Proven`.
        let match_elim = Prop::Impl(
            Box::new(Prop::Impl(Box::new(a.clone()), Box::new(r.clone()))),
            Box::new(Prop::Impl(
                Box::new(Prop::Impl(Box::new(b.clone()), Box::new(r.clone()))),
                Box::new(Prop::Impl(
                    Box::new(Prop::Sum(Box::new(a.clone()), Box::new(b.clone()))),
                    Box::new(r),
                )),
            )),
        );
        assert!(accepts(
            match_elim,
            TermKind::Lam {
                var_name: Some("ha".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("hb".into()),
                    body: Box::new(TermKind::Lam {
                        var_name: Some("s".into()),
                        body: Box::new(TermKind::Case {
                            discriminant: Box::new(TermKind::Hyp(Var::Named("s".into()))),
                            left_var: Some("x".into()),
                            left_body: Box::new(TermKind::App {
                                function: Box::new(TermKind::Hyp(Var::Named("ha".into()))),
                                argument: Box::new(TermKind::Hyp(Var::Named("x".into()))),
                            }),
                            right_var: Some("y".into()),
                            right_body: Box::new(TermKind::App {
                                function: Box::new(TermKind::Hyp(Var::Named("hb".into()))),
                                argument: Box::new(TermKind::Hyp(Var::Named("y".into()))),
                            }),
                        }),
                    }),
                }),
            },
        ));

        // The empty record and a nullary constructor are intentionally NOT
        // smuggled into those binary rules: Profile 1.2 lacks unit/zero rules.
        fn has_leaf(tree: &RealizesTree, want: &GeneratorId) -> bool {
            match tree {
                RealizesTree::Leaf { generator, .. } => generator == want,
                RealizesTree::Seq { left, right } | RealizesTree::Tensor { left, right } => {
                    has_leaf(left, want) || has_leaf(right, want)
                }
            }
        }

        let empty_record = Expr::Record(vec![]);
        let (_, empty_tree) =
            audited_type_check_tree(&empty_record, &TyCtx::new(), ctx).expect("empty record");
        assert!(has_leaf(&empty_tree, &g_record_empty()));
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &empty_tree),
            Outcome::Audited
        );

        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        let nullary_ctor = Expr::Ctor(bool_ty, "True".into(), vec![]);
        let (_, nullary_tree) =
            audited_type_check_tree(&nullary_ctor, &TyCtx::new(), ctx).expect("nullary ctor");
        assert!(has_leaf(&nullary_tree, &g_ctor_nullary()));
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &nullary_tree),
            Outcome::Audited
        );
    }

    #[test]
    fn structural_tree_endpoints_match_the_checked_expression() {
        // The tree handed to elaboration must prove exactly the same source and
        // target as the audited typing claim. In particular, field projection
        // needs an explicit `g_field_split`: without it the old tree began at
        // the base expression rather than at `base.field`.
        let assert_endpoints = |expr: Expr, ctx: TyCtx| {
            let (ty, _tree, state) = infer_tree(&expr, &ctx, Infer::new()).expect("infer");
            let expected_src = TreeObj::Atom(expr.config_id());
            let expected_dst = TreeObj::Atom(zonk(&ty, &state.subst).config_id());
            let (_audited, tree) =
                audited_type_check_tree(&expr, &ctx, ContextId::root()).expect("audited");
            assert_eq!(tree.src(), expected_src, "wrong tree source for {expr:?}");
            assert_eq!(tree.dst(), expected_dst, "wrong tree target for {expr:?}");
        };

        assert_endpoints(Expr::Record(vec![("x".into(), Expr::Lit(1))]), TyCtx::new());
        assert_endpoints(
            Expr::Record(vec![("x".into(), Expr::Lit(1)), ("y".into(), Expr::Lit(2))]),
            TyCtx::new(),
        );
        assert_endpoints(
            Expr::Field(Box::new(Expr::Var("r".into())), "x".into()),
            TyCtx::new().extend("r", Ty::Record(vec![("x".into(), Ty::Con("Int"))])),
        );

        let opt = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        assert_endpoints(
            Expr::Ctor(opt.clone(), "Some".into(), vec![Expr::Lit(1)]),
            TyCtx::new(),
        );
        let pair = Ty::Sum(
            "Pair".into(),
            vec![("MkPair".into(), vec![Ty::Con("Int"), Ty::Con("Str")])],
        );
        assert_endpoints(
            Expr::Ctor(
                pair,
                "MkPair".into(),
                vec![Expr::Lit(1), Expr::StrLit("x".into())],
            ),
            TyCtx::new(),
        );
        assert_endpoints(
            Expr::Match(
                Box::new(Expr::Var("o".into())),
                vec![
                    (Pattern::Ctor("None".into(), vec![]), Expr::Lit(0)),
                    (
                        Pattern::Ctor("Some".into(), vec![Pattern::Var("n".into())]),
                        Expr::Var("n".into()),
                    ),
                ],
            ),
            TyCtx::new().extend("o", opt),
        );
        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        assert_endpoints(
            Expr::Match(
                Box::new(Expr::Var("b".into())),
                vec![
                    (Pattern::Ctor("True".into(), vec![]), Expr::Lit(1)),
                    (Pattern::Wildcard, Expr::Lit(0)),
                ],
            ),
            TyCtx::new().extend("b", bool_ty),
        );
    }

    #[test]
    fn test_ctor_nullary_bool_stays_audited() {
        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        let expr = Expr::Ctor(bool_ty.clone(), "True".into(), vec![]);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer ctor nullary");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, bool_ty);

        let (aud, tree) =
            audited_type_check_tree(&expr, &ctx, context).expect("audited ctor nullary");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &tree),
            Outcome::Audited,
            "nullary constructor lacks a kernel zero/unit introduction rule"
        );
        assert!(tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_ctor_opt_some_proven() {
        let opt_ty = Ty::Sum(
            "Option".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Ctor(opt_ty.clone(), "Some".into(), vec![Expr::Lit(3)]);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer ctor opt");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, opt_ty);

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited ctor opt");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_ctor_pair_proven() {
        let pair_ty = Ty::Sum(
            "Pair".into(),
            vec![("MkPair".into(), vec![Ty::Con("Int"), Ty::Con("Str")])],
        );
        let expr = Expr::Ctor(
            pair_ty.clone(),
            "MkPair".into(),
            vec![Expr::Lit(1), Expr::StrLit("x".into())],
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, _ty_tree, st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer ctor pair");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, pair_ty);

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).expect("audited ctor pair");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_ctor_arg_type_mismatch() {
        let opt_ty = Ty::Sum(
            "Option".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Ctor(opt_ty, "Some".into(), vec![Expr::StrLit("bad".into())]);
        let ctx = TyCtx::new();

        let res = audited_type_check_tree(&expr, &ctx, ContextId::root());
        assert_eq!(res, Err(TypeError::Mismatch));
    }

    #[test]
    fn test_ctor_unknown_variant() {
        let opt_ty = Ty::Sum(
            "Option".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Ctor(opt_ty, "Nope".into(), vec![]);
        let ctx = TyCtx::new();

        let res = audited_type_check_tree(&expr, &ctx, ContextId::root());
        assert_eq!(res, Err(TypeError::Mismatch));
    }

    #[test]
    fn structural_generators_are_tight() {
        for generator in [
            g_record_split(),
            g_record(),
            g_field_split(),
            g_field(),
            g_ctor_split(),
            g_ctor(),
            g_match_split(),
            g_match(),
        ] {
            assert!(
                generator_is_tight(&generator),
                "{generator:?} should be discharged"
            );
        }
        for generator in [g_record_empty(), g_ctor_nullary(), g_match_catchall()] {
            assert!(
                !generator_is_tight(&generator),
                "{generator:?} must remain undischarged"
            );
        }
    }

    #[test]
    fn test_match_opt_some_var_binding_proven() {
        let opt_ty = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        // match o { None => 0, Some(k) => k }
        let expr = Expr::Match(
            Box::new(Expr::Var("o".into())),
            vec![
                (Pattern::Ctor("None".into(), vec![]), Expr::Lit(0)),
                (
                    Pattern::Ctor("Some".into(), vec![Pattern::Var("k".into())]),
                    Expr::Var("k".into()),
                ),
            ],
        );
        let ctx = TyCtx::new().extend("o", opt_ty);
        let context = ContextId::root();

        let (ty, _tree, _st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer match opt");
        assert_eq!(ty, Ty::Con("Int"));

        let (aud, real_tree) =
            audited_type_check_tree(&expr, &ctx, context).expect("audited match opt");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert!(real_tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &real_tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_match_wildcard_catch_all_stays_audited() {
        let bool_ty = Ty::Sum(
            "Bool".into(),
            vec![("True".into(), vec![]), ("False".into(), vec![])],
        );
        // match b { True => 1, _ => 0 }
        let expr = Expr::Match(
            Box::new(Expr::Var("b".into())),
            vec![
                (Pattern::Ctor("True".into(), vec![]), Expr::Lit(1)),
                (Pattern::Wildcard, Expr::Lit(0)),
            ],
        );
        let ctx = TyCtx::new().extend("b", bool_ty);
        let context = ContextId::root();

        let (ty, _tree, _st) = infer_tree(&expr, &ctx, Infer::new()).expect("infer match wildcard");
        assert_eq!(ty, Ty::Con("Int"));

        let (aud, real_tree) =
            audited_type_check_tree(&expr, &ctx, context).expect("audited match wildcard");
        assert_eq!(aud.outcome, Outcome::Audited);
        assert_eq!(
            honest_result_outcome(Outcome::Proven, &real_tree),
            Outcome::Audited,
            "a catch-all arm is not yet represented as explicit Case premises"
        );
        assert!(real_tree.well_formed());

        let res = brix_elaborate::elaborate_tree(&aud, &real_tree, Budget::new(2000, 2000));
        match res {
            ElaborationResult::Proven { judgement, .. } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
            }
            other => panic!("Expected Proven, got {:?}", other),
        }
    }

    #[test]
    fn test_match_non_exhaustive() {
        let opt_ty = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        // match o { None => 0 } -> missing Some
        let expr = Expr::Match(
            Box::new(Expr::Var("o".into())),
            vec![(Pattern::Ctor("None".into(), vec![]), Expr::Lit(0))],
        );
        let ctx = TyCtx::new().extend("o", opt_ty);

        let res = infer_tree(&expr, &ctx, Infer::new());
        assert_eq!(
            res.unwrap_err(),
            TypeError::NonExhaustive(vec!["Some".into()])
        );
    }

    #[test]
    fn test_match_on_non_sum() {
        // match (1) { _ => 0 } -> 1 is Int, not a Sum
        let expr = Expr::Match(
            Box::new(Expr::Lit(1)),
            vec![(Pattern::Wildcard, Expr::Lit(0))],
        );
        let ctx = TyCtx::new();

        let res = infer_tree(&expr, &ctx, Infer::new());
        assert_eq!(res.unwrap_err(), TypeError::Mismatch);
    }

    #[test]
    fn test_nested_ctor_pattern_unsupported() {
        let opt_ty = Ty::Sum(
            "Opt".into(),
            vec![
                ("None".into(), vec![]),
                ("Some".into(), vec![Ty::Con("Int")]),
            ],
        );
        let expr = Expr::Match(
            Box::new(Expr::Var("o".into())),
            vec![
                (
                    Pattern::Ctor("Some".into(), vec![Pattern::Ctor("Some".into(), vec![])]),
                    Expr::Lit(0),
                ),
                (Pattern::Wildcard, Expr::Lit(0)),
            ],
        );
        let ctx = TyCtx::new().extend("o", opt_ty);

        let res = infer_tree(&expr, &ctx, Infer::new());
        assert_eq!(res.unwrap_err(), TypeError::Unsupported);
    }
}
