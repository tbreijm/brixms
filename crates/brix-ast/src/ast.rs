//! Typed AST for the BrixMS surface language (spec Appendix D).
//!
//! Every node carries a [`Span`]. Nonterminals that Appendix D's own EBNF
//! leaves undefined (see this crate's delivery report / `spec/errata/`) are
//! represented by permissive, corpus-grounded shapes rather than guesses:
//! [`LooseBlock`]/[`LooseItem`] for item lists whose grammar Appendix D never
//! specifies (`PolicyItem`, `RecipeArgs`, `FeatureSetItem`, `DatasetItem`,
//! `StatModelItem`, `MlItem`, `ExperimentItem`, `VisualizationItem`), and
//! [`ExtensionDecl`] for whole top-level declaration keywords the spec body
//! uses (Parts 19, 21-27) that never made it into Appendix D's `Decl`
//! alternation at all (`logic`, `system dynamics`, `brick`, `workflow`, ...).

use crate::span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub text: String,
    pub span: Span,
}

/// A dotted path, `a.b.c` (Appendix D's `QualIdent`). A single segment is a
/// bare identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub segments: Vec<Ident>,
    pub span: Span,
}

impl Path {
    pub fn single(id: Ident) -> Self {
        Path {
            span: id.span,
            segments: vec![id],
        }
    }
}

// ---------------------------------------------------------------------
// File
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct File {
    pub span: Span,
    pub package: Option<PackageDecl>,
    pub module: Option<ModuleDecl>,
    pub uses: Vec<UseDecl>,
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone)]
pub struct PackageDecl {
    pub span: Span,
    pub name: Path,
    pub version: SemVer,
}

#[derive(Debug, Clone)]
pub struct ModuleDecl {
    pub span: Span,
    pub name: Ident,
}

#[derive(Debug, Clone)]
pub struct UseDecl {
    pub span: Span,
    pub path: Path,
    pub items: Vec<Ident>, // non-empty only for the `.{ a, b }` form
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub span: Span,
    pub text: String,
}

// ---------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Type {
    pub span: Span,
    pub kind: TypeKind,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    /// `Path<Arg, ...>` — a plain named type, optionally generic. Covers
    /// `String`, `Quantity<Mass>`, `Rel<{...}>`, `Result<T, E>`, ...
    Named { path: Path, args: Vec<TypeArg> },
    /// `{ f: T, g: U | rest }` row type.
    Row {
        fields: Vec<(Ident, Type)>,
        rest: Option<Box<Type>>,
    },
    /// `T / U` compound unit type, e.g. `Money<EUR> / Kilometre`.
    Div(Box<Type>, Box<Type>),
}

#[derive(Debug, Clone)]
pub enum TypeArg {
    Type(Type),
    /// A literal used in generic-argument position, e.g. `Net<"host">`.
    Lit(Expr),
}

// ---------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    Public(Option<RelVis>),
    #[default]
    Private,
}

impl Visibility {
    pub fn is_public(&self) -> bool {
        matches!(self, Visibility::Public(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelVis {
    Read,
    Write,
    Derive,
}

#[derive(Debug, Clone)]
pub enum Decl {
    Entity(EntityDecl),
    Rel(RelDecl),
    Derive(DeriveDecl),
    Constraint(ConstraintDecl),
    Query(QueryDecl),
    Protocol(ProtocolDecl),
    Driver(DriverDecl),
    Scenario(ScenarioDecl),
    Fn(FnDecl),
    Type(TypeDecl),
    Measure(MeasureDecl),
    Unit(UnitDecl),
    Enum(EnumDecl),
    Record(RecordDecl),
    DataRecipe(DataRecipeDecl),
    Feature(FeatureDecl),
    FeatureSet(FeatureSetDecl),
    Dataset(DatasetDecl),
    StatModel(StatModelDecl),
    MlWorkflow(MlWorkflowDecl),
    Experiment(ExperimentDecl),
    Visualization(VisualizationDecl),
    /// `trait Name<...> { type Assoc; fn method(...) -> T }` (Part V §3:
    /// "Traits provide constrained polymorphism with associated types;
    /// coherence per package graph; no inheritance"). Issue #111.
    Trait(TraitDecl),
    /// `impl Trait<...>? for Type { type Assoc = T; fn method(...) -> T { ... } }`
    /// (Part V §3 / §28.3 orphan rule). Issue #111.
    Impl(ImplDecl),
    /// Top-level `let name (: Type)? = expr` (Appendix D §4/§27.2 examples:
    /// `let n = count(from { ... })`, `let orders: Frame<{ ... }> = frame
    /// from { ... }`) — a local-binding form the spec's own body text uses
    /// at "program scope" in several places despite `Decl`'s Appendix D
    /// alternation never listing `let` (see errata). Distinct from
    /// `Decl::Extension` because it has real payload shape (a type
    /// annotation and a value expression), not just a structurally-parsed
    /// item list.
    Let(LetBindingDecl),
    /// Any top-level construct whose keyword Appendix D's `Decl`
    /// alternation doesn't list (see module docs) — parsed structurally
    /// but not semantically: `currency`, `phase`, `logic`,
    /// `system dynamics`, `hybrid simulation`, `decision`, `workflow`,
    /// `brick`, `export api`, `model contract`, `correction policy`,
    /// `interchange`, `consistency`, `factor`/`ordered factor`,
    /// `decision threshold`, top-level `policy`, `language task`, and a
    /// top-level `transaction ... isolation ... intent ...` form.
    Extension(ExtensionDecl),
    /// A `Decl` that failed to parse; the parser recovered by skipping to
    /// the next plausible declaration boundary. Carries the span and the
    /// verbatim source text it skipped so `fmt` can re-emit exactly that
    /// text (never a lossy comment) and diagnostics can point at it.
    Error(Span, String),
}

impl Decl {
    pub fn span(&self) -> Span {
        match self {
            Decl::Entity(d) => d.span,
            Decl::Rel(d) => d.span,
            Decl::Derive(d) => d.span,
            Decl::Constraint(d) => d.span,
            Decl::Query(d) => d.span,
            Decl::Protocol(d) => d.span,
            Decl::Driver(d) => d.span,
            Decl::Scenario(d) => d.span,
            Decl::Fn(d) => d.span,
            Decl::Type(d) => d.span,
            Decl::Measure(d) => d.span,
            Decl::Unit(d) => d.span,
            Decl::Enum(d) => d.span,
            Decl::Record(d) => d.span,
            Decl::DataRecipe(d) => d.span,
            Decl::Feature(d) => d.span,
            Decl::FeatureSet(d) => d.span,
            Decl::Dataset(d) => d.span,
            Decl::StatModel(d) => d.span,
            Decl::MlWorkflow(d) => d.span,
            Decl::Experiment(d) => d.span,
            Decl::Visualization(d) => d.span,
            Decl::Trait(d) => d.span,
            Decl::Impl(d) => d.span,
            Decl::Let(d) => d.span,
            Decl::Extension(d) => d.span,
            Decl::Error(s, _) => *s,
        }
    }

    pub fn vis(&self) -> Visibility {
        match self {
            Decl::Entity(d) => d.vis,
            Decl::Rel(d) => d.vis,
            Decl::Derive(d) => d.vis,
            Decl::Constraint(d) => d.vis,
            Decl::Query(d) => d.vis,
            Decl::Protocol(d) => d.vis,
            Decl::Driver(d) => d.vis,
            Decl::Scenario(d) => d.vis,
            Decl::Fn(d) => d.vis,
            Decl::Type(d) => d.vis,
            Decl::Measure(d) => d.vis,
            Decl::Unit(d) => d.vis,
            Decl::Enum(d) => d.vis,
            Decl::Record(d) => d.vis,
            Decl::DataRecipe(d) => d.vis,
            Decl::Feature(d) => d.vis,
            Decl::FeatureSet(d) => d.vis,
            Decl::Dataset(d) => d.vis,
            Decl::StatModel(d) => d.vis,
            Decl::MlWorkflow(d) => d.vis,
            Decl::Experiment(d) => d.vis,
            Decl::Visualization(d) => d.vis,
            Decl::Trait(d) => d.vis,
            Decl::Impl(d) => d.vis,
            Decl::Let(d) => d.vis,
            Decl::Extension(d) => d.vis,
            Decl::Error(_, _) => Visibility::Private,
        }
    }

    pub fn set_vis(&mut self, vis: Visibility, vis_span: Option<Span>) {
        let set_span = |span: &mut Span| {
            if let Some(vs) = vis_span {
                *span = vs.to(*span);
            }
        };
        match self {
            Decl::Entity(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Rel(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Derive(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Constraint(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Query(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Protocol(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Driver(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Scenario(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Fn(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Type(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Measure(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Unit(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Enum(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Record(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::DataRecipe(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Feature(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::FeatureSet(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Dataset(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::StatModel(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::MlWorkflow(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Experiment(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Visualization(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Trait(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Impl(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Let(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Extension(d) => {
                d.vis = vis;
                set_span(&mut d.span);
            }
            Decl::Error(_, _) => {}
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntityDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub span: Span,
    pub is_key: bool,
    pub name: Ident,
    pub ty: Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelKind {
    Ground,
    State,
    Event,
    Open,
}

#[derive(Debug, Clone)]
pub struct RelDecl {
    pub span: Span,
    pub vis: Visibility,
    pub kind: RelKind,
    pub name: Ident,
    pub roles: Vec<FieldDecl>,
    pub mods: Vec<RelMod>,
}

#[derive(Debug, Clone)]
pub enum RelMod {
    Key(Vec<Ident>),
    Unique(Vec<Ident>),
    Time(Ident),
    Index(Vec<Ident>),
    Partition(Vec<Ident>),
}

#[derive(Debug, Clone)]
pub struct DeriveDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub head: Head,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub enum Head {
    /// `QualIdent(ArgList)`
    Tuple { path: Path, args: Vec<Arg> },
    /// `Ident : Ident { ArgList } keyed by (IdentList)`
    Node {
        binder: Ident,
        ty: Ident,
        args: Vec<Arg>,
        keyed_by: Vec<Ident>,
    },
    /// `mask(Ident) by Ident`
    Mask { target: Ident, by: Ident },
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub span: Span,
    pub name: Option<Ident>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub span: Span,
    pub clauses: Vec<Clause>,
}

#[derive(Debug, Clone)]
pub enum Clause {
    Edge(EdgeClause),
    Entity(EntityClause),
    Let(LetClause),
    When(Expr),
    Any(Vec<Block>),
    Exists(Block),
    Without(Block),
    Optional(Block),
    History(EdgeClause),
    Path(PathClause),
    Cross(Block),
    /// A clause the parser could not parse; carries the verbatim skipped
    /// source text (see [`Decl::Error`]).
    Error(Span, String),
}

#[derive(Debug, Clone)]
pub struct EdgeClause {
    pub span: Span,
    pub alias: Option<Ident>,
    pub path: Path,
    pub args: Vec<Arg>,
}

#[derive(Debug, Clone)]
pub struct EntityClause {
    pub span: Span,
    pub binder: Ident,
    pub ty: Ident,
    pub fields: Vec<Arg>,
}

#[derive(Debug, Clone)]
pub struct LetClause {
    pub span: Span,
    pub pattern: Expr,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct PathClause {
    pub span: Span,
    pub expr: PathExpr,
    pub from: Ident,
    pub to: Ident,
}

#[derive(Debug, Clone)]
pub enum PathExpr {
    Step(PathStep),
    Alt(Vec<PathExpr>),
    Group(Box<PathExpr>),
    Repeat(Box<PathExpr>, Repeat),
}

#[derive(Debug, Clone)]
pub struct PathStep {
    pub span: Span,
    pub path: Path,
    pub from: Ident,
    pub to: Ident,
}

#[derive(Debug, Clone)]
pub enum Repeat {
    Plus,
    Star,
    Range(u64, Option<u64>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    Advisory,
    Strict,
    Audit,
}

#[derive(Debug, Clone)]
pub struct ConstraintDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub kind: ConstraintKind,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct QueryDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Type,
    pub from: Block,
    pub yield_: Expr,
    pub order: Option<OrderClause>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub span: Span,
    pub name: Ident,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct OrderClause {
    pub span: Span,
    pub by: Vec<Expr>,
    pub limit: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct ProtocolDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub request: RequestDecl,
    pub outcomes: Vec<OutcomeDecl>,
    pub policy: Option<LooseBlock>,
    /// Best-effort support for the estimator-protocol shape seen in Part
    /// 27.8 (`fit(...)`, `predict(...)`), which Appendix D's `ProtocolDecl`
    /// production does not describe at all (see errata).
    pub methods: Vec<FnSig>,
}

#[derive(Debug, Clone)]
pub struct RequestDecl {
    pub span: Span,
    pub roles: Vec<FieldDecl>,
    pub key: Vec<Ident>,
}

#[derive(Debug, Clone)]
pub struct OutcomeDecl {
    pub span: Span,
    pub name: Ident,
    pub roles: Vec<FieldDecl>,
}

#[derive(Debug, Clone)]
pub struct FnSig {
    pub span: Span,
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
}

#[derive(Debug, Clone)]
pub struct DriverDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub for_protocol: Ident,
    pub needs: Vec<CapRef>,
    pub req_param: Ident,
    pub cancel_param: Ident,
    pub body: FnBlock,
}

#[derive(Debug, Clone)]
pub struct CapRef {
    pub span: Span,
    pub name: Ident,
    pub args: Vec<TypeArg>,
}

#[derive(Debug, Clone)]
pub struct ScenarioDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub seed: SeedDecl,
    pub binds: Vec<BindDecl>,
    pub setup: Option<TxBlock>,
    pub steps: Vec<StepDecl>,
    pub ats: Vec<AtDecl>,
    pub asserts: Vec<AssertDecl>,
}

#[derive(Debug, Clone)]
pub enum SeedDecl {
    Nat(u64, Span),
    Each(Expr, Span),
}

#[derive(Debug, Clone)]
pub struct BindDecl {
    pub span: Span,
    pub protocol: Path,
    pub args: Vec<Arg>,
    pub to: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct StepDecl {
    pub span: Span,
    pub every: Expr,
    pub for_: Expr,
    pub body: TxBlock,
}

#[derive(Debug, Clone)]
pub struct AtDecl {
    pub span: Span,
    pub at: Expr,
    pub body: TxBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssertMode {
    Always,
    Eventually,
    AtEnd,
}

#[derive(Debug, Clone)]
pub struct AssertDecl {
    pub span: Span,
    pub mode: AssertMode,
    pub cond: Expr,
}

#[derive(Debug, Clone)]
pub struct TxBlock {
    pub span: Span,
    pub stmts: Vec<TxStmt>,
}

#[derive(Debug, Clone)]
pub enum TxStmt {
    Let {
        pattern: Expr,
        value: TxExpr,
    },
    Expr(TxExpr),
    /// A statement the parser could not parse; carries the verbatim skipped
    /// source text (see [`Decl::Error`]).
    Error(Span, String),
}

#[derive(Debug, Clone)]
pub enum TxExpr {
    Ensure {
        ty: Ident,
        args: Vec<Arg>,
        span: Span,
    },
    Fresh {
        ty: Ident,
        args: Vec<Arg>,
        span: Span,
    },
    AssertTuple {
        path: Path,
        args: Vec<Arg>,
        span: Span,
    },
    AssertStruct {
        ty: Ident,
        args: Vec<Arg>,
        span: Span,
    },
    Set {
        path: Path,
        args: Vec<Arg>,
        span: Span,
    },
    Retract {
        expr: Expr,
        span: Span,
    },
    Supersede {
        new: Expr,
        old: Expr,
        span: Span,
    },
}

impl TxExpr {
    pub fn span(&self) -> Span {
        match self {
            TxExpr::Ensure { span, .. }
            | TxExpr::Fresh { span, .. }
            | TxExpr::AssertTuple { span, .. }
            | TxExpr::AssertStruct { span, .. }
            | TxExpr::Set { span, .. }
            | TxExpr::Retract { span, .. }
            | TxExpr::Supersede { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FnBlock {
    pub span: Span,
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        pattern: Expr,
        value: Expr,
        span: Span,
    },
    Expr(Expr),
    /// A statement the parser could not parse; carries the verbatim skipped
    /// source text (see [`Decl::Error`]).
    Error(Span, String),
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub span: Span,
    pub name: Ident,
    pub bound: Option<Type>,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub span: Span,
    pub vis: Visibility,
    pub partial: bool,
    pub aggregate: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret: Type,
    pub effects: Option<Vec<Ident>>,
    pub body: Option<FnBody>,
}

#[derive(Debug, Clone)]
pub enum FnBody {
    Expr(Expr),
    Block(FnBlock),
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub value: Type,
}

/// `trait Name<Generics> { type Assoc; fn method(params) -> Ret }` — Part V §3.
/// Method entries reuse [`FnDecl`]; a signature-only method has `body: None`,
/// a default method carries a body. Issue #111.
#[derive(Debug, Clone)]
pub struct TraitDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub assoc_types: Vec<AssocTypeDecl>,
    pub methods: Vec<FnDecl>,
}

/// An associated-type *declaration* inside a trait: `type Item`.
#[derive(Debug, Clone)]
pub struct AssocTypeDecl {
    pub span: Span,
    pub name: Ident,
}

/// `impl Trait<Args>? for Type { type Assoc = T; fn method(params) -> Ret { .. } }`
/// — Part V §3 / §28.3 orphan rule. Issue #111.
#[derive(Debug, Clone)]
pub struct ImplDecl {
    pub span: Span,
    pub vis: Visibility,
    pub trait_name: Ident,
    pub trait_args: Vec<TypeArg>,
    /// The head type the trait is implemented for (`for Type`).
    pub target: Type,
    pub assoc_bindings: Vec<AssocTypeBinding>,
    pub methods: Vec<FnDecl>,
}

/// An associated-type *binding* inside an impl: `type Item = String`.
#[derive(Debug, Clone)]
pub struct AssocTypeBinding {
    pub span: Span,
    pub name: Ident,
    pub value: Type,
}

#[derive(Debug, Clone)]
pub struct MeasureDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
}

#[derive(Debug, Clone)]
pub struct UnitDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub measure: Ident,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub span: Span,
    pub name: Ident,
    pub payload: VariantPayload,
}

#[derive(Debug, Clone)]
pub enum VariantPayload {
    Unit,
    Tuple(Vec<Type>),
    Struct(Vec<FieldDecl>),
}

#[derive(Debug, Clone)]
pub struct RecordDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone)]
pub struct DataRecipeDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub items: Vec<RecipeItem>,
}

#[derive(Debug, Clone)]
pub enum RecipeItem {
    Input(Type),
    Output(Type),
    Step { name: Ident, rest: LooseItem },
    Quarantine(Expr),
}

#[derive(Debug, Clone)]
pub struct FeatureDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: FeatureBody,
}

#[derive(Debug, Clone)]
pub enum FeatureBody {
    Expr(Expr),
    Items(Vec<FeatureItem>),
}

#[derive(Debug, Clone)]
pub enum FeatureItem {
    ObservationTime(Expr),
    Window(Expr),
    Source(Path),
    Leakage(Ident),
    Missing(Ident),
}

#[derive(Debug, Clone)]
pub struct FeatureSetDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub version: Option<Expr>,
    pub items: LooseBlock,
}

#[derive(Debug, Clone)]
pub struct DatasetDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub items: LooseBlock,
}

#[derive(Debug, Clone)]
pub struct StatModelDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub items: LooseBlock,
}

#[derive(Debug, Clone)]
pub struct MlWorkflowDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub items: LooseBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentKind {
    Experiment,
    Tuning,
}

#[derive(Debug, Clone)]
pub struct ExperimentDecl {
    pub span: Span,
    pub vis: Visibility,
    pub kind: ExperimentKind,
    pub name: Ident,
    pub items: LooseBlock,
}

#[derive(Debug, Clone)]
pub struct VisualizationDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub items: LooseBlock,
}

#[derive(Debug, Clone)]
pub struct LetBindingDecl {
    pub span: Span,
    pub vis: Visibility,
    pub name: Ident,
    pub ty: Option<Type>,
    pub value: Expr,
}

/// A whole top-level construct outside Appendix D's `Decl` alternation
/// (see module docs and `spec/errata/`).
#[derive(Debug, Clone)]
pub struct ExtensionDecl {
    pub span: Span,
    pub vis: Visibility,
    pub keywords: Vec<Ident>,
    pub name: Option<Ident>,
    pub version: Option<Expr>,
    pub body: Option<LooseBlock>,
}

/// A generic, structurally-parsed item list used everywhere Appendix D
/// names an item nonterminal (`PolicyItem`, `DatasetItem`, `MlItem`, ...)
/// without ever defining it. See module docs.
#[derive(Debug, Clone)]
pub struct LooseBlock {
    pub span: Span,
    pub items: Vec<LooseItem>,
}

#[derive(Debug, Clone)]
pub struct LooseItem {
    pub span: Span,
    pub parts: Vec<LoosePart>,
}

#[derive(Debug, Clone)]
pub enum LoosePart {
    Expr(Expr),
    Block(LooseBlock),
    /// `name: value` recognized as a real pair (rather than falling through
    /// to an expression-level error on the `:`) — the common shape of the
    /// undefined item nonterminals' entries (see module docs).
    Pair {
        name: Ident,
        value: Expr,
    },
    /// `name = value` — a bare loose-item binding (e.g. `system dynamics`'s
    /// `auxiliary utilization = a.value / b.value`).
    Assign {
        name: Ident,
        value: Expr,
    },
    /// `name: type = value` — a typed loose-item binding (e.g. `system
    /// dynamics`'s `stock AvailableVehicles: Quantity<VehicleCount> = 100
    /// vehicles`). `ty` is the parsed left-of-`=` expression (which already
    /// round-trips generic instantiations like `Quantity<VehicleCount>` via
    /// [`ExprKind::Generic`]), not re-parsed as a [`Type`].
    TypedAssign {
        name: Ident,
        ty: Expr,
        value: Expr,
    },
    /// `name -> Type from { PatternClause* }` — an inline query-shaped
    /// loose item (e.g. a `policy` item's `candidates -> Rel<{ vehicle:
    /// Vehicle }> from { AssignmentCandidate(order, vehicle) }`), mirroring
    /// `QueryDecl`'s own `-> Type from { ... }` shape without the surrounding
    /// `query name(params) = ... yield ...` wrapper.
    Query {
        name: Ident,
        ret: Type,
        from: Block,
    },
}

// ---------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Expr {
    pub span: Span,
    pub kind: Box<ExprKind>,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i128),
    Float(f64),
    Str(String),
    Bool(bool),
    /// `<number> <unit>` (Appendix D lexical: quantity/money/duration
    /// literals, unified here since the parser doesn't classify which of
    /// the three a unit name denotes — that's a symbol-table concern).
    Measured {
        value: Box<Expr>,
        unit: Ident,
    },
    Ident(Path),
    Unary {
        op: UnOp,
        expr: Expr,
    },
    Binary {
        op: BinOp,
        lhs: Expr,
        rhs: Expr,
    },
    Range {
        lo: Option<Expr>,
        hi: Option<Expr>,
    },
    Call {
        callee: Expr,
        args: Vec<Arg>,
    },
    /// `Path? { field: expr, ... }` — struct/outcome literal. `path` is
    /// `None` for the anonymous form (`fail { ... }`, TxExpr bodies).
    StructLit {
        path: Option<Path>,
        fields: Vec<Arg>,
    },
    Field {
        base: Expr,
        name: Ident,
    },
    Try(Expr),
    If {
        cond: Expr,
        then: IfBody,
        else_: Option<Expr>,
    },
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
    },
    Closure {
        params: Vec<Ident>,
        body: Expr,
    },
    /// `succeed`/`fail` (Driver body vocabulary; not in Appendix D, see
    /// errata).
    Succeed {
        path: Option<Path>,
        args: Vec<Arg>,
    },
    Fail {
        path: Option<Path>,
        args: Vec<Arg>,
    },
    Paren(Expr),
    Block(FnBlock),
    /// `Base { (when Expr | otherwise) => Expr, ... }` — the `sim.script`
    /// adapter-script mini-language (Part VI §2: "explicit adapters,
    /// explicit lifecycle meaning"), e.g. `bind X to sim.script { when
    /// req.weight <= 2000 kg => Chosen { ... } otherwise => NoCapacity {}
    /// }`. Not in Appendix D's `Expr` production (see errata); shaped like
    /// `Match` but keyed by a `when`-guard/`otherwise` instead of a
    /// pattern, since there's no scrutinee to match against.
    AdapterScript {
        base: Expr,
        arms: Vec<ScriptArm>,
    },
    /// `Base@version` — an inline version tag on a name in loose-item
    /// position (e.g. `schema LogisticsProjection@3`). `version` is the
    /// verbatim semver-shaped run (see `Parser::semver`), not further
    /// parsed.
    Versioned {
        base: Expr,
        version: String,
    },
    /// `Base<TypeArg, ...>` — a generic instantiation appearing in
    /// expression/value position (e.g. `weight: Quantity<Mass>` inside a
    /// loose row-type-shaped item). Disambiguated from a `<`/`>` comparison
    /// chain by lookahead in `postfix` (see `Parser::generic_args_end`);
    /// not in Appendix D's `Expr` production, but needed so field-typed
    /// loose items and top-level `let x: T<U> = ...` forms round-trip
    /// instead of misparsing as chained comparisons.
    Generic {
        base: Expr,
        args: Vec<TypeArg>,
    },
    /// `from { PatternClause* } (yield Expr)?` used as an EXPRESSION (e.g.
    /// `count(from { Move(vehicle: v) })`) — Appendix D §4's relation
    /// comprehension, reusing the same pattern-clause grammar as
    /// `QueryDecl.from` (not in Appendix D's `Expr` production at all; see
    /// errata).
    From {
        block: Box<Block>,
        yield_: Option<Expr>,
    },
    /// `...` used in expression position — Appendix D's own prose uses it as
    /// a placeholder for omitted detail (not a grammar production). See
    /// module docs; formatted back verbatim so it round-trips.
    Ellipsis,
    /// A span of tokens the parser could not parse as an expression. Carries
    /// the exact source text of the tokens it skipped (captured at the skip
    /// site) so `fmt` can re-emit it verbatim instead of a lossy comment —
    /// the empty string here (the zero-token case in `atom()`) is itself a
    /// fixpoint.
    Error(String),
}

#[derive(Debug, Clone)]
pub enum IfBody {
    Then(Expr),
    Block(FnBlock),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub span: Span,
    pub pattern: Expr,
    pub body: Expr,
}

/// One arm of an [`ExprKind::AdapterScript`]. `when: None` is the
/// `otherwise => Expr` catch-all arm.
#[derive(Debug, Clone)]
pub struct ScriptArm {
    pub span: Span,
    pub when: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    Add,
    Sub,
    Mul,
    Div,
    Pipe,
    Tilde,
    Colon,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Or => "or",
            BinOp::And => "and",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::In => "in",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pipe => "|>",
            BinOp::Tilde => "~",
            BinOp::Colon => ":",
        }
    }
}
