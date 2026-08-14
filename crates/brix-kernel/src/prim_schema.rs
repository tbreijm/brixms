//! Source-object schemas for kernel-checked primitive realizations
//! (ADR-0015 ⟨D-PRIM⟩, §5 Stage B0).
//!
//! **Why a schema type lives in the kernel and not in the regime that builds
//! it.** ⟨D-PRIM⟩ puts the primitive-relation registry in the TCB, and §8.5
//! forbids trusting host-side semantic normalization: "joins, coercion paths,
//! result types, wildcard matching, transitive closure, and schema
//! interpretation are not trusted because the host computed them." A registry
//! row is matched by canonical bytes, so the type whose canonical bytes decide
//! membership must be the kernel's own. `soc-regimes` constructs values of
//! these types; it does not get to define what they encode to.
//!
//! **What Stage B0 fixes.** `g_arith`'s leaf was
//!
//! ```text
//! src: Prod(Atom(Type(result)), Atom(Type(result)))
//! dst: Atom(Type(result))
//! ```
//!
//! — both operands already coerced to the result type, promotions spliced in
//! as separate leaves. That encodes neither the operator, nor the original
//! operand types, nor the promotion paths, so `1.0 + 2.0` and `7 / 2` emitted
//! the *identical* leaf `Prod(Float, Float) → Float` even though `Div` has a
//! different result rule (`Int/Int → Float`). A relation keyed on
//! `(operator, lhs, rhs, promotions) → result` is unreachable from that source
//! object, which is exactly ADR-0015's rule: **if the source representation
//! does not encode every field that affects admissibility, the primitive MUST
//! NOT be discharged.**
//!
//! [`ArithTypingInputV1`] is that source object. Stage B0 only changes what is
//! *emitted* — no registry exists yet, every leaf is still a hypothesis, and
//! no grade moves.
//!
//! **Judgment scope.** These schemas describe a **typing** judgment
//! (`Γ ⊢ e₁ op e₂ : T`) and nothing else. Per ⟨D-JUDGE⟩ a typing rule is
//! compatible with mathematical, checked, wrapping, saturating, or arbitrary
//! total interpretations of an operator, so it cannot entail an evaluation
//! equation. Nothing here claims `1 + 2 ⇓ 3`, totality, progress, or
//! termination.

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, GeneratorId};

/// The identity of a source or destination **schema** — half of a primitive
/// relation's identity (ADR-0015 §7: "relation identities are immutable", and a
/// source schema is half of one).
///
/// Derived from the schema's already-frozen marker and version rather than from
/// a fresh name, so no new frozen string is minted and a schema's id cannot
/// drift from the bytes it actually writes. A v2 of any schema necessarily
/// yields a different `SchemaId`, which in turn yields a different
/// [`crate::PrimitiveRelationId`] — the immutability discipline becomes
/// structural instead of a rule a reviewer has to remember.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SchemaId(pub Digest);

impl SchemaId {
    /// The id of the schema whose preimages open with `marker` and `version`.
    ///
    /// The preimage is exactly `write_bytes(marker) ++ write_uint(version)` —
    /// the same two writes every schema in this module makes first, so the id
    /// is derived from the schema's own frozen header and nothing else.
    pub fn of_schema(marker: &[u8], version: u64) -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(marker);
        w.write_uint(version);
        SchemaId(Digest::of(Domain::Value, &w.finish()))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics, vectors).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl Canonical for SchemaId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// The fixed marker opening an [`ArithTypingInputV1`] preimage. Frozen v1 ABI.
///
/// Mirrors `brix_semantic::GENERATOR_SEMANTICS_MARKER_V1`'s shape (ADR-0020
/// D3) so the typing and settlement lanes share one identity discipline. It
/// also domain-separates this schema from the `Expr`/`Ty` configurations it
/// sits beside: every `ConfigId` is hashed under `Domain::Value`, so without a
/// marker a schema value and an expression could in principle canon-encode to
/// the same bytes.
pub const ARITH_TYPING_INPUT_MARKER_V1: &[u8] = b"brix.kernel.arith-typing-input";

/// The format version written into every [`ArithTypingInputV1`] preimage.
/// A new field, or a new meaning for an existing one, requires **v2** — it is
/// never appended opportunistically to v1 (ADR-0015 §7: relation identities
/// are immutable, and a source schema is half of a relation's identity).
pub const ARITH_TYPING_INPUT_VERSION_V1: u64 = 1;

/// The binary arithmetic operator a typing input is about.
///
/// Canonical ABI ordinals — append-only, never reordered. The ordinal is not
/// incidental: it is the field that makes `1.0 + 2.0` and `7 / 2`
/// distinguishable, so it contributes directly to the source object's
/// identity.
///
/// This is the kernel's own vocabulary rather than a re-export of
/// `soc_regimes::ArithOp`: the regime must not be able to change what a
/// registry row means by editing its own enum (§8.5), and `brix-kernel` may
/// not depend on `soc-regimes` in any case (TCB boundary, enforced by
/// `scripts/check_tcb_dependencies.py`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ArithOperatorV1 {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOperatorV1 {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            ArithOperatorV1::Add => 0,
            ArithOperatorV1::Sub => 1,
            ArithOperatorV1::Mul => 2,
            ArithOperatorV1::Div => 3,
        }
    }
}

impl Canonical for ArithOperatorV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// The closed set of numeric types an arithmetic operand can have.
///
/// **Why a closed kernel enum rather than a `ConfigId` of the regime's `Ty`.**
/// Stage B's registry rows are kernel-owned data, matched by canonical bytes.
/// If an operand type entered as a `ConfigId`, the kernel would have to
/// reproduce `soc_regimes::Ty`'s canonical encoding to author a single row —
/// a second encoder for a type the TCB does not own, which is exactly what
/// ADR-0015 §8.5 refuses to trust and what `DEPS.md` Ring0 §1.7 forbids
/// outright. Owning the vocabulary keeps row authoring inside the TCB.
///
/// Closed rather than open for the same reason [`crate`]'s callers keep
/// `ClaimKind` closed: `soc_regimes::arith_operand` already rejects every
/// non-numeric operand, so a name outside this set is a host bug, and the
/// conversion fails closed rather than inventing a variant.
///
/// Canonical ABI ordinals — append-only, never reordered. The order follows
/// the exact tower `ℕ ⊂ ℤ ⊂ ℚ ⊂ ℝ ⊂ ℂ` with the lossy `Float` branch last;
/// that is a readability convention only, and nothing may infer the coercion
/// order from it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum NumericTypeNameV1 {
    Nat,
    Int,
    Rat,
    Real,
    Complex,
    Float,
}

impl NumericTypeNameV1 {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            NumericTypeNameV1::Nat => 0,
            NumericTypeNameV1::Int => 1,
            NumericTypeNameV1::Rat => 2,
            NumericTypeNameV1::Real => 3,
            NumericTypeNameV1::Complex => 4,
            NumericTypeNameV1::Float => 5,
        }
    }

    /// The coercion-lattice node name for this type — the inverse of
    /// [`Self::from_lattice_node`].
    ///
    /// The kernel needs this to spell out the per-edge promotion generator ids
    /// that may appear in a relation row's promotion path
    /// (`type.rule.num.promote.{from}_{to}@1`). Those ids are the one
    /// identifier the kernel and the regime already agree on (see
    /// [`CoercionEdgeV1`]), so the kernel writes them itself rather than
    /// importing the regime's lattice — §8.5 does not trust a host-computed
    /// coercion path, and that includes the names of its edges.
    pub const fn lattice_name(self) -> &'static str {
        match self {
            NumericTypeNameV1::Nat => "Nat",
            NumericTypeNameV1::Int => "Int",
            NumericTypeNameV1::Rat => "Rat",
            NumericTypeNameV1::Real => "Real",
            NumericTypeNameV1::Complex => "Complex",
            NumericTypeNameV1::Float => "Float",
        }
    }

    /// The type named by a numeric coercion-lattice node, or `None` for a name
    /// outside the tower.
    ///
    /// Fallible rather than panicking: an unrecognised name means the host and
    /// the kernel disagree about what a numeric type is, and ADR-0002 §5.3
    /// requires that to fail closed — no arithmetic source object is built, so
    /// no relation can ever match one.
    pub fn from_lattice_node(name: &str) -> Option<Self> {
        match name {
            "Nat" => Some(NumericTypeNameV1::Nat),
            "Int" => Some(NumericTypeNameV1::Int),
            "Rat" => Some(NumericTypeNameV1::Rat),
            "Real" => Some(NumericTypeNameV1::Real),
            "Complex" => Some(NumericTypeNameV1::Complex),
            "Float" => Some(NumericTypeNameV1::Float),
            _ => None,
        }
    }
}

impl Canonical for NumericTypeNameV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// Whether a coercion edge preserves numeric identity.
///
/// **This distinction is load-bearing and is recorded rather than assumed.**
/// ADR-0015 ⟨D-PROMOTE⟩ rules that the exact edges `Nat→Int`, `Int→Rat`,
/// `Rat→Real`, `Real→Complex` are individually dischargeable, while
/// `Int→Float` "SHALL NOT be discharged as an embedding or promotion, now or
/// later" — a lossy map is not injective and does not preserve numeric
/// identity.
///
/// `soc_regimes`'s `NUMERIC` lattice nevertheless carries `Int ↪ Float`
/// alongside the exact tower, and describes all of its edges as the "*safe*
/// (information-preserving)" direction. `7 / 2` therefore travels a lossy edge
/// today. ADR-0015 §5 Stage B0 defines a promotion path as "an ordered
/// sequence of **exact** promotion-edge ids", so writing that edge into the
/// path unlabelled would encode a lossy conversion under a name asserting
/// exactness. Tagging each edge keeps the record honest without relocating the
/// edge — relocation is Stage E's ⟨D-PROMOTE⟩ work and would move a grade.
///
/// Canonical ABI ordinals — append-only, never reordered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CoercionKind {
    /// An injective, numeric-identity-preserving embedding. Individually
    /// dischargeable per ⟨D-PROMOTE⟩.
    Exact,
    /// A lossy conversion. Never dischargeable as a promotion or embedding;
    /// a future discharge would be of a *different* proposition (a specified
    /// width, rounding mode, and overflow behaviour), which `brix-canon`
    /// cannot express today because it excludes floats from `Canonical`.
    Lossy,
}

impl CoercionKind {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            CoercionKind::Exact => 0,
            CoercionKind::Lossy => 1,
        }
    }
}

impl Canonical for CoercionKind {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// One edge of a promotion path, named by the generator that witnesses it.
///
/// The edge is identified by its **generator id**, not by a pair of type names
/// or an index into a host-side table: the generator is the thing a future
/// per-edge discharge (Stage E) would be about, and it is the only identifier
/// the kernel and the regime already agree on.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CoercionEdgeV1 {
    /// The witnessed-coercion generator for this edge.
    pub generator: GeneratorId,
    /// Whether this edge preserves numeric identity. See [`CoercionKind`].
    pub kind: CoercionKind,
}

impl Canonical for CoercionEdgeV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: generator then kind.
        w.write_bytes(self.generator.digest().as_bytes());
        self.kind.canon_write(w);
    }
}

/// The `g_arith` source object (ADR-0015 §5 Stage B0).
///
/// Every field that affects admissibility of a binary arithmetic *typing*
/// judgement, and nothing else:
///
/// - `operator` — `Div`'s result rule differs from the other three, so the
///   operator is material to the result type.
/// - `lhs_type` / `rhs_type` — the operands' **original** types, as inferred,
///   *before* any promotion. This is the field the old leaf destroyed by
///   presenting both operands already coerced to the result type.
/// - `lhs_promotion_path` / `rhs_promotion_path` — the ordered sequence of
///   coercion edges taken from that operand's own type up to the type the
///   operation is performed at. **The empty path is identity.**
///
/// The result type is the leaf's `dst`, not a field here: it is what the
/// realization relation *concludes*, and the tree's own endpoint check pins it
/// against the claim.
///
/// **Path order is semantic**, so the two paths are encoded as lists rather
/// than sets, and `lhs` precedes `rhs` by field order — which is what makes an
/// operand-order swap detectable as a different [`ConfigId`].
///
/// ⟨D-PRIM⟩ §8.10: constructing this value is a claim about what the host
/// computed, never a claim that it is correct. Correctness is what a kernel
/// relation would decide by exact membership, and no such relation exists yet
/// (Stage B).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ArithTypingInputV1 {
    /// The binary operator.
    pub operator: ArithOperatorV1,
    /// The left operand's inferred type, before promotion.
    pub lhs_type: NumericTypeNameV1,
    /// The right operand's inferred type, before promotion.
    pub rhs_type: NumericTypeNameV1,
    /// Ordered coercion edges lifting the left operand to the type the
    /// operation is performed at. Empty means identity.
    pub lhs_promotion_path: Vec<CoercionEdgeV1>,
    /// Ordered coercion edges lifting the right operand to the type the
    /// operation is performed at. Empty means identity.
    pub rhs_promotion_path: Vec<CoercionEdgeV1>,
}

impl ArithTypingInputV1 {
    /// This source object's content-addressed configuration identity — what a
    /// tree leaf's `src` endpoint carries.
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

impl Canonical for ArithTypingInputV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Frozen v1 preimage: marker, version, operator, lhs type, rhs type,
        // lhs path, rhs path. Field order and framing are ABI.
        w.write_bytes(ARITH_TYPING_INPUT_MARKER_V1);
        w.write_uint(ARITH_TYPING_INPUT_VERSION_V1);
        self.operator.canon_write(w);
        self.lhs_type.canon_write(w);
        self.rhs_type.canon_write(w);
        // Lists, not sets: a promotion path is an *ordered* sequence, and two
        // paths over the same edges in different orders are different paths.
        w.write_list(self.lhs_promotion_path.iter().map(|e| e.canon_bytes()));
        w.write_list(self.rhs_promotion_path.iter().map(|e| e.canon_bytes()));
    }
}

/// The fixed marker opening a [`NumericResultTypeV1`] preimage. Frozen v1 ABI.
///
/// Domain-separated from [`ARITH_TYPING_INPUT_MARKER_V1`] for the same reason
/// that one exists: every `ConfigId` is a `Domain::Value` digest, so without
/// distinct markers a source object and a destination object could in principle
/// canon-encode to the same bytes — and a relation matched by exact membership
/// must never confuse its two endpoints.
pub const NUMERIC_RESULT_TYPE_MARKER_V1: &[u8] = b"brix.kernel.numeric-result-type";

/// The format version written into every [`NumericResultTypeV1`] preimage.
/// A new field, or a new meaning for an existing one, requires **v2**.
pub const NUMERIC_RESULT_TYPE_VERSION_V1: u64 = 1;

/// The `g_arith` destination object: the exact result type of a binary
/// arithmetic *typing* judgement (ADR-0015 §5 Stage B, `NumericResultTypeV1`).
///
/// **Why the result type needs a kernel-owned schema at all.** A registry row is
/// matched by canonical bytes, so the kernel must be able to author *both*
/// endpoints of the row. Stage B0 gave it the source endpoint
/// ([`ArithTypingInputV1`]). The destination was still
/// `soc_regimes::CfgAtom::Type(Ty::Con(result))`, whose encoding the kernel may
/// not reproduce — that would be a second semantic encoder for a type the TCB
/// does not own (ADR-0015 §8.5; `DEPS.md`, "never a second semantic encoder").
/// So the arithmetic leaf's `dst` becomes this schema, and the regime bridges
/// back to its own `Ty` vocabulary in a separate, explicitly undischarged leaf.
///
/// **What that bridge costs, stated plainly.** The bridge is not kernel-checkable
/// either, for the mirror-image reason. So Stage B does not lift the arithmetic
/// cap: it converts the *semantic* claim (`Div`'s result rule differs from the
/// other three) into a kernel-checked fact, and leaves two purely
/// vocabulary-renaming leaves as the residue. Closing those is a question about
/// who owns the canonical encoding of a realization endpoint, not a question
/// about arithmetic.
///
/// Deliberately a one-field struct rather than a bare [`NumericTypeNameV1`]:
/// the marker and version are what domain-separate a result object from an
/// operand type name that happens to share an ordinal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NumericResultTypeV1 {
    /// The exact result type the arithmetic typing rule concludes.
    pub name: NumericTypeNameV1,
}

impl NumericResultTypeV1 {
    /// This destination object's content-addressed configuration identity —
    /// what a tree leaf's `dst` endpoint carries.
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

impl Canonical for NumericResultTypeV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Frozen v1 preimage: marker, version, name. Field order is ABI.
        w.write_bytes(NUMERIC_RESULT_TYPE_MARKER_V1);
        w.write_uint(NUMERIC_RESULT_TYPE_VERSION_V1);
        self.name.canon_write(w);
    }
}

/// The [`SchemaId`] of [`ArithTypingInputV1`] — `TypingArithV1`'s source schema.
pub fn arith_typing_input_schema_id() -> SchemaId {
    SchemaId::of_schema(ARITH_TYPING_INPUT_MARKER_V1, ARITH_TYPING_INPUT_VERSION_V1)
}

/// The [`SchemaId`] of [`NumericResultTypeV1`] — `TypingArithV1`'s destination
/// schema.
pub fn numeric_result_type_schema_id() -> SchemaId {
    SchemaId::of_schema(
        NUMERIC_RESULT_TYPE_MARKER_V1,
        NUMERIC_RESULT_TYPE_VERSION_V1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(name: &str, kind: CoercionKind) -> CoercionEdgeV1 {
        CoercionEdgeV1 {
            generator: GeneratorId::named(name),
            kind,
        }
    }

    fn input() -> ArithTypingInputV1 {
        ArithTypingInputV1 {
            operator: ArithOperatorV1::Add,
            lhs_type: NumericTypeNameV1::Int,
            rhs_type: NumericTypeNameV1::Rat,
            lhs_promotion_path: vec![edge("type.rule.num.promote.Int_Rat@1", CoercionKind::Exact)],
            rhs_promotion_path: Vec::new(),
        }
    }

    /// Every field of the schema is material: changing any one of them alone
    /// changes the configuration identity. This is the property Stage B's
    /// registry will rest on — a row keyed on a `ConfigId` that ignored a
    /// field could be satisfied by an input that differs in it.
    #[test]
    fn every_field_is_bound_into_the_config_identity() {
        let base = input();
        let base_id = base.config_id();

        let mut op = base.clone();
        op.operator = ArithOperatorV1::Div;
        assert_ne!(op.config_id(), base_id, "operator must be material");

        let mut lhs = base.clone();
        lhs.lhs_type = NumericTypeNameV1::Nat;
        assert_ne!(lhs.config_id(), base_id, "lhs type must be material");

        let mut rhs = base.clone();
        rhs.rhs_type = NumericTypeNameV1::Real;
        assert_ne!(rhs.config_id(), base_id, "rhs type must be material");

        let mut lpath = base.clone();
        lpath.lhs_promotion_path = Vec::new();
        assert_ne!(lpath.config_id(), base_id, "lhs path must be material");

        let mut rpath = base.clone();
        rpath.rhs_promotion_path = vec![edge(
            "type.rule.num.promote.Rat_Real@1",
            CoercionKind::Exact,
        )];
        assert_ne!(rpath.config_id(), base_id, "rhs path must be material");
    }

    /// Operand order is material. Swapping the two operands (types and paths
    /// together) must not produce the same identity — otherwise a relation row
    /// authorizing `Nat - Int` would equally authorize `Int - Nat`.
    #[test]
    fn operand_order_is_material() {
        let base = input();
        let swapped = ArithTypingInputV1 {
            operator: base.operator,
            lhs_type: base.rhs_type,
            rhs_type: base.lhs_type,
            lhs_promotion_path: base.rhs_promotion_path.clone(),
            rhs_promotion_path: base.lhs_promotion_path.clone(),
        };
        assert_ne!(swapped.config_id(), base.config_id());
    }

    /// An edge's exactness is part of its identity, so a lossy conversion can
    /// never be silently substituted for an exact embedding over the same
    /// generator (ADR-0015 ⟨D-PROMOTE⟩).
    #[test]
    fn coercion_exactness_is_bound_into_the_edge_identity() {
        let exact = edge("type.rule.num.promote.Int_Float@1", CoercionKind::Exact);
        let lossy = edge("type.rule.num.promote.Int_Float@1", CoercionKind::Lossy);
        assert_ne!(exact.canon_bytes(), lossy.canon_bytes());
    }

    /// A promotion path is ordered: the same two edges in the other order is a
    /// different path, which is why the encoding uses `write_list`.
    #[test]
    fn promotion_path_order_is_significant() {
        let a = edge("type.rule.num.promote.Nat_Int@1", CoercionKind::Exact);
        let b = edge("type.rule.num.promote.Int_Rat@1", CoercionKind::Exact);

        let forward = ArithTypingInputV1 {
            lhs_promotion_path: vec![a.clone(), b.clone()],
            ..input()
        };
        let reversed = ArithTypingInputV1 {
            lhs_promotion_path: vec![b, a],
            ..input()
        };
        assert_ne!(forward.config_id(), reversed.config_id());
    }

    /// The operand-type vocabulary covers the numeric tower exactly, and
    /// refuses anything else rather than defaulting. A name the kernel does
    /// not recognise means the host and the TCB disagree about what a numeric
    /// type is, which must produce no source object at all.
    #[test]
    fn the_numeric_vocabulary_is_closed_and_fails_closed() {
        let tower = [
            ("Nat", NumericTypeNameV1::Nat),
            ("Int", NumericTypeNameV1::Int),
            ("Rat", NumericTypeNameV1::Rat),
            ("Real", NumericTypeNameV1::Real),
            ("Complex", NumericTypeNameV1::Complex),
            ("Float", NumericTypeNameV1::Float),
        ];
        for (name, want) in tower {
            assert_eq!(NumericTypeNameV1::from_lattice_node(name), Some(want));
        }
        // Distinct ordinals: no two tower nodes share an encoding.
        let mut encodings: Vec<Vec<u8>> = tower.iter().map(|(_, t)| t.canon_bytes()).collect();
        encodings.sort();
        encodings.dedup();
        assert_eq!(encodings.len(), tower.len());

        for name in ["Str", "Bool", "int", "", "Nat "] {
            assert_eq!(
                NumericTypeNameV1::from_lattice_node(name),
                None,
                "{name:?} is not a numeric tower node"
            );
        }
    }

    /// The frozen v1 preimage, re-spelled with primitive `CanonWriter` calls
    /// and literal constants rather than by importing the ones the encoder
    /// uses (`crates/brix-kernel/OWNER.md`: a guard that shares its subject's
    /// constants can be vacuously satisfied).
    #[test]
    fn the_v1_preimage_is_marker_version_operator_types_paths() {
        let value = input();

        let mut w = CanonWriter::new();
        w.write_bytes(b"brix.kernel.arith-typing-input");
        w.write_uint(1);
        w.write_enum(0, |_| {}); // ArithOperatorV1::Add
        w.write_enum(1, |_| {}); // NumericTypeNameV1::Int
        w.write_enum(2, |_| {}); // NumericTypeNameV1::Rat
        w.write_list([{
            let mut e = CanonWriter::new();
            e.write_bytes(
                GeneratorId::named("type.rule.num.promote.Int_Rat@1")
                    .digest()
                    .as_bytes(),
            );
            e.write_enum(0, |_| {}); // CoercionKind::Exact
            e.finish()
        }]);
        w.write_list(Vec::new());

        assert_eq!(value.canon_bytes(), w.finish());
    }

    /// The frozen v1 preimage of the destination object, re-spelled with
    /// primitive `CanonWriter` calls and literal constants for the same reason
    /// as the source object's.
    #[test]
    fn the_result_v1_preimage_is_marker_version_name() {
        let value = NumericResultTypeV1 {
            name: NumericTypeNameV1::Float,
        };

        let mut w = CanonWriter::new();
        w.write_bytes(b"brix.kernel.numeric-result-type");
        w.write_uint(1);
        w.write_enum(5, |_| {}); // NumericTypeNameV1::Float

        assert_eq!(value.canon_bytes(), w.finish());
    }

    /// A source object and a destination object must never collide, even when
    /// both are "about" the same numeric type. This is what the distinct
    /// markers buy: a relation matched by exact byte membership would otherwise
    /// be able to confuse its two endpoints.
    #[test]
    fn source_and_destination_objects_are_domain_separated() {
        let result = NumericResultTypeV1 {
            name: NumericTypeNameV1::Int,
        };
        let input = ArithTypingInputV1 {
            operator: ArithOperatorV1::Add,
            lhs_type: NumericTypeNameV1::Int,
            rhs_type: NumericTypeNameV1::Int,
            lhs_promotion_path: Vec::new(),
            rhs_promotion_path: Vec::new(),
        };
        assert_ne!(result.canon_bytes(), input.canon_bytes());
        assert_ne!(result.config_id(), input.config_id());

        // …and the bare operand-type name is not a result object either.
        assert_ne!(result.canon_bytes(), NumericTypeNameV1::Int.canon_bytes());
    }

    /// Every result type in the tower is a distinct destination object, so a
    /// row concluding `Int` can never be satisfied by a `Float` conclusion.
    #[test]
    fn every_result_type_is_a_distinct_destination_object() {
        let all = [
            NumericTypeNameV1::Nat,
            NumericTypeNameV1::Int,
            NumericTypeNameV1::Rat,
            NumericTypeNameV1::Real,
            NumericTypeNameV1::Complex,
            NumericTypeNameV1::Float,
        ];
        let mut ids: Vec<_> = all
            .iter()
            .map(|name| NumericResultTypeV1 { name: *name }.config_id())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }

    /// A `SchemaId` is derived from the schema's own frozen header, so the two
    /// schemas have distinct ids and neither can be reproduced from the other's
    /// constants.
    #[test]
    fn schema_ids_are_the_frozen_header_digest() {
        assert_ne!(
            arith_typing_input_schema_id(),
            numeric_result_type_schema_id()
        );

        // Independent reproduction: the two writes spelled out with literals
        // rather than the exported constants.
        let mut w = CanonWriter::new();
        w.write_bytes(b"brix.kernel.numeric-result-type");
        w.write_uint(1);
        let independent = Digest::of(Domain::Value, &w.finish());
        assert_eq!(numeric_result_type_schema_id().digest(), independent);
    }

    /// A version bump moves the schema id. This is the mechanism that makes
    /// ADR-0015 §7's "relation identities are immutable" structural rather than
    /// a discipline: a v2 source schema cannot leave `TypingArithV1`'s id alone.
    #[test]
    fn a_schema_version_bump_moves_the_schema_id() {
        assert_ne!(
            SchemaId::of_schema(ARITH_TYPING_INPUT_MARKER_V1, 1),
            SchemaId::of_schema(ARITH_TYPING_INPUT_MARKER_V1, 2)
        );
    }
}
