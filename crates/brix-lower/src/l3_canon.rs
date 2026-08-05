//! ADR-0012 §3.1–§3.3 — canonical encoders and frozen identities for the
//! constituents of [`crate::l3::L3PlanV1`].
//!
//! This module is the second half of L3 Stage A (ADR-0012 §11 blocker 2):
//! [`crate::l3::lower_l3_plan`] validates a module into structurally
//! comparable data with **no identity of any kind**; this module gives that
//! data canonical, content-addressed identity. It depends on `l3`'s types
//! one-way — validation never needs to compute a digest to decide
//! admissibility, and identity never re-validates.
//!
//! # Encoder shape (ADR-0012 §3.1: "the implementation MAY choose a concrete
//! encoder shape, but it MUST freeze it with independent vectors")
//!
//! Every genuinely underdetermined byte-layout choice below is marked at its
//! point of decision with an `// ADR-0012 §3.x:` comment. The two governing
//! precedents this module follows are `soc-core`'s
//! `GeneratorPartitionProfile::canon_preimage` (marker + version + kind +
//! ordered payload) and ADR-0013's certificate envelope (frozen field order,
//! length-framed sub-payloads, an independent second construction path in the
//! vector test). Frozen vectors: `vectors/l3_plan_v1.json`
//! (`crates/brix-lower/tests/l3_stage_a_vectors.rs`).
//!
//! # Reused vs. new identities (ADR-0012 §3.1)
//!
//! `ConfigId`, `GeneratorId`, `WitnessId`, and `RegimeId` are the existing
//! `brix-semantic` types, reused verbatim — this ADR does not repurpose their
//! domains. `ProgramIdV1`, `RuleId`, `L3ValueId`, `PendingIdV1`, and
//! `FactChainIdV1` are new identity types this ADR introduces.
//! `PresentationIdV1` mirrors (but does not import)
//! `soc_core::saturate::PresentationIdV1` — see its own doc comment for why.

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, GeneratorId, RegimeId, WitnessId};

use crate::l3::{L3ConfigBody, L3PlanItem, L3PlanV1, L3TypeRef, L3ValueV1, PlanLimitsV1};

// ---------------------------------------------------------------------------
// PlanLimitsV1 — Canonical.
// ---------------------------------------------------------------------------

impl Canonical for PlanLimitsV1 {
    /// Frozen field order: the struct's own declared order, which ADR-0012
    /// §3.3 states "in order" verbatim: `max_selected_rules`,
    /// `max_config_nodes`, `max_total_value_nodes`, `max_total_value_bytes`,
    /// `max_value_depth`, "all canonically encoded as `u64`."
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.max_selected_rules);
        w.write_uint(self.max_config_nodes);
        w.write_uint(self.max_total_value_nodes);
        w.write_uint(self.max_total_value_bytes);
        w.write_uint(self.max_value_depth);
    }
}

// ---------------------------------------------------------------------------
// L3ValueId — ADR-0012 §3.2.
// ---------------------------------------------------------------------------

/// The fixed marker opening an [`L3ValueId`] preimage.
///
/// ADR-0012 §3.2 requires `L3ValueId` to be "domain-separated" but does not
/// pin its byte tag (unlike `ProgramIdV1`'s literal `"brix.l3.plan"`, §3.1).
/// This module follows the plan preimage's own marker+version house style
/// (also `GeneratorPartitionProfile::canon_preimage`'s) rather than folding a
/// version suffix into a tag string, for one uniform convention across every
/// identity this module mints from scratch.
pub const L3_VALUE_MARKER: &[u8] = b"brix.l3.value";
/// Frozen format version for [`L3ValueId`]'s preimage.
pub const L3_VALUE_FORMAT_V1: u64 = 1;

/// Content-addressed, domain-separated identity of a closed [`L3ValueV1`]
/// (ADR-0012 §3.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct L3ValueId(pub Digest);

impl L3ValueId {
    /// Hash an already-built preimage under the value domain.
    pub fn from_canon(payload: &[u8]) -> Self {
        L3ValueId(Digest::of(Domain::Value, payload))
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

impl Canonical for L3ValueId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// Write one [`L3ValueV1`], recursively.
///
/// ADR-0012 §3.2: the `L3ValueV1` enum-tag order is not pinned by the ADR
/// text (only the four constructors and their normalization rules are); this
/// module fixes `Int = 0, Str = 1, Record = 2, NullaryVariant = 3` in the
/// constructors' declared order and freezes it here.
fn write_l3_value(w: &mut CanonWriter, value: &L3ValueV1) {
    match value {
        // Decoded integer value (ADR-0012 §3.2: "decoded to their semantic
        // values before canonical encoding, so spelling ... differences do
        // not create distinct facts").
        L3ValueV1::Int(i) => w.write_enum(0, |w| w.write_int(*i)),
        // Decoded string value, raw Unicode scalars (`write_str`, not
        // `write_ident` — this is a value, not an identifier, so it must not
        // be NFC-folded).
        L3ValueV1::Str(s) => w.write_enum(1, |w| w.write_str(s)),
        L3ValueV1::Record {
            nominal_config,
            fields,
        } => w.write_enum(2, |w| {
            w.write_ident(nominal_config);
            // Declaration order is already established by the normalizer
            // (ADR-0012 §3.2); `write_list` preserves the given order rather
            // than re-sorting it the way `write_record` would.
            w.write_list(fields.iter().map(|(name, value)| {
                let mut fw = CanonWriter::new();
                fw.write_ident(name);
                write_l3_value(&mut fw, value);
                fw.finish()
            }));
        }),
        L3ValueV1::NullaryVariant {
            nominal_sum,
            variant,
        } => w.write_enum(3, |w| {
            w.write_ident(nominal_sum);
            w.write_ident(variant);
        }),
    }
}

/// The frozen [`L3ValueId`] preimage: marker, format version, the value.
pub fn l3_value_preimage(value: &L3ValueV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(L3_VALUE_MARKER);
    w.write_uint(L3_VALUE_FORMAT_V1);
    write_l3_value(&mut w, value);
    w.finish()
}

/// The canonical identity of a closed [`L3ValueV1`] (ADR-0012 §3.2).
pub fn l3_value_id(value: &L3ValueV1) -> L3ValueId {
    L3ValueId::from_canon(&l3_value_preimage(value))
}

// ---------------------------------------------------------------------------
// ProgramIdV1 — ADR-0012 §3.1.
// ---------------------------------------------------------------------------

/// The fixed marker opening a `ProgramIdV1` preimage (ADR-0012 §3.1 point 1,
/// pinned verbatim: "the fixed marker `brix.l3.plan`").
pub const L3_PLAN_MARKER: &[u8] = b"brix.l3.plan";
/// The frozen format version (ADR-0012 §3.1 point 2, pinned verbatim: "format
/// version `1`").
pub const L3_PLAN_FORMAT_V1: u64 = 1;

/// Content-addressed identity of an [`L3PlanV1`] revision (ADR-0012 §3.1):
/// "the exact program-revision identity for this profile ... it identifies
/// the executable normalized plan."
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProgramIdV1(pub Digest);

impl ProgramIdV1 {
    /// Hash an already-built preimage under the value domain.
    pub fn from_canon(payload: &[u8]) -> Self {
        ProgramIdV1(Digest::of(Domain::Value, payload))
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

impl Canonical for ProgramIdV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// Encode one tagged plan item (ADR-0012 §3.1 point 4).
///
/// ADR-0012 §3.1 pins only "an item tag followed by" each kind's payload, not
/// the ordinal values; this module fixes `Config = 0, Let = 1, Rule = 2` (the
/// order the three item kinds are introduced in §1/§3.1's fragment
/// description) and freezes it here. Keeping every item — regardless of kind
/// — in one `write_list` in exact `Module.items` order (rather than, say,
/// three separate per-kind lists) is precisely what makes moving a
/// declaration across item kinds change the plan revision (§3.1).
fn encode_item(item: &L3PlanItem) -> Vec<u8> {
    let mut w = CanonWriter::new();
    match item {
        L3PlanItem::Config(decl) => w.write_enum(0, |w| {
            w.write_ident(&decl.name);
            write_config_body(w, &decl.body);
        }),
        L3PlanItem::Let { name, value } => w.write_enum(1, |w| {
            w.write_ident(name);
            w.write_bytes(l3_value_id(value).digest().as_bytes());
        }),
        L3PlanItem::Rule {
            ordinal,
            name,
            value,
        } => w.write_enum(2, |w| {
            w.write_uint(*ordinal);
            w.write_ident(name);
            w.write_bytes(l3_value_id(value).digest().as_bytes());
        }),
    }
    w.finish()
}

/// Write an [`L3ConfigBody`] in declaration order.
///
/// ADR-0012 §3.1 does not pin `Record`/`Sum` ordinals; this module fixes
/// `Record = 0, Sum = 1` (declared enum order) and freezes it here.
fn write_config_body(w: &mut CanonWriter, body: &L3ConfigBody) {
    match body {
        L3ConfigBody::Record(fields) => w.write_enum(0, |w| {
            // Field declaration order, never re-sorted (ADR-0012 §3.1:
            // "within a record config, fields use field declaration order").
            w.write_list(fields.iter().map(|(name, ty)| {
                let mut fw = CanonWriter::new();
                fw.write_ident(name);
                write_type_ref(&mut fw, ty);
                fw.finish()
            }));
        }),
        L3ConfigBody::Sum(variants) => w.write_enum(1, |w| {
            // Variant declaration order, and payload types in parameter
            // order (ADR-0012 §3.1).
            w.write_list(variants.iter().map(|(name, params)| {
                let mut vw = CanonWriter::new();
                vw.write_ident(name);
                vw.write_list(params.iter().map(|ty| {
                    let mut tw = CanonWriter::new();
                    write_type_ref(&mut tw, ty);
                    tw.finish()
                }));
                vw.finish()
            }));
        }),
    }
}

/// Write an [`L3TypeRef`].
///
/// ADR-0012 §3.1 does not pin the ordinal; this module fixes
/// `Int = 0, Str = 1, Config = 2` (declared enum order) and freezes it here.
fn write_type_ref(w: &mut CanonWriter, ty: &L3TypeRef) {
    match ty {
        L3TypeRef::Int => w.write_enum(0, |_| {}),
        L3TypeRef::Str => w.write_enum(1, |_| {}),
        L3TypeRef::Config(name) => w.write_enum(2, |w| w.write_ident(name)),
    }
}

/// The frozen `ProgramIdV1` preimage (ADR-0012 §3.1): marker, format version,
/// the fixed execution-profile marker, then the tagged item stream in exact
/// `Module.items` order.
///
/// The profile marker is written with `write_str`, not `write_ident`
/// (ADR-0013 §2's precedent: "a profile name is a *value*, not an
/// identifier"). Checked source type/grade annotations are absent from
/// `L3PlanItem` already (the `l3` validator erases them before this encoder
/// ever runs), so a satisfied annotation cannot perturb this preimage.
pub fn program_preimage(plan: &L3PlanV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(L3_PLAN_MARKER);
    w.write_uint(L3_PLAN_FORMAT_V1);
    w.write_str(&plan.profile);
    w.write_list(plan.items.iter().map(encode_item));
    w.finish()
}

/// The canonical identity of a validated [`L3PlanV1`] (ADR-0012 §3.1).
pub fn program_id(plan: &L3PlanV1) -> ProgramIdV1 {
    ProgramIdV1::from_canon(&program_preimage(plan))
}

// ---------------------------------------------------------------------------
// RuleId — ADR-0012 §3.1, pinned verbatim.
// ---------------------------------------------------------------------------

/// The frozen tag for `RuleId`'s preimage (ADR-0012 §3.1, pinned verbatim:
/// `("brix.l3.rule@1", ProgramIdV1, i, name)`).
pub const L3_RULE_TAG: &str = "brix.l3.rule@1";

/// Content-addressed identity of a selected rule at its module-order ordinal
/// (ADR-0012 §3.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RuleId(pub Digest);

impl RuleId {
    /// Hash an already-built preimage under the value domain.
    pub fn from_canon(payload: &[u8]) -> Self {
        RuleId(Digest::of(Domain::Value, payload))
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

impl Canonical for RuleId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// The frozen `RuleId` preimage (ADR-0012 §3.1): `write_tag` for the fixed
/// tag string (a *named* tag whose identity is the name itself — exactly
/// `CanonWriter::write_tag`'s documented use), then `program`, `ordinal`,
/// `name` in the pinned tuple order.
pub fn rule_preimage(program: ProgramIdV1, ordinal: u64, name: &str) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag(L3_RULE_TAG);
    w.write_bytes(program.digest().as_bytes());
    w.write_uint(ordinal);
    w.write_ident(name);
    w.finish()
}

/// The canonical identity of rule `name` at module-order ordinal `ordinal`
/// under program `program` (ADR-0012 §3.1).
pub fn rule_id(program: ProgramIdV1, ordinal: u64, name: &str) -> RuleId {
    RuleId::from_canon(&rule_preimage(program, ordinal, name))
}

// ---------------------------------------------------------------------------
// GeneratorId / WitnessId — reused `brix-semantic` types (ADR-0012 §3.1,
// pinned verbatim).
// ---------------------------------------------------------------------------

/// The frozen tag for the L3 generator preimage (ADR-0012 §3.1, pinned
/// verbatim: `("brix.l3.generator@1", ProgramIdV1, RuleId, src, dst)`).
pub const L3_GENERATOR_TAG: &str = "brix.l3.generator@1";

/// The frozen L3 generator preimage.
pub fn l3_generator_preimage(
    program: ProgramIdV1,
    rule: RuleId,
    src: ConfigId,
    dst: ConfigId,
) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag(L3_GENERATOR_TAG);
    w.write_bytes(program.digest().as_bytes());
    w.write_bytes(rule.digest().as_bytes());
    w.write_bytes(src.digest().as_bytes());
    w.write_bytes(dst.digest().as_bytes());
    w.finish()
}

/// The canonical identity of the one generator `g(program, rule)` witnessing
/// the transition `src -> dst` (ADR-0012 §3.1). Reuses `brix_semantic::
/// GeneratorId` verbatim — this ADR does not repurpose its domain.
pub fn l3_generator_id(
    program: ProgramIdV1,
    rule: RuleId,
    src: ConfigId,
    dst: ConfigId,
) -> GeneratorId {
    GeneratorId::from_canon(&l3_generator_preimage(program, rule, src, dst))
}

/// The committed witness identity a generator's decomposition proposes
/// (ADR-0012 §3.1: "the candidate's witness handle interns that generator's
/// primitive `WitnessId`"). A generator *is* a primitive witness — this
/// reuses `GeneratorId::witness_id` verbatim; no new encoder.
pub fn l3_witness_id(generator: GeneratorId) -> WitnessId {
    generator.witness_id()
}

// ---------------------------------------------------------------------------
// World, facts, and their fixed-size chain identities — ADR-0012 §3.3.
// ---------------------------------------------------------------------------

/// The fixed marker for a [`PendingIdV1`] node (ADR-0012 §3.3: "every node
/// ... uses its own domain/version marker"). The byte tag itself is not
/// pinned by the ADR text; chosen here.
pub const L3_PENDING_MARKER: &[u8] = b"brix.l3.pending";
/// Frozen format version for [`PendingIdV1`] nodes.
pub const L3_PENDING_FORMAT_V1: u64 = 1;

/// Fixed-size canonical identity of a `PendingV1` node (ADR-0012 §3.3): a
/// node's id depends only on its own head and its tail's *identity* — never
/// on the tail's full contents — which is what makes transition identity
/// O(1) rather than re-encoding the whole suffix on every commit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PendingIdV1(pub Digest);

impl PendingIdV1 {
    /// `PendingV1::Empty`'s fixed identity.
    ///
    /// Not pinned by the ADR text; `Empty = 0, Cons = 1` (declared
    /// constructor order) is this module's choice, frozen by the vectors.
    pub fn empty() -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(L3_PENDING_MARKER);
        w.write_uint(L3_PENDING_FORMAT_V1);
        w.write_enum(0, |_| {});
        PendingIdV1(Digest::of(Domain::Value, &w.finish()))
    }

    /// `PendingV1::Cons { rule, tail }`'s identity, from `rule` and the
    /// *identity* of `tail` (not `tail`'s own contents).
    pub fn cons(rule: RuleId, tail: PendingIdV1) -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(L3_PENDING_MARKER);
        w.write_uint(L3_PENDING_FORMAT_V1);
        w.write_enum(1, |w| {
            w.write_bytes(rule.digest().as_bytes());
            w.write_bytes(tail.0.as_bytes());
        });
        PendingIdV1(Digest::of(Domain::Value, &w.finish()))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }
}

impl Canonical for PendingIdV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// The fixed marker for a [`FactChainIdV1`] node. Not pinned by the ADR
/// text; chosen here.
pub const L3_FACT_CHAIN_MARKER: &[u8] = b"brix.l3.fact-chain";
/// Frozen format version for [`FactChainIdV1`] nodes.
pub const L3_FACT_CHAIN_FORMAT_V1: u64 = 1;

/// Fixed-size canonical identity of a `FactChainV1` node (ADR-0012 §3.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FactChainIdV1(pub Digest);

impl FactChainIdV1 {
    /// `FactChainV1::Genesis`'s versioned genesis digest (ADR-0012 §3.3:
    /// "starts the fact chain at its versioned genesis digest").
    ///
    /// `Genesis = 0, Append = 1` (declared constructor order) is this
    /// module's choice, mirroring [`PendingIdV1`]'s.
    pub fn genesis() -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(L3_FACT_CHAIN_MARKER);
        w.write_uint(L3_FACT_CHAIN_FORMAT_V1);
        w.write_enum(0, |_| {});
        FactChainIdV1(Digest::of(Domain::Value, &w.finish()))
    }

    /// `FactChainV1::Append { prior, fact }`'s identity, from the *identity*
    /// of `prior` and `fact`'s identity (the reused `ConfigId` from
    /// [`fact_id`]).
    pub fn append(prior: FactChainIdV1, fact: ConfigId) -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(L3_FACT_CHAIN_MARKER);
        w.write_uint(L3_FACT_CHAIN_FORMAT_V1);
        w.write_enum(1, |w| {
            w.write_bytes(prior.0.as_bytes());
            w.write_bytes(fact.digest().as_bytes());
        });
        FactChainIdV1(Digest::of(Domain::Value, &w.finish()))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }
}

impl Canonical for FactChainIdV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// `FactV1 { rule, payload }` (ADR-0012 §3.3): "a fact binds the publishing
/// `RuleId` and the canonical identity of the static normalized body; it is
/// not an untyped print string or a claim of general evaluation."
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FactV1 {
    pub rule: RuleId,
    pub payload: L3ValueId,
}

impl Canonical for FactV1 {
    /// Frozen field order (`rule`, `payload`); no additional marker beyond
    /// that — this follows `brix_semantic::Witness`'s precedent of relying on
    /// frozen field order plus a wrapping identity type rather than every
    /// small record minting its own domain tag.
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.rule.digest().as_bytes());
        w.write_bytes(self.payload.digest().as_bytes());
    }
}

/// The canonical identity of a fact — reuses `ConfigId` verbatim (ADR-0012
/// §3.1: "Existing `ConfigId` ... encoders are reused; this ADR does not
/// repurpose their domains").
pub fn fact_id(fact: &FactV1) -> ConfigId {
    ConfigId::of(fact)
}

/// `L3WorldV1 { program, pending, facts, fact_count }` (ADR-0012 §3.3),
/// pinned field order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct L3WorldV1 {
    pub program: ProgramIdV1,
    pub pending: PendingIdV1,
    pub facts: FactChainIdV1,
    pub fact_count: u64,
}

/// The fixed marker for an [`L3WorldV1`] (ADR-0012 §3.3: "every ... world
/// uses its own domain/version marker"). Not pinned by the ADR text; chosen
/// here.
pub const L3_WORLD_MARKER: &[u8] = b"brix.l3.world";
/// Frozen format version for [`L3WorldV1`].
pub const L3_WORLD_FORMAT_V1: u64 = 1;

impl Canonical for L3WorldV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(L3_WORLD_MARKER);
        w.write_uint(L3_WORLD_FORMAT_V1);
        w.write_bytes(self.program.digest().as_bytes());
        w.write_bytes(self.pending.digest().as_bytes());
        w.write_bytes(self.facts.digest().as_bytes());
        w.write_uint(self.fact_count);
    }
}

/// The canonical identity of a world — reuses `ConfigId` verbatim (ADR-0012
/// §3.3: "`ExecConfig.world` interns the canonical `ConfigId` of this
/// world").
pub fn world_id(world: &L3WorldV1) -> ConfigId {
    ConfigId::of(world)
}

/// Build the duplicate-free pending suffix chain in reverse module order
/// (ADR-0012 §3.3: "the runner builds the duplicate-free pending suffix
/// chain in reverse module order"), so the resulting chain's *head* is the
/// first rule to become eligible.
pub fn build_pending(rules: &[RuleId]) -> PendingIdV1 {
    let mut chain = PendingIdV1::empty();
    for rule in rules.iter().rev() {
        chain = PendingIdV1::cons(*rule, chain);
    }
    chain
}

// ---------------------------------------------------------------------------
// Policy — ADR-0012 §3.4. Not this slice's own section, but §9 Stage A's
// acceptance list requires a frozen "one ... policy" vector, and §3.4's shape
// is fully pinned, so it is implemented here rather than left unaddressed.
// ---------------------------------------------------------------------------

/// `L3PolicyV1` (ADR-0012 §3.4): "a canonical immutable envelope containing
/// the plan identity, the fixed profile marker, and the one compiler-owned
/// regime identity," in that pinned order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3PolicyV1 {
    pub plan: ProgramIdV1,
    pub profile: String,
    pub regime: RegimeId,
}

impl Canonical for L3PolicyV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.plan.digest().as_bytes());
        w.write_str(&self.profile);
        w.write_bytes(self.regime.digest().as_bytes());
    }
}

/// The canonical identity of a policy — reuses `ConfigId` verbatim (ADR-0012
/// §3.4: "`PresentationV1.adm_id` is the canonical digest of this
/// envelope").
pub fn policy_id(policy: &L3PolicyV1) -> ConfigId {
    ConfigId::of(policy)
}

// ---------------------------------------------------------------------------
// RunContextV1 — ADR-0012 §3.3, pinned field order.
// ---------------------------------------------------------------------------

/// `RunContextV1` (ADR-0012 §3.3): "a required canonical envelope with, in
/// order: format version, `ProgramIdV1` (the program revision), initial-world
/// identity, exact policy identity, exact profile marker, and
/// `PlanLimitsV1`." Deliberately excludes `SaturationBudget` (⟨D-LIM⟩).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunContextV1 {
    pub program: ProgramIdV1,
    pub initial_world: ConfigId,
    pub policy: ConfigId,
    pub profile: String,
    pub limits: PlanLimitsV1,
}

/// The frozen format version for [`RunContextV1`]'s envelope.
pub const L3_RUN_CONTEXT_FORMAT_V1: u64 = 1;

impl Canonical for RunContextV1 {
    /// ADR-0012 §3.3 pins the field list starting at "format version" with no
    /// leading marker byte-string (unlike `ProgramIdV1`'s explicit
    /// `"brix.l3.plan"` marker, §3.1). This encoder follows that literally:
    /// no additional marker is inserted ahead of the pinned list, since the
    /// ADR's own field enumeration is normative here.
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(L3_RUN_CONTEXT_FORMAT_V1);
        w.write_bytes(self.program.digest().as_bytes());
        w.write_bytes(self.initial_world.digest().as_bytes());
        w.write_bytes(self.policy.digest().as_bytes());
        w.write_str(&self.profile);
        self.limits.canon_write(w);
    }
}

/// The `ContextId` passed to commit/audit (ADR-0012 §3.3): "The `ContextId`
/// passed to commit/audit is derived from this complete envelope;
/// `ContextId::root()` is not a valid public L3-run substitute." Reuses
/// `ContextId` verbatim.
pub fn context_id(context: &RunContextV1) -> ContextId {
    ContextId::of(context)
}

// ---------------------------------------------------------------------------
// PresentationIdV1 — ADR-0012 §3.1, pinned verbatim.
// ---------------------------------------------------------------------------

/// A local mirror of `soc_core::saturate::PresentationIdV1`.
///
/// ADR-0012 §3.1 pins `PresentationIdV1 = PresentationIdV1(ProgramIdV1.
/// digest())` — "one revision identity, not two." The canonical *home* of
/// that wrapper type is `soc-core` (ADR-0014), which stepping and
/// certificates consume as an opaque caller-supplied identity. Stage A does
/// not construct a `saturate::PresentationV1` — that is Stage C's driver — so
/// rather than pull the whole `soc-core` surface into this validation-only
/// crate for one newtype, this module defines a structurally identical local
/// wrapper (`PresentationIdV1(Digest)`, same representation, same derivation
/// rule, same field visibility). Stage C's adapter is expected to construct
/// `soc_core::saturate::PresentationIdV1(program_id.digest())` directly at
/// the driver boundary; the two wrappers hold the same digest by the same
/// rule, so nothing is lost by not sharing the type across the crate
/// boundary this early, and nothing here forecloses Stage C's choice.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PresentationIdV1(pub Digest);

impl PresentationIdV1 {
    /// The one revision identity for `program` (ADR-0012 §3.1).
    pub fn of_program(program: ProgramIdV1) -> Self {
        PresentationIdV1(program.digest())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l3::{lower_l3_plan, L3_PROFILE_MARKER_V1};
    use brix_syntax::parse;

    fn plan(src: &str) -> L3PlanV1 {
        let module = parse(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .unwrap_or_else(|e| panic!("lowering failed: {e:?}"))
    }

    #[test]
    fn identical_plans_give_identical_program_ids() {
        let a = plan("rule r() = 1\n");
        let b = plan("rule r() = 1\n");
        assert_eq!(program_id(&a), program_id(&b));
    }

    #[test]
    fn distinct_plans_give_distinct_program_ids() {
        let a = plan("rule r() = 1\n");
        let b = plan("rule r() = 2\n");
        assert_ne!(program_id(&a), program_id(&b));
    }

    #[test]
    fn reordered_top_level_items_give_distinct_program_ids() {
        // Same two declarations, opposite relative order: this must not
        // collapse under an encoder that (incorrectly) grouped items by kind
        // instead of preserving `Module.items` order (ADR-0012 §3.1).
        let a = plan("let a = 1\nconfig Item = { x: Int }\n");
        let b = plan("config Item = { x: Int }\nlet a = 1\n");
        assert_ne!(program_id(&a), program_id(&b));
    }

    #[test]
    fn equivalent_int_spellings_give_the_same_l3_value_id() {
        // `007` and `7` are the same `i64` once `l3.rs`'s normalizer decodes
        // them (ADR-0012 §3.2/§9); assert the identity-level consequence
        // rather than just the structural one `l3_stage_a.rs` already checks.
        let a = plan("let a = 007\n");
        let b = plan("let a = 7\n");
        let value = |p: &L3PlanV1| match &p.items[0] {
            L3PlanItem::Let { value, .. } => value.clone(),
            other => panic!("expected Let, got {other:?}"),
        };
        assert_eq!(value(&a), L3ValueV1::Int(7));
        assert_eq!(l3_value_id(&value(&a)), l3_value_id(&value(&b)));
    }

    #[test]
    fn reordered_record_literal_fields_give_the_same_l3_value_id() {
        // Source-order differs; declaration order (established by `l3.rs`'s
        // normalizer, not by this encoder) does not, so the two lowered
        // values must hash to the same `L3ValueId` (ADR-0012 §3.2/§9).
        let a = plan(
            r#"
                config Item = { name: Str, base: Int }
                let widget = Item { name: "widget", base: 10 }
            "#,
        );
        let b = plan(
            r#"
                config Item = { name: Str, base: Int }
                let widget = Item { base: 10, name: "widget" }
            "#,
        );
        let widget_value = |p: &L3PlanV1| match &p.items[1] {
            L3PlanItem::Let { value, .. } => value.clone(),
            other => panic!("expected Let, got {other:?}"),
        };
        assert_eq!(
            l3_value_id(&widget_value(&a)),
            l3_value_id(&widget_value(&b))
        );
    }

    #[test]
    fn same_shape_records_from_different_configs_do_not_collapse() {
        let a = L3ValueV1::Record {
            nominal_config: "Item".to_string(),
            fields: vec![("x".to_string(), L3ValueV1::Int(1))],
        };
        let b = L3ValueV1::Record {
            nominal_config: "Other".to_string(),
            fields: vec![("x".to_string(), L3ValueV1::Int(1))],
        };
        assert_ne!(l3_value_id(&a), l3_value_id(&b));
    }

    #[test]
    fn same_named_variants_from_different_sums_do_not_collapse() {
        let a = L3ValueV1::NullaryVariant {
            nominal_sum: "A".to_string(),
            variant: "X".to_string(),
        };
        let b = L3ValueV1::NullaryVariant {
            nominal_sum: "B".to_string(),
            variant: "X".to_string(),
        };
        assert_ne!(l3_value_id(&a), l3_value_id(&b));
    }

    #[test]
    fn presentation_id_is_the_program_digest() {
        let p = plan("rule r() = 1\n");
        let program = program_id(&p);
        assert_eq!(
            PresentationIdV1::of_program(program),
            PresentationIdV1(program.digest())
        );
    }

    #[test]
    fn satisfied_declared_type_does_not_change_program_id() {
        let with_annotation = plan("let a: Int = 1\n");
        let without_annotation = plan("let a = 1\n");
        assert_eq!(
            program_id(&with_annotation),
            program_id(&without_annotation)
        );
    }

    #[test]
    fn pending_chain_head_is_the_first_module_order_rule() {
        let r0 = rule_id(ProgramIdV1(Digest::of(Domain::Value, b"p")), 0, "r0");
        let r1 = rule_id(ProgramIdV1(Digest::of(Domain::Value, b"p")), 1, "r1");
        let pending = build_pending(&[r0, r1]);
        let expected = PendingIdV1::cons(r0, PendingIdV1::cons(r1, PendingIdV1::empty()));
        assert_eq!(pending, expected);
    }

    #[test]
    fn empty_and_genesis_are_stable() {
        assert_eq!(PendingIdV1::empty(), PendingIdV1::empty());
        assert_eq!(FactChainIdV1::genesis(), FactChainIdV1::genesis());
    }

    #[test]
    fn equivalent_string_escapes_give_the_same_l3_value_id() {
        // Mirrors `l3_stage_a.rs`'s
        // `equivalent_int_and_string_spellings_normalize_to_the_same_value`
        // (structural equality) at the identity level: a `\n` escape and a
        // literal embedded newline byte decode to the same `String`
        // (ADR-0012 §3.2/§9).
        let escaped = plan("let b = \"line\\n\"\n");
        let literal = plan("let b = \"line\n\"\n");
        let value = |p: &L3PlanV1| match &p.items[0] {
            L3PlanItem::Let { value, .. } => value.clone(),
            other => panic!("expected Let, got {other:?}"),
        };
        assert_eq!(value(&escaped), L3ValueV1::Str("line\n".to_string()));
        assert_eq!(l3_value_id(&value(&escaped)), l3_value_id(&value(&literal)));
    }

    #[test]
    fn changed_config_declaration_changes_program_id() {
        let a = plan("config Item = { name: Str }\n");
        let b = plan("config Item = { name: Str, extra: Int }\n");
        assert_ne!(program_id(&a), program_id(&b));
    }

    #[test]
    fn changed_let_binding_changes_program_id() {
        let a = plan("let a = 1\n");
        let b = plan("let a = 2\n");
        assert_ne!(program_id(&a), program_id(&b));
    }

    #[test]
    fn moving_a_declaration_across_item_kinds_changes_program_id() {
        // The same name bound to the same normalized value (`Int(1)`), once
        // as a `let` and once as a `rule` — ADR-0012 §3.1's single tagged
        // item stream must distinguish these, not silently regroup by kind.
        let as_let = L3PlanV1 {
            profile: L3_PROFILE_MARKER_V1.to_string(),
            items: vec![L3PlanItem::Let {
                name: "a".to_string(),
                value: L3ValueV1::Int(1),
            }],
            limits: PlanLimitsV1::generous(),
        };
        let as_rule = L3PlanV1 {
            profile: L3_PROFILE_MARKER_V1.to_string(),
            items: vec![L3PlanItem::Rule {
                ordinal: 0,
                name: "a".to_string(),
                value: L3ValueV1::Int(1),
            }],
            limits: PlanLimitsV1::generous(),
        };
        assert_ne!(program_id(&as_let), program_id(&as_rule));
    }
}
