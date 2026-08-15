//! The kernel's compiled-in, immutable, judgment-scoped primitive-relation
//! registry (ADR-0015 ⟨D-PRIM⟩, §5 Stage B).
//!
//! The identity scheme this module mints — [`PrimitiveRelationId`],
//! [`crate::SchemaId`] — is pinned by ADR-0023 ⟨D-RELID⟩/⟨D-SCHEMAID⟩ (Accepted),
//! which also carries the lossy-row decision (⟨D-LOSSYROW⟩, confirmed), the
//! sibling-domain constraint on the settlement lane (⟨D-DISJOINT⟩), and the
//! endpoint-vocabulary finding that re-scoped ADR-0015 Stage D's gate 1.
//!
//! **What problem this solves.** `elaborate_tree` turns every derivation leaf
//! into a *hypothesis*, so the kernel proves `leaves ⇒ conclusion` and never
//! checks that any leaf's realization actually holds. That is why a result must
//! be capped by its least-discharged leaf. Which leaves count as discharged was
//! established by **prose**: a doc comment argued `g_var ↔ Hyp`, `g_app2 ↔ modus
//! ponens`. Those correspondences are real, but the mechanism does not scale and
//! cannot be audited — and `g_arith` has no kernel rule to correspond to at all,
//! so under that mechanism its discharge could only ever be an assertion.
//!
//! This module is the mechanism that replaces the argument: a finite, exact,
//! kernel-owned relation, and a zero-premise term ([`crate::TermKind::PrimRealizes`])
//! that synthesizes `Prop::Realizes(g, src, dst)` **iff** `(src, dst)` is an
//! exact member of it.
//!
//! **What it deliberately does not do.**
//!
//! - It moves no grade. A leaf is closed only when a certificate actually
//!   contains the `PrimRealizes` term *and* the kernel accepted the resulting
//!   proof (ADR-0015 §5 Stage D, §8.7). Shipping this registry upgrades no
//!   existing proof: old certificates whose leaves are `Hyp` remain
//!   assumption-dependent and remain capped (§7).
//! - It says nothing about evaluation. Per ⟨D-JUDGE⟩ the judgment kind is inside
//!   the relation identity, so a typing relation is *structurally incapable* of
//!   synthesizing an evaluation generator's `Realizes` (§8.6). Nothing here
//!   claims `1 + 2 ⇓ 3`, totality, progress, or termination.
//! - It proves nothing about the Rust generator that produced an input. The rule
//!   decides that the submitted instance belongs to the kernel's normative
//!   relation; it does not establish that the host's inference is complete,
//!   deterministic, or incapable of producing rejected outputs (§8.10).
//!
//! **Why the rows are enumerated here rather than imported.** ADR-0015 §8.5
//! refuses to trust host-computed joins, coercion paths, and result types. So
//! the numeric tower, the join, the division result rule, and the promotion-edge
//! ids below are the kernel's *own* declaration, written out in this file. They
//! must of course agree with what `soc-regimes` emits or no row will ever
//! match — but that agreement is a fact a gate checks by running the real
//! generator, not an assumption inherited by sharing a table.
//!
//! **Why an expansion rather than 120 literal rows.** §8.9 forbids wildcard or
//! pattern rows: the relation the *checker* consults must be finite and exact,
//! and it is — membership is a `BTreeSet` lookup with no predicate, range rule,
//! or decision procedure at check time. The expansion below runs once, at
//! registry construction, and its complete output is frozen in
//! `vectors/primitive_relation_typing_arith_v{1,2}.json` so every row is
//! auditable byte-for-byte and a change to the expansion fails a gate rather
//! than a review.
//!
//! **One arithmetic relation.** `TypingArithV2` (Stage E ⟨D-PROMOTE⟩, ADR-0024)
//! is it. The superseded `TypingArithV1` was retired rather than retained — see
//! [`compiled_in_relations`] for the ruling and ADR-0024 §3 for the argument.
//! Its id resolves to `None`, which fails closed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{GeneratorId, PropositionId};

use crate::prim_schema::{
    arith_typing_input_schema_id, numeric_result_type_schema_id, ArithOperatorV1,
    ArithTypingInputV1, CoercionEdgeV1, CoercionKind, NumericResultTypeV1, NumericTypeNameV1,
    SchemaId,
};

/// The fixed marker opening a [`PrimitiveRelationId`] preimage. Frozen v1 ABI.
pub const PRIMITIVE_RELATION_MARKER_V1: &[u8] = b"brix.kernel.primitive-relation";

/// The format version written into every [`PrimitiveRelationId`] preimage.
pub const PRIMITIVE_RELATION_VERSION_V1: u64 = 1;

/// The proposition kind a primitive relation speaks about (ADR-0015 ⟨D-JUDGE⟩).
///
/// **Closed, not open.** ADR-0015 §9 leaves the choice open and recommends
/// closed, for the reason that decides it: an unknown kind must never silently
/// default to "discharged". A closed enum makes an unrecognised kind a compile
/// error rather than a runtime fall-through. Revisit if a third kind arrives.
///
/// Canonical ABI ordinals — append-only, never reordered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum JudgmentKind {
    /// `Γ ⊢ e : T`. The only kind that exists today.
    Typing,
}

impl JudgmentKind {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            JudgmentKind::Typing => 0,
        }
    }
}

impl Canonical for JudgmentKind {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// The identity of a primitive realization relation.
///
/// **Content-derived, not a name.** ADR-0015 §7 requires that adding, removing
/// or changing a row does not update `TypingArithV1` — it allocates
/// `TypingArithV2`, "otherwise identical certificate bytes would mean different
/// things under different kernel releases." Deriving the id from the relation's
/// *entire contents* makes that structural: edit a row and the id changes by
/// construction, so a stale id cannot survive a semantic edit. A named id would
/// leave §7 as a rule a reviewer has to remember.
///
/// The trade-off is that the id is not human-legible and the row set is only
/// auditable through a frozen vector. Those vectors exist
/// (`vectors/primitive_relation_typing_arith_v{1,2}.json`), and each lists
/// every row in readable form beside its digests.
///
/// Stage E is the first exercise of this: relocating one coercion edge changed
/// 20 rows and therefore produced a genuinely different id, with no separate
/// step required to allocate it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PrimitiveRelationId(pub Digest);

impl PrimitiveRelationId {
    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics, vectors).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl Canonical for PrimitiveRelationId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// One row of a primitive relation: an exact `(src, dst)` endpoint pair, in the
/// form the elaborator actually produces.
///
/// The endpoints are [`PropositionId`]s rather than `ConfigId`s because that is
/// what a leaf's `ObjectTerm::Const` carries — `brix-elaborate`'s
/// `tree_obj_to_object_term` maps `TreeObj::Atom(c)` to
/// `ObjectTerm::Const(PropositionId(c.digest()))`. Storing the same
/// representation the checker will compare against removes a conversion step
/// from the trusted path.
pub type Row = (PropositionId, PropositionId);

/// An immutable, judgment-scoped primitive realization relation.
///
/// Per ⟨D-PRIM⟩ the minimum semantic partition is one relation per **judgment
/// kind × generator × schema version**; all four are fields here, and all four
/// are bound into [`PrimitiveRelationId`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PrimitiveRelation {
    /// The proposition kind this relation is about. A typing relation cannot
    /// license evaluation, equality, or numeric correctness (§8.6).
    pub judgment_kind: JudgmentKind,
    /// The generator whose `Realizes` this relation synthesizes. **The relation
    /// identity fixes the generator; the caller does not supply it** (⟨D-PRIM⟩).
    pub generator: GeneratorId,
    /// The schema `src` must be canonical under.
    pub source_schema: SchemaId,
    /// The schema `dst` must be canonical under.
    pub destination_schema: SchemaId,
    /// The finite, exact set of admissible endpoint pairs.
    pub rows: BTreeSet<Row>,
}

impl PrimitiveRelation {
    /// This relation's content-derived identity.
    pub fn id(&self) -> PrimitiveRelationId {
        PrimitiveRelationId(Digest::of(Domain::Value, &self.canon_bytes()))
    }

    /// Whether `(src, dst)` is an exact member. This is the entire semantic
    /// content of the checking rule: no predicate, no range, no closure.
    pub fn admits(&self, src: &PropositionId, dst: &PropositionId) -> bool {
        self.rows.contains(&(*src, *dst))
    }
}

impl Canonical for PrimitiveRelation {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Frozen v1 preimage: marker, version, judgment kind, generator,
        // source schema, destination schema, rows. Field order is ABI.
        w.write_bytes(PRIMITIVE_RELATION_MARKER_V1);
        w.write_uint(PRIMITIVE_RELATION_VERSION_V1);
        self.judgment_kind.canon_write(w);
        w.write_bytes(self.generator.digest().as_bytes());
        self.source_schema.canon_write(w);
        self.destination_schema.canon_write(w);
        // A relation is a *set* of pairs: sorted by canonical element bytes and
        // deduplicated, so the id cannot depend on authoring order.
        w.write_set(self.rows.iter().map(|(src, dst)| {
            let mut e = CanonWriter::new();
            e.write_bytes(src.digest().as_bytes());
            e.write_bytes(dst.digest().as_bytes());
            e.finish()
        }));
    }
}

// ---------------------------------------------------------------------------
// The kernel's own numeric tower.
//
// Written out here rather than imported from `soc_regimes::NUMERIC`: ADR-0015
// §8.5 does not trust a host-computed join, coercion path, or result type, and
// a relation assembled from the host's table would serialize the host's
// assertion rather than move authority into the kernel (§8.3, the
// caller-supplied-table rejection). Agreement with the regime is checked by
// running the real generator in `arithmetic_rule_is_a_kernel_primitive`.
// ---------------------------------------------------------------------------

/// The coercion tower ℕ ⊂ ℤ ⊂ ℚ ⊂ ℝ ⊂ ℂ plus the lossy `Int ↪ Float` branch,
/// each edge tagged with whether it preserves numeric identity.
///
/// `Float` has no outgoing edge and only `Int` reaches it, which is what makes
/// `Float` incomparable to `Rat`/`Real`/`Complex` — `join(Float, Rat)` is
/// `None`, so no row exists for that mixture and gate 3
/// (`arithmetic_rule_has_no_unchecked_join`) holds by construction rather than
/// by an exclusion the author had to remember.
const TOWER: &[(NumericTypeNameV1, NumericTypeNameV1, CoercionKind)] = &[
    (
        NumericTypeNameV1::Nat,
        NumericTypeNameV1::Int,
        CoercionKind::Exact,
    ),
    (
        NumericTypeNameV1::Int,
        NumericTypeNameV1::Rat,
        CoercionKind::Exact,
    ),
    (
        NumericTypeNameV1::Rat,
        NumericTypeNameV1::Real,
        CoercionKind::Exact,
    ),
    (
        NumericTypeNameV1::Real,
        NumericTypeNameV1::Complex,
        CoercionKind::Exact,
    ),
    (
        NumericTypeNameV1::Int,
        NumericTypeNameV1::Float,
        CoercionKind::Lossy,
    ),
];

/// Every node of the tower, in ordinal order.
const NODES: &[NumericTypeNameV1] = &[
    NumericTypeNameV1::Nat,
    NumericTypeNameV1::Int,
    NumericTypeNameV1::Rat,
    NumericTypeNameV1::Real,
    NumericTypeNameV1::Complex,
    NumericTypeNameV1::Float,
];

/// Every operator, in ordinal order.
const OPERATORS: &[ArithOperatorV1] = &[
    ArithOperatorV1::Add,
    ArithOperatorV1::Sub,
    ArithOperatorV1::Mul,
    ArithOperatorV1::Div,
];

/// The reflexive–transitive upward closure of `name`, including `name`.
fn ancestors(name: NumericTypeNameV1) -> Vec<NumericTypeNameV1> {
    let mut out = vec![name];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        for (from, to, _) in TOWER {
            if *from == cur && !out.contains(to) {
                out.push(*to);
            }
        }
        i += 1;
    }
    out
}

/// `a ≤ b`: a value of type `a` coerces up to `b` along the tower.
fn le(a: NumericTypeNameV1, b: NumericTypeNameV1) -> bool {
    ancestors(a).contains(&b)
}

/// The least upper bound, or `None` when the two types are incomparable — the
/// case that makes mixing `Float` with `Rat` a type error rather than a silent
/// coercion.
fn join(a: NumericTypeNameV1, b: NumericTypeNameV1) -> Option<NumericTypeNameV1> {
    let aa = ancestors(a);
    let bb = ancestors(b);
    let common: Vec<NumericTypeNameV1> = aa.into_iter().filter(|x| bb.contains(x)).collect();
    common
        .iter()
        .copied()
        .find(|&x| common.iter().all(|&y| le(x, y)))
}

/// The least numeric *field* (closed under division) at or above `name`.
///
/// This is the language's declared division result rule: `Int / Int : Float`.
/// It is the field that makes `Div` differ from the other three operators, and
/// therefore the reason ADR-0015 Stage B0 had to re-schema the source object at
/// all — before it, `1.0 + 2.0` and `7 / 2` emitted the identical leaf.
fn field_of(name: NumericTypeNameV1) -> NumericTypeNameV1 {
    match name {
        NumericTypeNameV1::Nat | NumericTypeNameV1::Int => NumericTypeNameV1::Float,
        other => other,
    }
}

/// The unique upward edge path from `from` to `to`, or `None` if unreachable.
/// Empty means identity.
fn edge_path(
    from: NumericTypeNameV1,
    to: NumericTypeNameV1,
) -> Option<Vec<(NumericTypeNameV1, NumericTypeNameV1, CoercionKind)>> {
    if from == to {
        return Some(Vec::new());
    }
    if !le(from, to) {
        return None;
    }
    // Breadth-first over a tower whose up-paths are unique, so the first route
    // found is the route.
    let mut reached_via: BTreeMap<
        NumericTypeNameV1,
        (NumericTypeNameV1, NumericTypeNameV1, CoercionKind),
    > = BTreeMap::new();
    let mut queue = vec![from];
    let mut i = 0;
    while i < queue.len() {
        let cur = queue[i];
        i += 1;
        for (a, b, kind) in TOWER {
            if *a == cur && *b != from && !reached_via.contains_key(b) {
                reached_via.insert(*b, (*a, *b, *kind));
                queue.push(*b);
            }
        }
    }
    let mut path = Vec::new();
    let mut node = to;
    while node != from {
        let edge = *reached_via.get(&node)?;
        path.push(edge);
        node = edge.0;
    }
    path.reverse();
    Some(path)
}

/// The generator id naming one coercion edge.
///
/// **The family follows from the declared exactness, and only from it**
/// (ADR-0024 ⟨D-LOSSYFAMILY⟩). An exact edge is named under the promotion
/// family; a lossy edge under `type.rule.num.convert.lossy`, so no id in any
/// relation asserts an embedding for a map that does not preserve numeric
/// identity (⟨D-PROMOTE⟩). Deriving the family here rather than tabulating it
/// means an id and its [`CoercionKind`] tag cannot disagree — before Stage E
/// they did, with `Int ↪ Float` tagged lossy and named as a promotion.
///
/// The pre-Stage-E naming, in which every edge sat in the promotion family,
/// was carried by an `EdgeNaming::LegacyAllPromote` variant for as long as
/// `TypingArithV1` was compiled in. Retiring that relation retired the variant
/// with it: the naming is now one fact, not a choice.
fn edge_generator(
    from: NumericTypeNameV1,
    to: NumericTypeNameV1,
    kind: CoercionKind,
) -> GeneratorId {
    let prefix = match kind {
        CoercionKind::Exact => "type.rule.num.promote",
        CoercionKind::Lossy => "type.rule.num.convert.lossy",
    };
    GeneratorId::named(&format!(
        "{prefix}.{}_{}@1",
        from.lattice_name(),
        to.lattice_name()
    ))
}

/// The witnessed promotion path `from ↪ … ↪ to` as the canonical data a source
/// object carries.
///
/// The per-edge generator id is spelled here rather than imported: it is the
/// one identifier the kernel and the regime agree on, and §8.5 does not trust
/// the host to name the edges of a path the kernel is about to authorize.
fn promotion_path(from: NumericTypeNameV1, to: NumericTypeNameV1) -> Option<Vec<CoercionEdgeV1>> {
    Some(
        edge_path(from, to)?
            .into_iter()
            .map(|(a, b, kind)| CoercionEdgeV1 {
                generator: edge_generator(a, b, kind),
                kind,
            })
            .collect(),
    )
}

/// Build the exhaustive arithmetic row set under a given edge naming.
///
/// **On lossy promotion paths.** `Div` routes integer division through
/// `field_of(Int) == Float`, so `7 / 2` travels the `Int ↪ Float` edge, which
/// Stage B0 tags [`CoercionKind::Lossy`]. Those rows **are** included, and the
/// reasoning is ⟨D-JUDGE⟩'s: a typing discharge never claims a value,
/// evaluation, or exactness property, and `7 / 2 : Float` is a correct typing
/// judgement for a language whose declared rule is `Int/Int → Float`.
/// ⟨D-PROMOTE⟩'s prohibition is on discharging `Int→Float` *as an embedding or
/// promotion*, which after Stage E is a claim no id in this relation makes:
/// the edge is named under `type.rule.num.convert.lossy`, not under the
/// promotion family. Exactness is bound into the row's `src` bytes, so the
/// relation can never accept a lossy path where an exact one was claimed.
///
/// The counter-reading — ADR-0015 §5 Stage B0's "an ordered sequence of
/// **exact** promotion-edge ids", which read strictly would exclude these rows
/// — was confirmed against on ⟨D-JUDGE⟩'s grounds and is closed. The phrase
/// carries an inline erratum in ADR-0015 §5 Stage B0 restating it as "coercion
/// -edge ids, each carrying its declared exactness", and after Stage E these
/// paths are not sequences of *promotion*-edge ids at all.
fn arith_rows() -> BTreeSet<Row> {
    let mut rows = BTreeSet::new();
    for op in OPERATORS {
        for lhs in NODES {
            for rhs in NODES {
                let Some(base) = join(*lhs, *rhs) else {
                    // Incomparable operands (e.g. Float with Rat): no result
                    // type, so no row. Absence is not refutation (§8.8) — the
                    // kernel simply has not introduced the fact.
                    continue;
                };
                let result = if *op == ArithOperatorV1::Div {
                    field_of(base)
                } else {
                    base
                };
                // Both operands must reach the type the operation is performed
                // at. `Div` on `Real` stays at `Real`; `Div` on `Nat` lands at
                // `Float`, which `Nat` reaches via `Nat ↪ Int ↪ Float`.
                let (Some(lhs_path), Some(rhs_path)) =
                    (promotion_path(*lhs, result), promotion_path(*rhs, result))
                else {
                    continue;
                };
                let src = ArithTypingInputV1 {
                    operator: *op,
                    lhs_type: *lhs,
                    rhs_type: *rhs,
                    lhs_promotion_path: lhs_path,
                    rhs_promotion_path: rhs_path,
                };
                let dst = NumericResultTypeV1 { name: result };
                rows.insert((
                    PropositionId(src.config_id().digest()),
                    PropositionId(dst.config_id().digest()),
                ));
            }
        }
    }
    rows
}

/// Build an arithmetic typing relation, enforcing the functionality invariant.
///
/// ADR-0015 §5 Stage B: "Rows must satisfy a build-time functionality
/// invariant: one canonical `src` never maps to two result types." Checked here,
/// at construction, rather than only in a test — a registry that violated it
/// would let one source object be realized to two different result types, and
/// the kernel would accept both.
fn build_arith_relation() -> PrimitiveRelation {
    let rows = arith_rows();

    let mut seen: BTreeMap<PropositionId, PropositionId> = BTreeMap::new();
    for (src, dst) in &rows {
        if let Some(previous) = seen.insert(*src, *dst) {
            assert_eq!(
                previous, *dst,
                "arithmetic functionality invariant violated: one canonical \
                 src maps to two distinct result types"
            );
        }
    }

    PrimitiveRelation {
        judgment_kind: JudgmentKind::Typing,
        generator: GeneratorId::named("type.rule.arith@1"),
        source_schema: arith_typing_input_schema_id(),
        destination_schema: numeric_result_type_schema_id(),
        rows,
    }
}

/// `TypingArithV2` — the Stage E relation ⟨D-PROMOTE⟩, and the only arithmetic
/// typing relation this kernel carries.
///
/// It differed from the retired `TypingArithV1` in exactly one thing: paths
/// crossing `Int ↪ Float` name that edge under `type.rule.num.convert.lossy`
/// rather than under the promotion family — 20 of the 120 rows. No grade and no
/// inferred type changed; what changed is that no id in the relation asserts an
/// embedding for a map that does not preserve numeric identity.
///
/// **It keeps the name `V2` now that `V1` is gone**, and renaming it would be
/// the exact reinterpretation ADR-0015 §7 forbids: `V1`'s identity existed, so
/// reusing its name for a different row set would make one name mean two things
/// across two kernel releases. Version names here are historical, not slots.
fn build_typing_arith_v2() -> PrimitiveRelation {
    build_arith_relation()
}

/// The compiled-in registry, built once.
///
/// One physical artifact is fine as packaging; the semantic partition is by
/// judgment kind × generator × schema version, which lives in each relation's
/// own fields and identity (⟨D-PRIM⟩).
fn registry() -> &'static BTreeMap<PrimitiveRelationId, PrimitiveRelation> {
    static REGISTRY: OnceLock<BTreeMap<PrimitiveRelationId, PrimitiveRelation>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map = BTreeMap::new();
        for relation in compiled_in_relations() {
            map.insert(relation.id(), relation);
        }
        map
    })
}

/// Every relation compiled into this kernel release. Adding one is a kernel
/// release, which is the correct cost: a generator whose realization is not
/// derivable from existing kernel rules *is* a new trusted axiom (⟨D-PRIM⟩).
/// **`TypingArithV1` is deliberately absent** (ADR-0024 §3). It was retained
/// briefly on §7's plain reading that identities persist, and retired on the
/// maintainer ruling that §7 promises this to certificates that *exist*: none
/// naming V1 ever did, because Stage D has not landed and `elaborate_tree`
/// still emits every leaf as a `Hyp`. Keeping it would have been trusted TCB
/// data with nothing consulting it — the same thing ⟨D-EXACTCOVERED⟩ refused a
/// per-edge coercion relation for — and worse, its rows spelled
/// `type.rule.num.promote.Int_Float@1`, a generator family the lattice no
/// longer declares. Its id now resolves to `None`, which per §7 means the
/// kernel has not introduced the fact, never that its negation holds.
fn compiled_in_relations() -> Vec<PrimitiveRelation> {
    vec![build_typing_arith_v2()]
}

/// The identity of the current arithmetic **typing** relation (ADR-0015 §5
/// Stage E ⟨D-PROMOTE⟩).
///
/// Pinned by `vectors/primitive_relation_typing_arith_v2.json`.
pub fn typing_arith_v2() -> PrimitiveRelationId {
    build_typing_arith_v2().id()
}

/// Resolve a relation id against the compiled-in registry.
///
/// Fails closed on an unknown id, per ADR-0015 §7: "An unknown relation id under
/// a known `PrimRealizes` constructor also fails closed." `None` means the
/// kernel has not introduced the fact — never that its negation holds (§8.8).
pub fn resolve(id: &PrimitiveRelationId) -> Option<&'static PrimitiveRelation> {
    registry().get(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reachable operand pairs, spelled out independently of `join`: the 25
    /// pairs inside the exact tower, plus the five `Float`-compatible pairs.
    /// `Float` mixed with `Rat`/`Real`/`Complex` is absent — that is gate 3's
    /// property, holding by construction.
    #[test]
    fn the_matrix_is_the_thirty_joinable_pairs_times_four_operators() {
        let exact = [
            NumericTypeNameV1::Nat,
            NumericTypeNameV1::Int,
            NumericTypeNameV1::Rat,
            NumericTypeNameV1::Real,
            NumericTypeNameV1::Complex,
        ];
        let mut expected = 0;
        for a in NODES {
            for b in NODES {
                let both_exact = exact.contains(a) && exact.contains(b);
                let float_compatible = matches!(
                    (a, b),
                    (NumericTypeNameV1::Float, NumericTypeNameV1::Float)
                        | (NumericTypeNameV1::Float, NumericTypeNameV1::Nat)
                        | (NumericTypeNameV1::Float, NumericTypeNameV1::Int)
                        | (NumericTypeNameV1::Nat, NumericTypeNameV1::Float)
                        | (NumericTypeNameV1::Int, NumericTypeNameV1::Float)
                );
                assert_eq!(
                    join(*a, *b).is_some(),
                    both_exact || float_compatible,
                    "join({a:?}, {b:?})"
                );
                if both_exact || float_compatible {
                    expected += 1;
                }
            }
        }
        assert_eq!(expected, 30, "joinable operand pairs");
        assert_eq!(
            arith_rows().len(),
            30 * OPERATORS.len()
        );
    }

    /// `Div` is the operator the whole Stage B0 re-schema existed for: it has a
    /// different result rule from the other three, and the source object must
    /// make that visible.
    #[test]
    fn division_has_its_own_result_rule() {
        assert_eq!(field_of(NumericTypeNameV1::Int), NumericTypeNameV1::Float);
        assert_eq!(field_of(NumericTypeNameV1::Nat), NumericTypeNameV1::Float);
        assert_eq!(field_of(NumericTypeNameV1::Rat), NumericTypeNameV1::Rat);

        let add = ArithTypingInputV1 {
            operator: ArithOperatorV1::Add,
            lhs_type: NumericTypeNameV1::Int,
            rhs_type: NumericTypeNameV1::Int,
            lhs_promotion_path: Vec::new(),
            rhs_promotion_path: Vec::new(),
        };
        let relation = build_typing_arith_v2();
        let int = NumericResultTypeV1 {
            name: NumericTypeNameV1::Int,
        };
        let float = NumericResultTypeV1 {
            name: NumericTypeNameV1::Float,
        };
        assert!(relation.admits(
            &PropositionId(add.config_id().digest()),
            &PropositionId(int.config_id().digest())
        ));
        // The same operands under `+` do not conclude `Float`.
        assert!(!relation.admits(
            &PropositionId(add.config_id().digest()),
            &PropositionId(float.config_id().digest())
        ));
    }

    /// `Nat / Nat` lands at `Float`, so both operands travel `Nat ↪ Int ↪ Float`
    /// — a two-edge path whose second edge is lossy. The path is data in the
    /// source object, so the row is keyed on it.
    #[test]
    fn division_on_nat_carries_a_two_edge_path_ending_lossy() {
        let path = promotion_path(NumericTypeNameV1::Nat, NumericTypeNameV1::Float).unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].kind, CoercionKind::Exact);
        assert_eq!(path[1].kind, CoercionKind::Lossy);

        // Stage E ⟨D-PROMOTE⟩: the exact edge keeps the promotion family, the
        // lossy one does not. The id no longer asserts an embedding for a map
        // that is not injective.
        assert_eq!(
            path[0].generator,
            GeneratorId::named("type.rule.num.promote.Nat_Int@1")
        );
        assert_eq!(
            path[1].generator,
            GeneratorId::named("type.rule.num.convert.lossy.Int_Float@1")
        );
    }

    /// The retired `TypingArithV1`, rebuilt **here in the test** rather than
    /// compiled into the kernel.
    ///
    /// This is the whole point of the retirement: the legacy row set is history
    /// worth auditing, but it is not trusted data, so it lives in a test where
    /// nothing can resolve it. Its one difference from the current relation is
    /// that every edge, exact or lossy, sat in the promotion family.
    ///
    /// The expansion is duplicated rather than shared with [`arith_rows`], and
    /// deliberately: a retired relation is frozen by definition, so there is no
    /// future edit for the two copies to drift across, and sharing the code
    /// would mean the kernel still carried the legacy naming.
    fn legacy_relation() -> PrimitiveRelation {
        fn legacy_path(
            from: NumericTypeNameV1,
            to: NumericTypeNameV1,
        ) -> Option<Vec<CoercionEdgeV1>> {
            Some(
                edge_path(from, to)?
                    .into_iter()
                    .map(|(a, b, kind)| CoercionEdgeV1 {
                        generator: GeneratorId::named(&format!(
                            "type.rule.num.promote.{}_{}@1",
                            a.lattice_name(),
                            b.lattice_name()
                        )),
                        kind,
                    })
                    .collect(),
            )
        }

        let mut rows = BTreeSet::new();
        for op in OPERATORS {
            for lhs in NODES {
                for rhs in NODES {
                    let Some(base) = join(*lhs, *rhs) else {
                        continue;
                    };
                    let result = if *op == ArithOperatorV1::Div {
                        field_of(base)
                    } else {
                        base
                    };
                    let (Some(lhs_path), Some(rhs_path)) =
                        (legacy_path(*lhs, result), legacy_path(*rhs, result))
                    else {
                        continue;
                    };
                    let src = ArithTypingInputV1 {
                        operator: *op,
                        lhs_type: *lhs,
                        rhs_type: *rhs,
                        lhs_promotion_path: lhs_path,
                        rhs_promotion_path: rhs_path,
                    };
                    let dst = NumericResultTypeV1 { name: result };
                    rows.insert((
                        PropositionId(src.config_id().digest()),
                        PropositionId(dst.config_id().digest()),
                    ));
                }
            }
        }
        PrimitiveRelation {
            judgment_kind: JudgmentKind::Typing,
            generator: GeneratorId::named("type.rule.arith@1"),
            source_schema: arith_typing_input_schema_id(),
            destination_schema: numeric_result_type_schema_id(),
            rows,
        }
    }

    /// An edge's generator family follows its declared exactness and nothing
    /// else, so an id and its [`CoercionKind`] tag cannot disagree — which they
    /// did before Stage E, with `Int ↪ Float` tagged lossy and named as a
    /// promotion (ADR-0024 ⟨D-LOSSYFAMILY⟩).
    #[test]
    fn the_edge_family_follows_declared_exactness() {
        for (from, to, kind) in TOWER {
            // Both families spelled literally, so the test does not re-derive
            // the rule it is checking.
            let suffix = format!("{}_{}@1", from.lattice_name(), to.lattice_name());
            let as_promotion = GeneratorId::named(&format!("type.rule.num.promote.{suffix}"));
            let as_lossy = GeneratorId::named(&format!("type.rule.num.convert.lossy.{suffix}"));
            let actual = edge_generator(*from, *to, *kind);

            match kind {
                CoercionKind::Exact => {
                    assert_eq!(actual, as_promotion, "an exact edge stays a promotion");
                }
                CoercionKind::Lossy => {
                    assert_eq!(actual, as_lossy);
                    assert_ne!(
                        actual, as_promotion,
                        "no lossy edge may be named as a promotion"
                    );
                }
            }
        }
    }

    /// Stage E's blast radius, still audited after the retirement: the current
    /// relation is the legacy matrix with exactly one edge renamed — same
    /// generator, schemas, judgment kind and row count, 100 rows shared and 20
    /// moved. A different count would mean the relocation had silently changed
    /// what typechecks.
    #[test]
    fn the_current_relation_is_the_legacy_matrix_with_one_edge_renamed() {
        let legacy = legacy_relation();
        let current = build_typing_arith_v2();

        assert_eq!(legacy.judgment_kind, current.judgment_kind);
        assert_eq!(legacy.generator, current.generator);
        assert_eq!(legacy.source_schema, current.source_schema);
        assert_eq!(legacy.destination_schema, current.destination_schema);
        assert_eq!(legacy.rows.len(), current.rows.len());

        // 20 of the 120 rows have a path crossing `Int ↪ Float`: the 4 `Div`
        // rows whose operands are both Nat/Int (each crossing on *both* sides),
        // the 4 mixed Float pairs under `Div`, and the same 4 under each of
        // Add/Sub/Mul. `Float op Float` never crosses — both paths are empty.
        let shared = legacy.rows.intersection(&current.rows).count();
        assert_eq!(shared, 100, "rows not crossing the lossy edge are unchanged");
        assert_eq!(
            legacy.rows.len() - shared,
            20,
            "rows crossing it are relocated"
        );

        assert_ne!(legacy.id(), current.id());
    }

    /// The retired relation is genuinely gone, and its absence fails closed.
    ///
    /// This is stronger than "some unknown digest does not resolve": the id
    /// asked for is the *real* pre-Stage-E identity, rebuilt from the legacy row
    /// set. Per ADR-0015 §7 a `None` here means the kernel has not introduced
    /// the fact — never that its negation holds.
    #[test]
    fn the_retired_relation_is_absent_and_fails_closed() {
        let retired = legacy_relation().id();
        assert_ne!(retired, typing_arith_v2());
        assert!(
            resolve(&retired).is_none(),
            "TypingArithV1 must not resolve after retirement"
        );
        assert!(resolve(&typing_arith_v2()).is_some());
        assert_eq!(
            compiled_in_relations().len(),
            1,
            "one arithmetic relation is compiled in"
        );
    }

    /// The functionality invariant holds: one canonical `src`, one result type.
    #[test]
    fn one_source_object_never_maps_to_two_result_types() {
        // `build_arith_relation` asserts this; calling it is the test.
        let relation = build_typing_arith_v2();
        let mut sources: Vec<PropositionId> = relation.rows.iter().map(|(s, _)| *s).collect();
        let total = sources.len();
        sources.sort();
        sources.dedup();
        assert_eq!(sources.len(), total, "a src appears in at most one row");
    }

    /// The relation identity binds every field. Change any one of them and the
    /// id moves — which is what makes ADR-0015 §7's immutability structural.
    #[test]
    fn the_relation_identity_binds_every_field() {
        let base = build_typing_arith_v2();
        let base_id = base.id();

        let mut other_generator = base.clone();
        other_generator.generator = GeneratorId::named("type.rule.arith.input@1");
        assert_ne!(other_generator.id(), base_id, "generator must be material");

        let mut other_source = base.clone();
        other_source.source_schema = numeric_result_type_schema_id();
        assert_ne!(other_source.id(), base_id, "source schema must be material");

        let mut other_destination = base.clone();
        other_destination.destination_schema = arith_typing_input_schema_id();
        assert_ne!(
            other_destination.id(),
            base_id,
            "destination schema must be material"
        );

        let mut fewer_rows = base.clone();
        let dropped = *fewer_rows.rows.iter().next().unwrap();
        fewer_rows.rows.remove(&dropped);
        assert_ne!(fewer_rows.id(), base_id, "the row set must be material");
    }

    /// The registry resolves the relation it was built from, and refuses
    /// anything else. "Not in the registry" is absence, never refutation.
    #[test]
    fn resolution_fails_closed_on_an_unknown_id() {
        let id = typing_arith_v2();
        let relation = resolve(&id).expect("TypingArithV2 resolves");
        assert_eq!(relation.judgment_kind, JudgmentKind::Typing);
        assert_eq!(
            relation.generator,
            GeneratorId::named("type.rule.arith@1"),
            "the relation identity fixes the generator"
        );

        let unknown = PrimitiveRelationId(Digest::of(Domain::Value, b"not a relation"));
        assert!(resolve(&unknown).is_none());
    }

    /// The judgment kind is inside the identity, so a relation for another kind
    /// is a *different* relation and cannot be reached by the typing id. This is
    /// ⟨D-JUDGE⟩'s mechanical enforcement — the scoping lives in the relation
    /// identity, never in a comment or a naming convention.
    #[test]
    fn judgment_kind_is_part_of_the_relation_identity() {
        let typing = build_typing_arith_v2();
        let mut preimage = typing.canon_bytes();
        // The judgment-kind ordinal is written third, right after the marker
        // and version. Rather than patch bytes, rebuild the preimage by hand
        // with a different ordinal and confirm it digests differently.
        let mut w = CanonWriter::new();
        w.write_bytes(PRIMITIVE_RELATION_MARKER_V1);
        w.write_uint(PRIMITIVE_RELATION_VERSION_V1);
        w.write_enum(1, |_| {}); // a hypothetical second judgment kind
        w.write_bytes(typing.generator.digest().as_bytes());
        typing.source_schema.canon_write(&mut w);
        typing.destination_schema.canon_write(&mut w);
        w.write_set(typing.rows.iter().map(|(src, dst)| {
            let mut e = CanonWriter::new();
            e.write_bytes(src.digest().as_bytes());
            e.write_bytes(dst.digest().as_bytes());
            e.finish()
        }));
        let other = w.finish();

        assert_ne!(preimage, other);
        preimage.clear();
        assert!(resolve(&PrimitiveRelationId(Digest::of(Domain::Value, &other))).is_none());
    }

    /// Mixing `Float` with the exact branch has no join, so no row exists — and
    /// no promotion path is invented to manufacture one.
    #[test]
    fn float_mixed_with_the_exact_branch_has_no_row() {
        assert_eq!(join(NumericTypeNameV1::Float, NumericTypeNameV1::Rat), None);
        assert_eq!(
            promotion_path(NumericTypeNameV1::Rat, NumericTypeNameV1::Float),
            None
        );
        assert_eq!(
            promotion_path(NumericTypeNameV1::Float, NumericTypeNameV1::Rat),
            None
        );
    }
}
