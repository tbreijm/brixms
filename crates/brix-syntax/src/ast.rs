//! The Brix surface AST (SOC paradigm) — ADR-0010, L1.
//!
//! Design commitments this AST encodes (user-ratified 2026-07-31):
//! - **SOC is the spine**: `config` (configuration families), `regime`/`gen`
//!   (logged generators 𝒢), and `rule` (settlement rules that propose→commit)
//!   are first-class top-level items, not library calls.
//! - **Progressive disclosure**: grades (`@Derived`/`@Audited`/`@Proven`) and
//!   witnesses are *optional* surface — everyday `.brix` omits them entirely
//!   (they are inferred), power users opt in. Erasure is still checked even
//!   when grades are invisible, so [`Grade`] lives on [`Ty`] as an option, not
//!   a requirement.
//! - **Keyword composition ops**: witness composition is `then` (sequential ∘)
//!   and `and` (parallel ⊗) — see [`BinOp`]. Never `;`/`⊗`.
//!
//! This is intentionally a *surface* AST: it preserves what was written
//! (numeric/string literals as text, optional annotations) and defers all
//! semantics to L2 lowering (→ configs/generators/candidates). It is a fresh
//! design and deliberately NOT the legacy `soc_regimes::native` `NExpr`/`NTy`.

/// A parsed `.brix` module: an ordered list of top-level items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub items: Vec<Item>,
}

/// A top-level declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// `config Name = <body>` — a configuration family (algebraic).
    Config(ConfigDecl),
    /// `regime Name { gen … }` — a realization regime bundling generators.
    Regime(RegimeDecl),
    /// `rule name(params) [: Ty] = expr` — a settlement rule (propose→commit).
    Rule(Callable),
    /// `let name [: Ty] = expr` — a binding.
    Let(LetDecl),
    /// `fn name(params) [: Ty] = expr` — a pure (lax) helper, not a logged
    /// generator. Distinct from `gen`/`rule` so the lax/tight split is explicit.
    Fn(Callable),
    /// `show expr` — surface a committed fact (query/print).
    Show(Expr),
    /// `witness name = expr` — bind a witness value (power-user surface;
    /// sugar-equivalent to `let`, kept distinct for round-trip fidelity).
    Witness { name: String, value: Expr },
}

/// `config Name = <body>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDecl {
    pub name: String,
    /// Declared type parameters, `config List<T> = …`. Empty for an ordinary
    /// config, which keeps every existing declaration unchanged.
    pub params: Vec<String>,
    pub body: ConfigBody,
}

/// The right-hand side of a `config` declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigBody {
    /// `Zero | Succ(Nat)` — a sum of variants.
    Sum(Vec<Variant>),
    /// `{ name: Str, base: Money }` — a record of named fields.
    Record(Vec<FieldDecl>),
}

/// One variant of a sum config, e.g. `Succ(Nat)` or `Zero` (empty params).
///
/// A **named-field** variant — `MonsterData { frame: MonsterFrame, atk: Int }` —
/// is desugared here into a single positional parameter whose type is
/// [`Ty::Record`]. The two spellings mean the same thing, so keeping one
/// representation avoids a second source of truth about a variant's payload;
/// the surface distinction survives only in how it is written and constructed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub params: Vec<Ty>,
}

/// One named field of a record config or record literal type position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: Ty,
}

/// `regime Name { gen … }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegimeDecl {
    pub name: String,
    pub gens: Vec<Callable>,
}

/// A `gen` / `rule` / `fn` — same shape, distinguished by the enclosing [`Item`]
/// / [`RegimeDecl`]. (`gen` appears in a regime; `rule` and `fn` at top level.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callable {
    pub name: String,
    pub params: Vec<Param>,
    /// Optional declared return type (inferred when absent).
    pub ret: Option<Ty>,
    pub body: Expr,
}

/// A parameter `name [: Ty]` (type inferred when absent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Option<Ty>,
}

/// `let name [: Ty] = expr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetDecl {
    pub name: String,
    /// Optional type annotation, itself optionally grade-annotated. An
    /// annotation is an *assertion the checker must discharge*, never required.
    pub ty: Option<Ty>,
    pub value: Expr,
}

/// A surface type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// A named type: `Money`, `Str`, `Item`, `Nat`, …
    Named(String),
    /// A grade-annotated type: `Ty @Grade`. Only the outermost annotation is
    /// meaningful at the surface; the graded modality wraps the payload type.
    Graded(Box<Ty>, Grade),
    /// An anonymous record type, `{ name: Str, atk: Int }`.
    ///
    /// Written directly in a type position, and also the desugaring target for
    /// a named-field sum variant (see [`Variant`]).
    Record(Vec<FieldDecl>),
    /// Type application, `List<Int>` — a parameterized config at a specific
    /// instantiation.
    App(String, Vec<Ty>),
}

/// An epistemic grade on the outcome lattice (`Derived → Audited → Proven`).
/// A modality on a type; erasure (dropping it) is a checked error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    Derived,
    Audited,
    Proven,
}

/// A surface expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// Numeric literal, preserved as written (`10`, `1.2`) — interpretation
    /// deferred to lowering.
    Num(String),
    /// String literal.
    Str(String),
    /// Boolean literal, `true` or `false`.
    Bool(bool),
    /// A variable / nullary reference (also a nullary constructor like `Zero`).
    Var(String),
    /// Record literal: `Item { name: e, base: e }`.
    Record {
        config: String,
        fields: Vec<(String, Expr)>,
    },
    /// Field / role projection: `e.field`.
    Field(Box<Expr>, String),
    /// Application: `f(args)` — a call, a generator/rule invocation, or a
    /// constructor application (`Succ(k)`); disambiguated at lowering.
    Call { func: String, args: Vec<Expr> },
    /// Binary operator, including the witness composition ops `then`/`and`.
    Bin {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `match e { pat => e, … } [proving exhaustive]`.
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        /// `true` when the source wrote `proving exhaustive` after the match
        /// block — a request that exhaustiveness be kernel-certified.
        /// Semantics (coverage checking) are handled downstream; this field
        /// only carries the surface request.
        proving_exhaustive: bool,
    },
    /// `prove e` — power-user: elaborate to the kernel → a `@Proven` value.
    Prove(Box<Expr>),
    /// `why(e)` — power-user: the witness (derivation) behind `e`.
    Why(Box<Expr>),
    /// `audit e` — power-user: force replay-verification → `@Audited`.
    Audit(Box<Expr>),
}

/// Binary operators. Arithmetic ops are ordinary; `Then`/`And` are the witness
/// composition operators (sequential ∘ / parallel ⊗) — keyboard-friendly
/// keywords chosen over `;`/`⊗`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    /// `w1 then w2` — sequential witness composition (∘, kernel `RealizesComp`).
    Then,
    /// `wf and wx` — parallel witness composition (⊗, kernel `RealizesTensor`).
    And,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `==`
    Eq,
    /// `!=`
    Ne,
}

impl BinOp {
    /// Whether this is a comparison, which types to `Bool` rather than to the
    /// type of its operands.
    pub const fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne
        )
    }
}

/// One arm of a `match`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

/// A match pattern (exhaustiveness is a provable coverage property at lowering).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// `_`
    Wildcard,
    /// A binding pattern, `k`.
    Var(String),
    /// A constructor pattern, `Succ(k)` or `Zero` (empty args).
    Ctor { name: String, args: Vec<Pattern> },
}
