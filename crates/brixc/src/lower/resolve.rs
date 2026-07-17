//! The resolver ([`ProgramResolver`]), its side tables ([`LowerMeta`]), the
//! PRELUDE-STUB, and name resolution helpers (import map, variant lookup).
//!
//! Two namespaces exist during lowering (design §"Name/variable
//! resolution"): (1) the **decl namespace** — relations, fns, types,
//! enums, units, populated by pass 1 (schema.rs) plus imports plus the
//! PRELUDE, all owned by [`ProgramResolver`]; (2) the **pattern-variable**
//! namespace, one flat scope per rule/constraint/query body, which is just
//! a `BTreeSet<Ident>` threaded through `decl.rs`/`expr.rs` (no dedicated
//! type — brix-ir's [`brix_ir::pattern::Pattern::bound_vars`] is the
//! IR-side authority on what a body actually exports; this crate's
//! in-progress set only needs to answer "have I seen this name yet" while
//! walking the AST left to right).

use std::collections::BTreeMap;

use brix_ast::ast;
use brix_ast::Span;
use brix_ir::core::Expr as IrExpr;
use brix_ir::effects::EffectRow;
use brix_ir::frontend::{FnSignature, RelationSchema, SchemaResolver, TableResolver};
use brix_ir::ident::{Ident as IrIdent, QualIdent};
use brix_ir::types::{IntWidth, Ty, TyVar};

/// What a declared type-position name (from `Decl::Entity`/`Decl::Enum`/
/// `Decl::Type`) means when `tymap` looks it up. Kept internal to the
/// resolver; `tymap.rs` only calls the accessor methods below.
#[derive(Clone, Debug)]
enum TypeNsEntry {
    Entity,
    Enum,
    /// A `type` alias, 1-level expanded at pass-1 build time (design
    /// §"Pass 1": "type alias table (1-level expand, cycle=error)").
    Alias(Ty),
}

/// How a PRELUDE-seeded (or, in a fuller build, package-declared) unit name
/// classifies a `Measured` literal (design §"PRELUDE-STUB").
#[derive(Clone, Debug)]
pub enum UnitClass {
    /// A `Quantity<M>` unit; the `Ident` is the measure name (`Mass`,
    /// `Kilometre`, ...).
    Quantity(IrIdent),
    Duration,
    /// A `Money<C>` unit; the `Ident` is the currency code.
    Money(IrIdent),
}

/// Which enum (if any) declares a given bare variant name, for the
/// unqualified-variant case (design: "In general expr pos, unqualified
/// variant must be unique across enums in scope else error").
pub enum VariantLookup {
    None,
    Unique(QualIdent, u32),
    Ambiguous,
}

/// The whole-program decl-namespace resolver: wraps an `ir::TableResolver`
/// (relations/fns/completeness witnesses — the `SchemaResolver` brix-ir
/// itself needs) plus the extra decl-namespace tables lowering needs that
/// brix-ir's checker does not (entities, enums, aliases, units, imports).
/// Built once by pass 1 ([`crate::lower::schema`]) then read-only through
/// pass 2 and the final checks.
#[derive(Default)]
pub struct ProgramResolver {
    table: TableResolver,
    /// Every relation schema registered so far, keyed for dedup/replace-in-
    /// place the same way `TableResolver` does internally. `TableResolver`
    /// does not expose iteration (it only needs point lookups for
    /// `SchemaResolver`), and `check_relation_keys` needs to run over
    /// *every* schema (design: "`check_relation_keys` (every schema)") —
    /// this is the enumerable mirror that makes that possible without
    /// widening brix-ir's public surface beyond mismatches (A)/(B).
    relations_by_name: BTreeMap<QualIdent, RelationSchema>,
    entities: std::collections::BTreeSet<QualIdent>,
    enums: BTreeMap<QualIdent, Vec<IrIdent>>,
    type_ns: BTreeMap<QualIdent, TypeNsEntry>,
    units: BTreeMap<String, UnitClass>,
    /// `use p.{a, b}` — bare name -> fully qualified target.
    import_map: BTreeMap<String, QualIdent>,
    /// `use p.q` (no `.{...}` items) — bare first-segment alias -> its
    /// qualified prefix (design: "`use brix.sim` -> prefix sim.X ->
    /// brix.sim.X").
    prefix_map: BTreeMap<String, QualIdent>,
}

impl ProgramResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_relation(mut self, schema: RelationSchema) -> Self {
        self.relations_by_name
            .insert(schema.name.clone(), schema.clone());
        self.table = self.table.with_relation(schema);
        self
    }

    /// Every registered relation schema, in canonical (`QualIdent`) order —
    /// what `check_relation_keys` needs to run over "every schema".
    pub fn relations(&self) -> impl Iterator<Item = &RelationSchema> {
        self.relations_by_name.values()
    }

    pub fn with_function(mut self, sig: FnSignature) -> Self {
        self.table = self.table.with_function(sig);
        self
    }

    pub fn with_witness(mut self, relation: QualIdent) -> Self {
        self.table = self.table.with_witness(relation);
        self
    }

    pub fn with_entity(mut self, name: QualIdent) -> Self {
        self.type_ns.insert(name.clone(), TypeNsEntry::Entity);
        self.entities.insert(name);
        self
    }

    pub fn with_enum(mut self, name: QualIdent, variants: Vec<IrIdent>) -> Self {
        self.type_ns.insert(name.clone(), TypeNsEntry::Enum);
        self.enums.insert(name, variants);
        self
    }

    pub fn with_alias(mut self, name: QualIdent, ty: Ty) -> Self {
        self.type_ns.insert(name, TypeNsEntry::Alias(ty));
        self
    }

    pub fn with_unit(mut self, name: impl Into<String>, class: UnitClass) -> Self {
        self.units.insert(name.into(), class);
        self
    }

    pub fn with_import(mut self, bare: impl Into<String>, target: QualIdent) -> Self {
        self.import_map.insert(bare.into(), target);
        self
    }

    pub fn with_prefix(mut self, bare: impl Into<String>, target: QualIdent) -> Self {
        self.prefix_map.insert(bare.into(), target);
        self
    }

    pub fn is_entity(&self, name: &QualIdent) -> bool {
        matches!(self.type_ns.get(name), Some(TypeNsEntry::Entity))
    }

    pub fn is_enum(&self, name: &QualIdent) -> bool {
        matches!(self.type_ns.get(name), Some(TypeNsEntry::Enum))
    }

    pub fn alias_ty(&self, name: &QualIdent) -> Option<&Ty> {
        match self.type_ns.get(name) {
            Some(TypeNsEntry::Alias(ty)) => Some(ty),
            _ => None,
        }
    }

    pub fn unit_class(&self, name: &str) -> Option<&UnitClass> {
        self.units.get(name)
    }

    pub fn enum_variants(&self, name: &QualIdent) -> Option<&[IrIdent]> {
        self.enums.get(name).map(|v| v.as_slice())
    }

    /// Ordinal of `variant` within `enum_name`'s declaration-order variant
    /// list (mismatch B's encoding), when `enum_name` is a known enum.
    pub fn variant_ordinal(&self, enum_name: &QualIdent, variant: &str) -> Option<u32> {
        self.enums
            .get(enum_name)
            .and_then(|vs| vs.iter().position(|v| v.as_str() == variant))
            .map(|i| i as u32)
    }

    /// Resolve a bare variant name across every enum currently in scope
    /// (design: "unqualified variant must be unique across enums in scope
    /// else error demanding qualification").
    pub fn find_unique_variant(&self, variant: &str) -> VariantLookup {
        let mut hits: Vec<(&QualIdent, u32)> = Vec::new();
        for (enum_name, variants) in &self.enums {
            if let Some(ord) = variants.iter().position(|v| v.as_str() == variant) {
                hits.push((enum_name, ord as u32));
            }
        }
        match hits.len() {
            0 => VariantLookup::None,
            1 => VariantLookup::Unique(hits[0].0.clone(), hits[0].1),
            _ => VariantLookup::Ambiguous,
        }
    }

    /// Resolve a surface `ast::Path` to a `QualIdent`, applying the v0
    /// import/prefix map (design §"Qualified names resolve": "(1)
    /// protocol-synth relations, (2) import map, (3) prelude" — protocol-
    /// synth relations need no special case here because they were
    /// registered under exactly the segment sequence the surface syntax
    /// spells, e.g. `AssignOrder.Chosen`, so a literal, unresolved path
    /// already matches them byte-for-byte; only bare single names and
    /// `use`d prefixes need substitution). Never fails — an unresolvable
    /// name still yields a `QualIdent` (division of labor: existence is
    /// `check_rule`'s job for relations, and lowering's own diagnostics for
    /// values).
    pub fn resolve_path(&self, path: &ast::Path) -> QualIdent {
        let segs: Vec<&str> = path.segments.iter().map(|s| s.text.as_str()).collect();
        // `ast::Path` is built from `Parser::qual_ident`/`Path::single`,
        // which always emit at least one segment — but `resolve_path` is a
        // public seam, so degrade to an (unreachable in practice) sentinel
        // name rather than panic if that invariant is ever violated
        // (totality: never panic on any `ast::Path`, however constructed).
        let Some(&first) = segs.first() else {
            return QualIdent::simple("%empty-path");
        };
        if segs.len() == 1 {
            if let Some(q) = self.import_map.get(first) {
                return q.clone();
            }
            return QualIdent::simple(first);
        }
        if let Some(prefix) = self.prefix_map.get(first) {
            let mut full: Vec<IrIdent> = prefix.segments().to_vec();
            full.extend(segs[1..].iter().map(|s| IrIdent::new(*s)));
            return QualIdent::from_segments(full);
        }
        QualIdent::from_segments(segs.iter().map(|s| IrIdent::new(*s)))
    }
}

impl SchemaResolver for ProgramResolver {
    fn relation(&self, name: &QualIdent) -> Option<&RelationSchema> {
        self.table.relation(name)
    }

    fn function(&self, name: &QualIdent) -> Option<&FnSignature> {
        self.table.function(name)
    }

    fn has_completeness_witness(&self, relation: &QualIdent) -> bool {
        self.table.has_completeness_witness(relation)
    }
}

/// What lowering keeps about a declared fn beyond its `ir::FnSignature`:
/// parameter *names* (for named-argument call reordering — brix-ir's
/// `FnSignature` only has positional `Ty`s) and the surface body, which has
/// "no home" in Core IR yet (design: fn bodies are never lowered in v0) but
/// is kept for a future Ring 1 lane rather than discarded.
#[derive(Clone)]
pub struct FnInfo {
    pub param_names: Vec<IrIdent>,
    pub is_partial: bool,
    pub is_aggregate: bool,
    pub body: Option<ast::FnBody>,
}

/// The v0 side tables for the impedance mismatches that don't yet have an
/// IR-shape fix (design §"Impedance mismatches", (C) and (E); (D) needs no
/// table — the record bridge's field order is recoverable from the
/// enclosing `Expr::ty`, see `expr.rs`). Also carries the span map Core IR
/// itself has no room for, and the monotonic `TyVar` supply.
#[derive(Default)]
pub struct LowerMeta {
    /// (C) `pattern::Clause::When`/`Clause::Let` carry no expr payload.
    /// Keyed by (decl name, DFS clause-visit ordinal — see
    /// `decl::ClauseCounter`), since one decl's body may contain several
    /// `when`/`let` clauses, including nested ones.
    clause_exprs: BTreeMap<(IrIdent, u32), IrExpr>,
    /// (E) `core::Query` carries no `params`.
    query_params: BTreeMap<IrIdent, Vec<(IrIdent, Ty)>>,
    fn_info: BTreeMap<QualIdent, FnInfo>,
    decl_spans: BTreeMap<IrIdent, Span>,
    relation_decl_spans: BTreeMap<QualIdent, Span>,
    role_spans: BTreeMap<(QualIdent, IrIdent), Span>,
    next_tyvar: u32,
}

impl LowerMeta {
    pub fn fresh_tyvar(&mut self) -> TyVar {
        let v = self.next_tyvar;
        self.next_tyvar += 1;
        TyVar(v)
    }

    pub fn set_decl_span(&mut self, name: IrIdent, span: Span) {
        self.decl_spans.insert(name, span);
    }

    pub fn decl_span(&self, name: &IrIdent) -> Option<Span> {
        self.decl_spans.get(name).copied()
    }

    pub fn set_relation_span(&mut self, name: QualIdent, span: Span) {
        self.relation_decl_spans.insert(name, span);
    }

    pub fn decl_span_by_qual(&self, name: &QualIdent) -> Option<Span> {
        self.relation_decl_spans.get(name).copied()
    }

    pub fn set_role_span(&mut self, relation: QualIdent, role: IrIdent, span: Span) {
        self.role_spans.insert((relation, role), span);
    }

    pub fn role_span(&self, relation: &QualIdent, role: &IrIdent) -> Option<Span> {
        self.role_spans
            .get(&(relation.clone(), role.clone()))
            .copied()
    }

    pub fn set_clause_expr(&mut self, decl: IrIdent, ordinal: u32, expr: IrExpr) {
        self.clause_exprs.insert((decl, ordinal), expr);
    }

    pub fn clause_expr(&self, decl: &IrIdent, ordinal: u32) -> Option<&IrExpr> {
        self.clause_exprs.get(&(decl.clone(), ordinal))
    }

    pub fn set_query_params(&mut self, name: IrIdent, params: Vec<(IrIdent, Ty)>) {
        self.query_params.insert(name, params);
    }

    pub fn query_params(&self, name: &IrIdent) -> Option<&[(IrIdent, Ty)]> {
        self.query_params.get(name).map(|v| v.as_slice())
    }

    pub fn set_fn_info(&mut self, name: QualIdent, info: FnInfo) {
        self.fn_info.insert(name, info);
    }

    pub fn fn_info(&self, name: &QualIdent) -> Option<&FnInfo> {
        self.fn_info.get(name)
    }
}

/// PRELUDE-STUB (design §"Pass 1 — schema build"): a hand-seeded stand-in
/// for real package resolution (brixpkg does not expose a symbol table to
/// this lane yet). Seeds exactly the surface the spec corpus's `use
/// brix.*` imports pull in: math/time units, `EUR` money, `brix.math.clamp`,
/// `sim.Now`, `brix.ops.*` operator signatures, and `count` as an aggregate
/// fn. Stopgap until real package resolution lands (see design ruling).
pub fn seed_prelude(resolver: ProgramResolver) -> ProgramResolver {
    let mut r = resolver
        .with_unit("kg", UnitClass::Quantity(IrIdent::new("Mass")))
        .with_unit("km", UnitClass::Quantity(IrIdent::new("Kilometre")))
        .with_unit("hours", UnitClass::Duration)
        .with_unit("s", UnitClass::Duration)
        .with_unit("EUR", UnitClass::Money(IrIdent::new("EUR")));

    r = r.with_function(FnSignature {
        name: QualIdent::from("brix.math.clamp"),
        params: vec![Ty::F64, Ty::F64, Ty::F64],
        ret: Ty::F64,
        effects: EffectRow::empty(),
        is_aggregate: false,
        may_diverge: false,
    });

    r = r.with_relation(RelationSchema {
        name: QualIdent::from("brix.sim.Now"),
        roles: vec![(IrIdent::new("at"), Ty::Instant)],
        key: vec![],
        model_closed: false,
        derived: false,
    });

    for (name, arity) in [
        ("or", 2),
        ("and", 2),
        ("not", 1),
        ("eq", 2),
        ("ne", 2),
        ("lt", 2),
        ("le", 2),
        ("gt", 2),
        ("ge", 2),
        ("in", 2),
        ("add", 2),
        ("sub", 2),
        ("mul", 2),
        ("div", 2),
        ("neg", 1),
    ] {
        r = r.with_function(FnSignature {
            name: QualIdent::from(format!("brix.ops.{name}").as_str()),
            // The op signatures are ad hoc polymorphic (`+` closes over
            // every numeric/quantity type); a real fn-sig table would carry
            // a trait bound, not a monomorphic `Ty`. `Ty::Var(TyVar(0))` is
            // a placeholder that is never actually unified against (v0 has
            // no unifier) — only `effects`/`is_aggregate` are consulted by
            // lowering (effect-row union) and brix-ir (nothing, today).
            params: (0..arity).map(|_| Ty::Var(TyVar(0))).collect(),
            ret: Ty::Var(TyVar(0)),
            effects: EffectRow::empty(),
            is_aggregate: false,
            may_diverge: false,
        });
    }

    r.with_function(FnSignature {
        name: QualIdent::simple("count"),
        params: vec![],
        ret: Ty::Int(IntWidth::I64),
        effects: EffectRow::empty(),
        is_aggregate: true,
        may_diverge: false,
    })
}
