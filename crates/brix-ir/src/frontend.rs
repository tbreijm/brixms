//! The AST-facing interface — the seam brix-ast (and the schema/name-resolution
//! lane) must satisfy so integration is a thin adapter, not a rewrite.
//!
//! brix-ast is being built in a sibling lane and is not on this branch yet
//! (per the task note). Rather than block, brix-ir defines here the *input*
//! it needs, as traits plus a small owned data type. When the AST lands, a
//! lowering adapter implements [`FrontendSource`] over the real AST and hands
//! brix-ir what it already knows how to check. Nothing in this module depends
//! on the AST's concrete shape.
//!
//! Design rule for this seam: brix-ir consumes **names and structure**, and
//! asks the frontend to resolve **schema facts** (a relation's declared role
//! types, whether a relation is model-closed, a function's declared effect
//! row). brix-ir never parses text and never invents schema — that is the
//! frontend's job. Everything returned is owned/clonable so brix-ir's IR does
//! not borrow from the AST arena.

use crate::core::{Constraint, FnDef, Query, Rule};
use crate::effects::EffectRow;
use crate::ident::{Ident, QualIdent};
use crate::types::Ty;

/// What brix-ir needs to know about a relation to check patterns and keys.
/// The frontend/schema lane owns the schema graph and answers these.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RelationSchema {
    pub name: QualIdent,
    /// Declared roles and their types, in declaration order (role *order* is
    /// non-semantic — Part IV §1 — but declaration order is what App. G
    /// "entity keys ... in declaration order" needs).
    pub roles: Vec<(Ident, Ty)>,
    /// Key role names (the `key(...)` modifier). Every key role's type must
    /// pass [`crate::types::check_key_canonical`].
    pub key: Vec<Ident>,
    /// Whether the relation is model-closed (Part III §7). `open` relations
    /// need a `Complete` witness before an absence-sensitive read compiles.
    pub model_closed: bool,
    /// Whether this relation is graph-derived (has `derive` producers) — an
    /// ordinary fn may not consume a derived `Rel` inside a rule (Part IV §4).
    pub derived: bool,
}

/// What brix-ir needs to know about a called function.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FnSignature {
    pub name: QualIdent,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub effects: EffectRow,
    /// Whether this is an `aggregate fn` (Part IV §4): consuming a graph-derived
    /// `Rel` from a rule creates a strict phase dependency.
    pub is_aggregate: bool,
    /// Whether this fn may `diverge` (banned in rule bodies — Part V §5).
    pub may_diverge: bool,
}

/// The resolved-schema view the frontend presents to brix-ir. This is the
/// **environment** `Σ` (relation schemas) and part of `Γ` (function sigs) from
/// Appendix E, populated by name resolution.
///
/// It is a trait so the eventual AST lowering can implement it lazily over the
/// real schema graph, while tests implement it over a hand-built table.
pub trait SchemaResolver {
    /// Look up a relation's schema by qualified name. `None` = unresolved name
    /// (a name-resolution error the frontend should already have reported;
    /// brix-ir treats it as "cannot check" rather than crashing).
    fn relation(&self, name: &QualIdent) -> Option<&RelationSchema>;

    /// All declared signatures for `name` (typed overloads), in declaration
    /// order. Empty slice = unresolved. Call sites must select by arity +
    /// argument types; see [`crate::infer`].
    fn functions(&self, name: &QualIdent) -> &[FnSignature];

    /// First declared signature for `name` (declaration order). Prefer
    /// [`SchemaResolver::functions`] at typed call sites.
    fn function(&self, name: &QualIdent) -> Option<&FnSignature> {
        self.functions(name).first()
    }

    /// Whether a `Complete(relation, partition, ...)` witness is in scope for
    /// an absence-sensitive read (Part III §7). Used to admit `without` /
    /// `optional` over `open` relations.
    fn has_completeness_witness(&self, relation: &QualIdent) -> bool;
}

/// The whole-program input brix-ir lowers and checks: the declaration nodes
/// already in Core IR form. The lowering adapter (future) produces this from
/// the AST; brix-ir's checker consumes it against a [`SchemaResolver`].
///
/// Kept as an owned struct (not a trait) because it *is* the IR — the trait
/// boundary is only where brix-ir must call back into frontend-owned schema.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FrontendSource {
    pub rules: Vec<Rule>,
    pub constraints: Vec<Constraint>,
    pub queries: Vec<Query>,
    /// User-defined functions with lowered bodies (issue #47). Empty for
    /// programs whose functions are all builtins or (pre-#47) hand-registered.
    pub functions: Vec<FnDef>,
}

impl FrontendSource {
    pub fn new() -> Self {
        FrontendSource::default()
    }
}

/// A minimal in-memory [`SchemaResolver`] for tests and for the pre-AST period.
/// Uses sorted `Vec`s (no `HashMap` — semantic path discipline) so lookups are
/// deterministic. The real frontend will supply its own implementation over
/// the schema graph; this one lets brix-ir's checks be exercised today.
#[derive(Default, Debug)]
pub struct TableResolver {
    relations: Vec<RelationSchema>,
    functions: Vec<FnSignature>,
    witnesses: Vec<QualIdent>,
}

impl TableResolver {
    pub fn new() -> Self {
        TableResolver::default()
    }

    pub fn with_relation(mut self, schema: RelationSchema) -> Self {
        match self
            .relations
            .binary_search_by(|r| r.name.cmp(&schema.name))
        {
            Ok(pos) => self.relations[pos] = schema,
            Err(pos) => self.relations.insert(pos, schema),
        }
        self
    }

    /// Register a function signature. Same-name declarations are kept as
    /// typed overloads (declaration order preserved within the name group).
    /// Identical param/ret shapes replace the prior entry so re-seeding a
    /// prelude is idempotent.
    pub fn with_function(mut self, sig: FnSignature) -> Self {
        let start = self.functions.partition_point(|f| f.name < sig.name);
        let mut end = start;
        while end < self.functions.len() && self.functions[end].name == sig.name {
            if self.functions[end].params == sig.params && self.functions[end].ret == sig.ret {
                self.functions[end] = sig;
                return self;
            }
            end += 1;
        }
        self.functions.insert(end, sig);
        self
    }

    pub fn with_witness(mut self, relation: QualIdent) -> Self {
        if let Err(pos) = self.witnesses.binary_search(&relation) {
            self.witnesses.insert(pos, relation);
        }
        self
    }
}

impl SchemaResolver for TableResolver {
    fn relation(&self, name: &QualIdent) -> Option<&RelationSchema> {
        self.relations
            .binary_search_by(|r| r.name.cmp(name))
            .ok()
            .map(|pos| &self.relations[pos])
    }

    fn functions(&self, name: &QualIdent) -> &[FnSignature] {
        let start = self.functions.partition_point(|f| f.name < *name);
        let mut end = start;
        while end < self.functions.len() && self.functions[end].name == *name {
            end += 1;
        }
        &self.functions[start..end]
    }

    fn has_completeness_witness(&self, relation: &QualIdent) -> bool {
        self.witnesses.binary_search(relation).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_resolver_round_trips_a_relation_schema() {
        let schema = RelationSchema {
            name: QualIdent::from("ComputedPrice"),
            roles: vec![
                (Ident::new("order"), Ty::NodeRef(Ident::new("Order"))),
                (Ident::new("amount"), Ty::F64),
            ],
            key: vec![Ident::new("order")],
            model_closed: true,
            derived: true,
        };
        let r = TableResolver::new().with_relation(schema);
        let got = r.relation(&QualIdent::from("ComputedPrice")).unwrap();
        assert_eq!(got.key, vec![Ident::new("order")]);
        assert!(got.model_closed && got.derived);
        assert!(r.relation(&QualIdent::from("Nope")).is_none());
    }

    #[test]
    fn witness_lookup() {
        let r = TableResolver::new().with_witness(QualIdent::from("Delivered"));
        assert!(r.has_completeness_witness(&QualIdent::from("Delivered")));
        assert!(!r.has_completeness_witness(&QualIdent::from("Other")));
    }

    #[test]
    fn same_name_functions_are_kept_as_overloads() {
        use crate::effects::EffectRow;
        use crate::types::IntWidth;
        let r = TableResolver::new()
            .with_function(FnSignature {
                name: QualIdent::from("clamp"),
                params: vec![
                    Ty::Int(IntWidth::Int),
                    Ty::Int(IntWidth::Int),
                    Ty::Int(IntWidth::Int),
                ],
                ret: Ty::Int(IntWidth::Int),
                effects: EffectRow::empty(),
                is_aggregate: false,
                may_diverge: false,
            })
            .with_function(FnSignature {
                name: QualIdent::from("clamp"),
                params: vec![Ty::F64, Ty::F64, Ty::F64],
                ret: Ty::F64,
                effects: EffectRow::empty(),
                is_aggregate: false,
                may_diverge: false,
            });
        let overloads = r.functions(&QualIdent::from("clamp"));
        assert_eq!(overloads.len(), 2);
        assert_eq!(overloads[0].ret, Ty::Int(IntWidth::Int));
        assert_eq!(overloads[1].ret, Ty::F64);
        assert_eq!(r.function(&QualIdent::from("clamp")).unwrap().ret, Ty::Int(IntWidth::Int));
    }
}
