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

use std::collections::{BTreeMap, BTreeSet};

use brix_ast::ast;
use brix_ast::Span;
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

/// Transaction semantics retained alongside the schema so native runtime
/// projection does not have to guess every relation is ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeRelationKind {
    Entity,
    Ground,
    State,
    Event,
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
    relation_kinds: BTreeMap<QualIdent, RuntimeRelationKind>,
    entities: std::collections::BTreeSet<QualIdent>,
    enums: BTreeMap<QualIdent, Vec<IrIdent>>,
    type_ns: BTreeMap<QualIdent, TypeNsEntry>,
    /// Unit name -> (dimension class, integer scale to its canonical minor
    /// unit). The scale lets `lower_measured` fold `150 EUR` to the integer
    /// 15000 (issue #47 Slice 1.5); most units are 1, `EUR` is 100 (cents).
    units: BTreeMap<String, (UnitClass, i64)>,
    /// `use p.{a, b}` — bare name -> fully qualified target.
    import_map: BTreeMap<String, QualIdent>,
    /// Bare names that were `use`-imported to two (or more) *different*
    /// qualified targets (issue #42 Slice 2: ambiguous cross-package
    /// imports — `use a.{Foo}` + `use b.{Foo}`). Populated by
    /// [`Self::with_import`] itself, so this is true the moment a second,
    /// conflicting `use` item is registered — it does not require a
    /// separate resolution pass over every reference to notice the hazard.
    ambiguous_imports: BTreeSet<String>,
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
        self.relation_kinds
            .entry(schema.name.clone())
            .or_insert(RuntimeRelationKind::Ground);
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
        self.relation_kinds
            .insert(name.clone(), RuntimeRelationKind::Entity);
        self.type_ns.insert(name.clone(), TypeNsEntry::Entity);
        self.entities.insert(name);
        self
    }

    pub fn with_relation_kind(mut self, name: QualIdent, kind: RuntimeRelationKind) -> Self {
        self.relation_kinds.insert(name, kind);
        self
    }

    pub fn relation_kind(&self, name: &QualIdent) -> RuntimeRelationKind {
        self.relation_kinds
            .get(name)
            .copied()
            .unwrap_or(RuntimeRelationKind::Ground)
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
        self.units.insert(name.into(), (class, 1));
        self
    }

    /// Register a unit with a non-unit integer scale to its canonical minor
    /// unit (e.g. `EUR` = 100 cents). See [`Self::unit_scale`].
    pub fn with_unit_scaled(
        mut self,
        name: impl Into<String>,
        class: UnitClass,
        scale: i64,
    ) -> Self {
        self.units.insert(name.into(), (class, scale));
        self
    }

    /// Register a `use`-imported bare name -> qualified target. If `bare`
    /// was already imported to a *different* target, this is a genuine
    /// ambiguity (design: "ambiguous import"): the conflict is recorded in
    /// [`Self::ambiguous_imports`] rather than silently letting the later
    /// `use` win (the map is still updated last-wins underneath, purely so
    /// `resolve_path` always has *some* qualified target to hand back —
    /// "never fails" — but callers must check [`Self::is_ambiguous_import`]
    /// before trusting that target).
    pub fn with_import(mut self, bare: impl Into<String>, target: QualIdent) -> Self {
        let bare = bare.into();
        if let Some(existing) = self.import_map.get(&bare) {
            if *existing != target {
                self.ambiguous_imports.insert(bare.clone());
            }
        }
        self.import_map.insert(bare, target);
        self
    }

    /// The qualified target `bare` currently maps to via `use`, if any
    /// (last-wins if ambiguous — see [`Self::is_ambiguous_import`]).
    pub fn imported_target(&self, bare: &str) -> Option<&QualIdent> {
        self.import_map.get(bare)
    }

    /// Whether `bare` was `use`-imported to two or more different qualified
    /// targets (issue #42 Slice 2).
    pub fn is_ambiguous_import(&self, bare: &str) -> bool {
        self.ambiguous_imports.contains(bare)
    }

    /// Every bare name that was `use`-imported ambiguously, in sorted
    /// (deterministic) order.
    pub fn ambiguous_imports(&self) -> &BTreeSet<String> {
        &self.ambiguous_imports
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
        self.units.get(name).map(|(class, _)| class)
    }

    /// The integer scale from a unit to its canonical minor unit (default 1;
    /// `EUR` = 100). `lower_measured` folds it into a `Measured` literal so
    /// `150 EUR` becomes the integer 15000 the runtime/oracle expect.
    pub fn unit_scale(&self, name: &str) -> i64 {
        self.units.get(name).map(|(_, scale)| *scale).unwrap_or(1)
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

    fn functions(&self, name: &QualIdent) -> &[FnSignature] {
        self.table.functions(name)
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

/// Lowering metadata that deliberately remains outside semantic Core IR:
/// source spans, fn-only information, and the monotonic `TyVar` supply.
#[derive(Default)]
pub struct LowerMeta {
    /// Overload list per function name (declaration order).
    fn_info: BTreeMap<QualIdent, Vec<FnInfo>>,
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

    pub fn set_fn_info(&mut self, name: QualIdent, info: FnInfo) {
        self.fn_info.entry(name).or_default().push(info);
    }

    /// First declared `FnInfo` for `name` (compat). Prefer [`fn_info_for_arity`].
    pub fn fn_info(&self, name: &QualIdent) -> Option<&FnInfo> {
        self.fn_info.get(name).and_then(|infos| infos.first())
    }

    /// Pick the overload whose parameter count matches `arity`.
    pub fn fn_info_for_arity(&self, name: &QualIdent, arity: usize) -> Option<&FnInfo> {
        self.fn_info
            .get(name)
            .and_then(|infos| infos.iter().find(|info| info.param_names.len() == arity))
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
        // Money's canonical minor unit is cents: `150 EUR` -> integer 15000.
        // This ×100 is a convention (money-in-minor-units, matching the oracle
        // `Value` docs and the transaction data), consolidated here as the one
        // source of truth for `lower_measured`; a spec-level minor-unit
        // declaration would ground it (issue #47 Slice 1.5).
        .with_unit_scaled("EUR", UnitClass::Money(IrIdent::new("EUR")), 100);

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
