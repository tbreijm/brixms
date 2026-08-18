//! L3 Stage A — rule-fragment validation and plan lowering (ADR-0012).
//!
//! This module implements exactly ADR-0012 §3.1 (plan validation and
//! admissible source) and §3.2 (closed static values), producing an owned,
//! in-memory [`L3PlanV1`]. Canonical identity for these structures
//! (`ProgramIdV1`, `L3ValueId`, `RuleId`, …) lives in the sibling
//! [`crate::l3_canon`] module, which this one deliberately does not import
//! from: validation/lowering and identity are two different concerns, and
//! [`lower_l3_plan`] never needs a digest to decide whether a module is
//! admissible.
//!
//! - **No `Regime`/`IncrementalRegime`, no settlement adapter/driver, no
//!   audit semantics, no CLI.** Those are ADR-0012 Stages B–D and are out of
//!   scope here (ADR-0012 §9).
//!
//! What this module *does* do: validate that a parsed [`ast::Module`] lies in
//! the ADR-0012 §1 fragment (`config` + immutable `let` + zero-argument
//! `rule`, with every rule body a closed static value), and lower it,
//! **in exact `Module.items` order**, to [`L3PlanV1`] — enough owned data for
//! a later slice to build one `Regime`/`IncrementalRegime` without
//! re-validating the source.
//!
//! # Fail-closed choices on genuinely ambiguous ground
//!
//! A few points in ADR-0012 §3.1/§3.2 do not fully specify a case this slice
//! must nonetheless decide. Per instruction, each is resolved by *rejecting*
//! rather than inventing permissive behavior; each is marked at its point of
//! decision with a `// ADR-0012 §3.x:` comment naming the open question.

use std::collections::{BTreeMap, BTreeSet};

use brix_syntax::ast;

/// The only execution profile this slice recognizes (ADR-0012 §1, §3.1,
/// §3.4, ⟨D-PROFILE⟩ in the ADR-0014 re-pin).
///
/// The 2026-08-02 draft named this marker `brix.l3.rule-agenda@1` — the
/// saturation-blind profile that was never implemented. The re-pin retires
/// that identity permanently: it MUST NOT be minted by any implementation
/// (ADR-0012 §1, §10). This is the live marker.
pub const L3_PROFILE_MARKER_V1: &str = "brix.l3.rule-agenda-saturated@1";

/// The retired, saturation-blind profile marker from the 2026-08-02 draft.
/// No build ever emitted it. It is kept only so callers/tests can name it
/// explicitly when checking that it is rejected like any other unknown
/// marker (ADR-0012 §1, §3.1, §10) — it MUST NOT be minted by
/// [`lower_l3_plan`] or any other implementation.
pub const L3_PROFILE_MARKER_RETIRED_V0: &str = "brix.l3.rule-agenda@1";

/// A resolved, closed-static-value type: the payload type of a `let`/`rule`
/// annotation or a config field/variant-parameter type (ADR-0012 §3.1: "the
/// only primitive payload types are `Int` and `Str`").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3TypeRef {
    Int,
    Str,
    /// A nominal config (record or sum) referenced by name.
    Config(String),
}

/// A validated, declaration-ordered config body (ADR-0012 §3.1: "within a
/// record config, fields use field declaration order; within a sum config,
/// variants use variant declaration order and payload types use parameter
/// order").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3ConfigBody {
    Record(Vec<(String, L3TypeRef)>),
    Sum(Vec<(String, Vec<L3TypeRef>)>),
}

/// A validated `config` declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3ConfigDecl {
    pub name: String,
    pub body: L3ConfigBody,
}

/// `L3ValueV1` — the complete executable consequence language (ADR-0012
/// §3.2). Record identity includes the declared nominal config, and
/// `NullaryVariant` identity includes both the declared nominal sum and
/// variant, so structurally identical values from different declarations
/// never collapse under `PartialEq`/`Eq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3ValueV1 {
    Int(i64),
    Str(String),
    Record {
        nominal_config: String,
        /// Declaration order, regardless of the literal's source order
        /// (ADR-0012 §3.2).
        fields: Vec<(String, L3ValueV1)>,
    },
    NullaryVariant {
        nominal_sum: String,
        variant: String,
    },
}

/// One item of the lowered plan, preserving `Module.items` order and kind
/// (ADR-0012 §3.1: "names and ordering are preserved as written in
/// `Module.items`"; moving a declaration across item kinds must change the
/// plan).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3PlanItem {
    Config(L3ConfigDecl),
    Let {
        name: String,
        value: L3ValueV1,
    },
    Rule {
        /// This rule's position among *rules only*, 0-based, in module order
        /// (ADR-0012 §3.1: "rule ordinal, name, and `L3ValueId`").
        ordinal: u64,
        name: String,
        value: L3ValueV1,
    },
}

/// `PlanLimitsV1` — the *semantic* half of ADR-0012 §3.3's two-limit-set
/// split ⟨D-LIM⟩, restricted to the subset this validation-only slice can
/// actually enforce.
///
/// ADR-0012 §3.3 splits the 2026-08-02 draft's single `RunLimitsV1` into two,
/// because they have different identity consequences:
///
/// - **`PlanLimitsV1`** (this struct) decides whether a plan is an
///   admissible executable artifact at all — a plan rejected for exceeding
///   `max_value_depth` is a different question about a different artifact.
///   It is part of `RunContextV1`'s canonical identity.
/// - **`SaturationBudget`** (`max_visible_steps`/`max_hidden_steps`/
///   `max_administrative_states`, replacing the draft's `max_commits`) bounds
///   a saturated stepping loop instead, and is excluded from every canonical
///   identity: folding it into `ContextId` — a field of
///   `QuiescenceCertificateV1` — would give two runs under different
///   *sufficient* budgets two different certificates for the same fact,
///   contradicting ADR-0014 §6.2.
///
/// There is no stepping loop in this slice (no `Regime`, no engine, no
/// journal — see the module doc), so `SaturationBudget` has nothing to bound
/// here. It is intentionally not part of this struct; it belongs to the
/// Stage B/C driver's own request/result types, which this slice does not
/// implement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanLimitsV1 {
    /// Caps the number of `rule` items selected (i.e. present) in the
    /// module.
    pub max_selected_rules: u64,
    /// Caps the running total of config nodes across every `config`
    /// declaration. A config node is one declaration, field, variant, or
    /// variant payload type (ADR-0012 §3.3).
    pub max_config_nodes: u64,
    /// Caps the running total of value-tree nodes across every `let` and
    /// `rule` value, counting each substituted `let` occurrence's full
    /// expanded size again (ADR-0012 §3.3: "including each substituted
    /// occurrence, rather than deduplicating equal identities").
    pub max_total_value_nodes: u64,
    /// Caps the running total of decoded string bytes, with the same
    /// substitution-recounts-in-full accounting as `max_total_value_nodes`.
    pub max_total_value_bytes: u64,
    /// Caps the depth of any single `let`/`rule` value tree (root depth is
    /// one).
    pub max_value_depth: u64,
}

impl PlanLimitsV1 {
    /// Limits wide enough that no ordinary fixture trips them; use this and
    /// override one field to test a specific limit in isolation.
    pub const fn generous() -> Self {
        Self {
            max_selected_rules: u64::MAX,
            max_config_nodes: u64::MAX,
            max_total_value_nodes: u64::MAX,
            max_total_value_bytes: u64::MAX,
            max_value_depth: u64::MAX,
        }
    }
}

/// A fully validated L3 v1 plan (ADR-0012 §3.1). This is plain owned data —
/// no canonical encoding, no identity. Two modules that differ only in
/// whitespace, comments, literal spelling, or record-literal field order
/// lower to `==` plans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3PlanV1 {
    pub profile: String,
    pub items: Vec<L3PlanItem>,
    pub limits: PlanLimitsV1,
}

/// Every distinguishable way a module can fail to lie in the ADR-0012 §1
/// fragment. One variant per rejection reason so tests can assert on the
/// exact failure, per ADR-0012 §6.1 ("no silent omission").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum L3LowerError {
    /// The caller's requested profile does not match
    /// [`L3_PROFILE_MARKER_V1`] (ADR-0012 §3.1: "an unknown or mismatched
    /// profile is rejected before any engine state exists").
    ProfileMismatch { expected: String, found: String },
    /// A top-level `fn` is present. `fn` is parsed surface, never silently
    /// dropped (ADR-0012 §1.4).
    FnItemNotAllowed(String),
    /// A top-level `regime` is present (ADR-0012 §1.4).
    RegimeItemNotAllowed(String),
    /// A top-level `show` is present (ADR-0012 §1.4).
    ShowItemNotAllowed,
    /// A `use` reached lowering. Imports are resolved first; one arriving
    /// here means resolution was skipped.
    UnresolvedImport(String),
    /// A top-level `witness` binding is present (ADR-0012 §1.4).
    WitnessItemNotAllowed(String),
    /// A `rule` was declared with one or more parameters (ADR-0012 §1: "every
    /// selected rule has zero parameters").
    ParameterizedRule(String),
    /// Two top-level `config`/`let`/`rule` items share a name, including
    /// across different item kinds (ADR-0012 §3.1: "duplicate top-level
    /// config/let/rule names and cross-kind collisions are rejected").
    DuplicateItemName(String),
    /// A `config` declares the same field (record) or variant (sum) name
    /// twice.
    DuplicateConfigMember { config: String, member: String },
    /// A type name does not resolve to `Int`, `Str`, or a declared config in
    /// the module (ADR-0012 §3.1).
    UnknownTypeName(String),
    /// A recognized but non-`Int`/`Str` primitive type name (e.g. `Float`)
    /// was used where only `Int`/`Str` are legal payload types.
    UnsupportedPrimitiveType(String),
    /// An anonymous record type (`{ a: Int }`), which reaches this profile only
    /// as the desugaring of a named-field sum variant. Not part of the v1
    /// executable fragment.
    AnonymousRecordTypeNotAllowed,
    /// A `config`, direct or indirect, cyclically references itself through
    /// nominal field/variant-parameter types (ADR-0012 §3.1: "direct or
    /// mutually recursive nominal configs are rejected in v1"). `cycle` lists
    /// the discovered cycle in traversal order, closing on the repeated name.
    RecursiveConfig { cycle: Vec<String> },
    /// A `@Grade` annotation was found on a `let`/`rule` type annotation or a
    /// config field/variant-parameter type.
    ///
    /// ADR-0012 §3.2: genuine ambiguity. §3.1 says a declared grade
    /// assertion "is checked through the existing L2 grade rules", but §3.2
    /// says in the same breath that "L3 v1 does not invoke the L2 evaluator
    /// or reuse a typing/coverage grade as execution evidence" — and a closed
    /// static value never goes through L2 elaboration in this fragment, so
    /// there is no earned outcome grade to check a `@Grade` assertion
    /// against without invoking exactly the machinery §3.2 forbids. Rather
    /// than silently accept-and-ignore (a silent omission, barred by §6.1)
    /// or fabricate an outcome, this slice rejects any `@Grade` annotation
    /// reachable from the closed-static-value fragment.
    GradeAssertionUnsupported(String),
    /// A variable reference does not resolve to a prior closed `let`, a
    /// nullary constructor, or its own enclosing `let` — i.e. a free
    /// variable (ADR-0012 §3.1/§3.2 call this a "free variable"; §9's
    /// Stage-A fixture list calls the same failure an "unclosed... let
    /// reference"; this slice uses `UnclosedReference` as the one name for
    /// that one failure — see the ambiguity note below).
    ///
    /// ADR-0012 ambiguity: §3.1 enumerates exactly three rejected reference
    /// shapes — "forward references, free variables, and recursive
    /// references" — while §9 lists three differently-named ones —
    /// "unclosed, forward, and recursive let references". Reading "unclosed"
    /// as a Stage-A synonym for "free variable" (a reference that never
    /// closes to a value because it names nothing bindable) is the only
    /// mapping that keeps both lists at exactly three distinct cases; this
    /// slice adopts that reading and fails closed on any reference that
    /// resolves to nothing.
    UnclosedReference(String),
    /// A `let`/`rule` value references a `let` declared *later* in the
    /// module (ADR-0012 §3.1: "forward references ... are rejected").
    ForwardLetReference(String),
    /// A `let` value references its own name (ADR-0012 §3.1: "recursive
    /// references are rejected").
    RecursiveLetReference(String),
    /// A bare or applied name matches a variant declared in more than one
    /// sum config, so which sum it names is ambiguous (ADR-0012 §3.2: "two
    /// ... identically named variants from different sums[] cannot
    /// collapse" — which requires such a name to never resolve silently to
    /// either).
    AmbiguousConstructorReference(String),
    /// `Expr::Call` to something other than a known payload-bearing
    /// constructor (ordinary function/rule call, or an unresolved name) —
    /// rejected outright (ADR-0012 §3.2).
    CallNotAllowed(String),
    /// A payload-bearing constructor call, or a bare reference to a
    /// non-nullary variant (ADR-0012 §3.2: "`Expr::Call` (including
    /// payload-bearing constructors)").
    PayloadBearingConstructor(String),
    /// `Expr::Field` in a rule/let body (ADR-0012 §3.2).
    FieldAccessNotAllowed(String),
    /// `Expr::Match` in a rule/let body (ADR-0012 §3.2).
    MatchNotAllowed,
    /// Arithmetic (`+ - * /`) in a rule/let body (ADR-0012 §3.2).
    ArithmeticNotAllowed,
    /// A comparison (`< <= > >= == !=`) in a rule/let body. Not in the v1
    /// executable fragment, for the same reason arithmetic is not: the profile
    /// assigns no evaluation semantics to rule bodies (ADR-0012 §3.2).
    ComparisonNotAllowed,
    /// A boolean literal in a rule/let body. v1's value grammar is
    /// `Int`/`Str`/nullary-constructor/record only (ADR-0012 §3.2).
    BooleanLiteralNotAllowed,
    /// Witness composition (`then`/`and`) in a rule/let body (ADR-0012 §1.4).
    WitnessCompositionNotAllowed,
    /// `prove` in a rule/let body (ADR-0012 §1.4).
    ProveNotAllowed,
    /// `why(...)` in a rule/let body (ADR-0012 §1.4).
    WhyNotAllowed,
    /// `audit` in a rule/let body (ADR-0012 §1.4).
    AuditNotAllowed,
    /// A numeric literal token that is not an integer (ADR-0012 §3.2:
    /// "Float literals are rejected in this v1 profile").
    FloatLiteralNotAllowed(String),
    /// An integer literal token does not fit in `i64`.
    IntegerOverflow(String),
    /// A record literal is missing a field its declared config requires.
    MissingRecordField { config: String, field: String },
    /// A record literal names a field its declared config does not have.
    UnknownRecordField { config: String, field: String },
    /// A record literal repeats the same field name.
    DuplicateRecordLiteralField { config: String, field: String },
    /// A record literal names a config that is a sum, not a record.
    NotARecordConfig(String),
    /// A `let`/`rule` declared type does not match the value's inferred
    /// type.
    DeclaredTypeMismatch {
        name: String,
        expected: String,
        found: String,
    },
    /// More `rule` items are selected than `PlanLimitsV1::max_selected_rules`
    /// allows.
    RuleCountExceeded { limit: u64, actual: u64 },
    /// The running total of config nodes exceeds
    /// `PlanLimitsV1::max_config_nodes`.
    ConfigNodeLimitExceeded { limit: u64, actual: u64 },
    /// The running total of value nodes exceeds
    /// `PlanLimitsV1::max_total_value_nodes`.
    ValueNodeLimitExceeded { limit: u64, actual: u64 },
    /// The running total of decoded value bytes exceeds
    /// `PlanLimitsV1::max_total_value_bytes`.
    ValueByteLimitExceeded { limit: u64, actual: u64 },
    /// A single value's tree depth exceeds `PlanLimitsV1::max_value_depth`.
    ValueDepthExceeded { limit: u64, actual: u64 },
}

/// Lower and validate a parsed module into an [`L3PlanV1`] under the
/// ADR-0012 `brix.l3.rule-agenda-saturated@1` profile.
///
/// `profile` is the execution profile the caller is compiling under. The
/// current `.brix` grammar ([`ast::Module`]) has no surface syntax for a
/// module to declare its own execution profile — there is no `Item` variant
/// for it.
///
/// ADR-0012 ambiguity (§3.1): "the profile marker is exactly
/// `brix.l3.rule-agenda-saturated@1`; an unknown or mismatched profile is
/// rejected before any engine state exists" presumes a profile marker to
/// compare against, but does not say where a module obtains one absent
/// surface syntax. This slice fails closed by requiring the caller to assert
/// the expected profile explicitly (e.g. a future CLI flag or a later
/// surface pragma would plumb it through here) rather than assuming every
/// module is automatically this profile; passing anything other than
/// [`L3_PROFILE_MARKER_V1`] rejects before any other validation runs —
/// including the retired [`L3_PROFILE_MARKER_RETIRED_V0`], which is rejected
/// the same way as any other unknown marker (ADR-0012 §1, §3.1, §10: it MUST
/// NOT be minted by any implementation).
///
/// `limits` bounds config/value size (ADR-0012 §3.3 `PlanLimitsV1` — the
/// semantic half of the two-limit-set split; see [`PlanLimitsV1`] for why
/// the execution `SaturationBudget` half is not, and cannot be, a parameter
/// here). Limit failures are reported as soon as the relevant running total
/// is known, which in this validation-only slice is always before the
/// function returns a plan at all.
pub fn lower_l3_plan(
    module: &ast::Module,
    profile: &str,
    limits: &PlanLimitsV1,
) -> Result<L3PlanV1, L3LowerError> {
    if profile != L3_PROFILE_MARKER_V1 {
        return Err(L3LowerError::ProfileMismatch {
            expected: L3_PROFILE_MARKER_V1.to_string(),
            found: profile.to_string(),
        });
    }

    // Pass 0: no top-level item outside {config, let, rule} is silently
    // dropped (ADR-0012 §1.4, §6.1).
    for item in &module.items {
        match item {
            ast::Item::Fn(c) => return Err(L3LowerError::FnItemNotAllowed(c.name.clone())),
            ast::Item::Regime(r) => return Err(L3LowerError::RegimeItemNotAllowed(r.name.clone())),
            ast::Item::Show(_) => return Err(L3LowerError::ShowItemNotAllowed),
            // Imports are resolved before lowering, so a `use` reaching here
            // means the caller skipped resolution — refused rather than
            // silently ignored.
            ast::Item::Use(path) => return Err(L3LowerError::UnresolvedImport(path.clone())),
            ast::Item::Witness { name, .. } => {
                return Err(L3LowerError::WitnessItemNotAllowed(name.clone()))
            }
            ast::Item::Config(_) | ast::Item::Let(_) | ast::Item::Rule(_) => {}
        }
    }

    // Pass 1: zero-arg rule check, plus top-level name collisions
    // (cross-kind included).
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    for item in &module.items {
        let name = match item {
            ast::Item::Config(c) => &c.name,
            ast::Item::Let(l) => &l.name,
            ast::Item::Rule(r) => {
                if !r.params.is_empty() {
                    return Err(L3LowerError::ParameterizedRule(r.name.clone()));
                }
                &r.name
            }
            _ => unreachable!("non-{{config,let,rule}} items rejected in pass 0"),
        };
        if !seen_names.insert(name.clone()) {
            return Err(L3LowerError::DuplicateItemName(name.clone()));
        }
    }

    // Fail closed on the rule-count limit before normalizing any rule body.
    let rule_count = module
        .items
        .iter()
        .filter(|it| matches!(it, ast::Item::Rule(_)))
        .count() as u64;
    if rule_count > limits.max_selected_rules {
        return Err(L3LowerError::RuleCountExceeded {
            limit: limits.max_selected_rules,
            actual: rule_count,
        });
    }

    // Pass 2: configs. Config type references resolve in the complete
    // module environment (order-independent existence), but direct/mutual
    // recursion is rejected (ADR-0012 §3.1).
    let mut config_order: Vec<String> = Vec::new();
    let mut raw_configs: BTreeMap<String, &ast::ConfigBody> = BTreeMap::new();
    for item in &module.items {
        if let ast::Item::Config(c) = item {
            config_order.push(c.name.clone());
            raw_configs.insert(c.name.clone(), &c.body);
        }
    }
    let config_names: BTreeSet<String> = config_order.iter().cloned().collect();

    let mut configs: BTreeMap<String, L3ConfigDecl> = BTreeMap::new();
    let mut total_config_nodes: u64 = 0;
    for name in &config_order {
        let body = raw_configs[name];
        total_config_nodes = total_config_nodes.saturating_add(1); // the declaration itself
        let l3_body = match body {
            ast::ConfigBody::Record(fields) => {
                let mut seen_fields: BTreeSet<String> = BTreeSet::new();
                let mut out = Vec::with_capacity(fields.len());
                for f in fields {
                    if !seen_fields.insert(f.name.clone()) {
                        return Err(L3LowerError::DuplicateConfigMember {
                            config: name.clone(),
                            member: f.name.clone(),
                        });
                    }
                    total_config_nodes = total_config_nodes.saturating_add(1);
                    let ty = resolve_ty(&f.ty, &config_names)?;
                    out.push((f.name.clone(), ty));
                }
                L3ConfigBody::Record(out)
            }
            ast::ConfigBody::Sum(variants) => {
                let mut seen_variants: BTreeSet<String> = BTreeSet::new();
                let mut out = Vec::with_capacity(variants.len());
                for v in variants {
                    if !seen_variants.insert(v.name.clone()) {
                        return Err(L3LowerError::DuplicateConfigMember {
                            config: name.clone(),
                            member: v.name.clone(),
                        });
                    }
                    total_config_nodes = total_config_nodes.saturating_add(1);
                    let mut params = Vec::with_capacity(v.params.len());
                    for p in &v.params {
                        total_config_nodes = total_config_nodes.saturating_add(1);
                        params.push(resolve_ty(p, &config_names)?);
                    }
                    out.push((v.name.clone(), params));
                }
                L3ConfigBody::Sum(out)
            }
        };
        if total_config_nodes > limits.max_config_nodes {
            return Err(L3LowerError::ConfigNodeLimitExceeded {
                limit: limits.max_config_nodes,
                actual: total_config_nodes,
            });
        }
        configs.insert(
            name.clone(),
            L3ConfigDecl {
                name: name.clone(),
                body: l3_body,
            },
        );
    }

    detect_recursive_configs(&config_order, &configs)?;

    // Constructor table: variant name -> (nominal sum, arity). A variant
    // name shared by more than one sum is ambiguous and resolves to neither
    // (ADR-0012 §3.2).
    let mut ctors: BTreeMap<String, (String, usize)> = BTreeMap::new();
    let mut ambiguous_ctors: BTreeSet<String> = BTreeSet::new();
    for name in &config_order {
        if let L3ConfigBody::Sum(variants) = &configs[name].body {
            for (vname, params) in variants {
                if ambiguous_ctors.contains(vname) {
                    continue;
                }
                match ctors.get(vname) {
                    Some((existing_sum, _)) if existing_sum != name => {
                        ctors.remove(vname);
                        ambiguous_ctors.insert(vname.clone());
                    }
                    Some(_) => { /* unreachable: dup-within-sum already rejected above */ }
                    None => {
                        ctors.insert(vname.clone(), (name.clone(), params.len()));
                    }
                }
            }
        }
    }

    // Pass 3: lets and rules, in exact module order, so a rule/let can only
    // ever see lets already closed earlier in the same pass ("prior",
    // ADR-0012 §3.1).
    let all_let_names: BTreeSet<String> = module
        .items
        .iter()
        .filter_map(|it| match it {
            ast::Item::Let(l) => Some(l.name.clone()),
            _ => None,
        })
        .collect();
    let mut closed_lets: BTreeMap<String, L3ValueV1> = BTreeMap::new();
    let mut budget = ValueBudget {
        limits: *limits,
        total_nodes: 0,
        total_bytes: 0,
    };
    let mut items: Vec<L3PlanItem> = Vec::with_capacity(module.items.len());
    let mut rule_ordinal: u64 = 0;

    for item in &module.items {
        match item {
            ast::Item::Config(c) => {
                items.push(L3PlanItem::Config(configs[&c.name].clone()));
            }
            ast::Item::Let(l) => {
                let env = NormEnv {
                    configs: &configs,
                    ctors: &ctors,
                    ambiguous_ctors: &ambiguous_ctors,
                    closed_lets: &closed_lets,
                    all_let_names: &all_let_names,
                    self_name: Some(l.name.as_str()),
                };
                let value = normalize_static_value(&l.value, &env)?;
                check_declared_type(&l.ty, &value, &l.name, &config_names)?;
                let metrics = value_metrics(&value);
                budget.charge(metrics)?;
                closed_lets.insert(l.name.clone(), value.clone());
                items.push(L3PlanItem::Let {
                    name: l.name.clone(),
                    value,
                });
            }
            ast::Item::Rule(r) => {
                let env = NormEnv {
                    configs: &configs,
                    ctors: &ctors,
                    ambiguous_ctors: &ambiguous_ctors,
                    closed_lets: &closed_lets,
                    all_let_names: &all_let_names,
                    self_name: None,
                };
                let value = normalize_static_value(&r.body, &env)?;
                check_declared_type(&r.ret, &value, &r.name, &config_names)?;
                let metrics = value_metrics(&value);
                budget.charge(metrics)?;
                items.push(L3PlanItem::Rule {
                    ordinal: rule_ordinal,
                    name: r.name.clone(),
                    value,
                });
                rule_ordinal += 1;
            }
            _ => unreachable!("non-{{config,let,rule}} items rejected in pass 0"),
        }
    }

    Ok(L3PlanV1 {
        profile: profile.to_string(),
        items,
        limits: *limits,
    })
}

/// Resolve a surface type to an [`L3TypeRef`], rejecting grade annotations,
/// unsupported primitives, and unknown names.
fn resolve_ty(ty: &ast::Ty, config_names: &BTreeSet<String>) -> Result<L3TypeRef, L3LowerError> {
    match ty {
        ast::Ty::Graded(inner, _) => Err(L3LowerError::GradeAssertionUnsupported(format!(
            "{inner:?}"
        ))),
        // Type application — a parameterized config at an instantiation. The
        // v1 profile's config vocabulary is monomorphic, so it is refused by
        // name rather than approximated by its head.
        ast::Ty::App(name, _) => Err(L3LowerError::UnknownTypeName(name.clone())),
        // An anonymous record type — the desugaring of a named-field sum
        // variant. The v1 profile has no anonymous payload type, and its
        // constructors are nullary anyway (`PayloadBearingConstructor`), so it
        // is refused here by name rather than approximated by a nominal one.
        ast::Ty::Record(_) => Err(L3LowerError::AnonymousRecordTypeNotAllowed),
        ast::Ty::Named(n) => match n.as_str() {
            "Int" => Ok(L3TypeRef::Int),
            "Str" => Ok(L3TypeRef::Str),
            "Float" => Err(L3LowerError::UnsupportedPrimitiveType(n.clone())),
            other => {
                if config_names.contains(other) {
                    Ok(L3TypeRef::Config(other.to_string()))
                } else {
                    Err(L3LowerError::UnknownTypeName(other.to_string()))
                }
            }
        },
    }
}

/// Check a `let`/`rule`'s optional declared type against the inferred type
/// of its normalized value.
fn check_declared_type(
    declared: &Option<ast::Ty>,
    value: &L3ValueV1,
    name: &str,
    config_names: &BTreeSet<String>,
) -> Result<(), L3LowerError> {
    let Some(ty) = declared else {
        return Ok(());
    };
    let expected = resolve_ty(ty, config_names)?;
    let found = infer_value_type(value);
    if expected != found {
        return Err(L3LowerError::DeclaredTypeMismatch {
            name: name.to_string(),
            expected: format!("{expected:?}"),
            found: format!("{found:?}"),
        });
    }
    Ok(())
}

fn infer_value_type(v: &L3ValueV1) -> L3TypeRef {
    match v {
        L3ValueV1::Int(_) => L3TypeRef::Int,
        L3ValueV1::Str(_) => L3TypeRef::Str,
        L3ValueV1::Record { nominal_config, .. } => L3TypeRef::Config(nominal_config.clone()),
        L3ValueV1::NullaryVariant { nominal_sum, .. } => L3TypeRef::Config(nominal_sum.clone()),
    }
}

/// The read-only environment `normalize_static_value` recurses under.
/// Bundled into one struct (rather than one parameter apiece) to keep the
/// normalizer's signature small.
struct NormEnv<'a> {
    configs: &'a BTreeMap<String, L3ConfigDecl>,
    ctors: &'a BTreeMap<String, (String, usize)>,
    ambiguous_ctors: &'a BTreeSet<String>,
    closed_lets: &'a BTreeMap<String, L3ValueV1>,
    all_let_names: &'a BTreeSet<String>,
    /// `Some(name)` while normalizing `let name = ...`'s own value, so a
    /// self-reference can be distinguished from an ordinary unbound name.
    /// `None` while normalizing a rule body (rules are never a valid
    /// reference target from within the closed-static-value fragment).
    self_name: Option<&'a str>,
}

/// Normalize a surface expression to a closed [`L3ValueV1`] under
/// ADR-0012 §3.2, or reject with the specific reason it falls outside that
/// fragment.
fn normalize_static_value(expr: &ast::Expr, env: &NormEnv) -> Result<L3ValueV1, L3LowerError> {
    match expr {
        ast::Expr::Num(s) => {
            if s.contains('.') {
                return Err(L3LowerError::FloatLiteralNotAllowed(s.clone()));
            }
            s.parse::<i64>()
                .map(L3ValueV1::Int)
                .map_err(|_| L3LowerError::IntegerOverflow(s.clone()))
        }
        ast::Expr::Str(s) => Ok(L3ValueV1::Str(s.clone())),
        ast::Expr::Var(name) => {
            if env.ambiguous_ctors.contains(name) {
                return Err(L3LowerError::AmbiguousConstructorReference(name.clone()));
            }
            if let Some((sum, arity)) = env.ctors.get(name) {
                return if *arity == 0 {
                    Ok(L3ValueV1::NullaryVariant {
                        nominal_sum: sum.clone(),
                        variant: name.clone(),
                    })
                } else {
                    Err(L3LowerError::PayloadBearingConstructor(name.clone()))
                };
            }
            if let Some(v) = env.closed_lets.get(name) {
                return Ok(v.clone());
            }
            if env.self_name == Some(name.as_str()) {
                return Err(L3LowerError::RecursiveLetReference(name.clone()));
            }
            if env.all_let_names.contains(name) {
                return Err(L3LowerError::ForwardLetReference(name.clone()));
            }
            Err(L3LowerError::UnclosedReference(name.clone()))
        }
        ast::Expr::Record { config, fields } => {
            let Some(decl) = env.configs.get(config) else {
                return Err(L3LowerError::UnknownTypeName(config.clone()));
            };
            let field_tys = match &decl.body {
                L3ConfigBody::Record(fs) => fs,
                L3ConfigBody::Sum(_) => return Err(L3LowerError::NotARecordConfig(config.clone())),
            };

            let mut seen: BTreeSet<String> = BTreeSet::new();
            for (fname, _) in fields {
                if !seen.insert(fname.clone()) {
                    return Err(L3LowerError::DuplicateRecordLiteralField {
                        config: config.clone(),
                        field: fname.clone(),
                    });
                }
            }
            for (decl_name, _) in field_tys {
                if !fields.iter().any(|(n, _)| n == decl_name) {
                    return Err(L3LowerError::MissingRecordField {
                        config: config.clone(),
                        field: decl_name.clone(),
                    });
                }
            }
            for (fname, _) in fields {
                if !field_tys.iter().any(|(n, _)| n == fname) {
                    return Err(L3LowerError::UnknownRecordField {
                        config: config.clone(),
                        field: fname.clone(),
                    });
                }
            }

            let mut out_fields = Vec::with_capacity(field_tys.len());
            for (decl_name, decl_ty) in field_tys {
                let (_, fexpr) = fields
                    .iter()
                    .find(|(n, _)| n == decl_name)
                    .expect("presence already checked by the missing-field scan above");
                let fval = normalize_static_value(fexpr, env)?;
                let inferred = infer_value_type(&fval);
                if &inferred != decl_ty {
                    return Err(L3LowerError::DeclaredTypeMismatch {
                        name: format!("{config}.{decl_name}"),
                        expected: format!("{decl_ty:?}"),
                        found: format!("{inferred:?}"),
                    });
                }
                out_fields.push((decl_name.clone(), fval));
            }
            Ok(L3ValueV1::Record {
                nominal_config: config.clone(),
                fields: out_fields,
            })
        }
        ast::Expr::Field(_, name) => Err(L3LowerError::FieldAccessNotAllowed(name.clone())),
        ast::Expr::Call { func, args: _ } => {
            if env.ambiguous_ctors.contains(func) {
                return Err(L3LowerError::AmbiguousConstructorReference(func.clone()));
            }
            if let Some((_, arity)) = env.ctors.get(func) {
                if *arity > 0 {
                    return Err(L3LowerError::PayloadBearingConstructor(func.clone()));
                }
            }
            Err(L3LowerError::CallNotAllowed(func.clone()))
        }
        ast::Expr::Bin { op, .. } => match op {
            ast::BinOp::Add | ast::BinOp::Sub | ast::BinOp::Mul | ast::BinOp::Div => {
                Err(L3LowerError::ArithmeticNotAllowed)
            }
            ast::BinOp::Then | ast::BinOp::And => Err(L3LowerError::WitnessCompositionNotAllowed),
            ast::BinOp::Lt
            | ast::BinOp::Le
            | ast::BinOp::Gt
            | ast::BinOp::Ge
            | ast::BinOp::Eq
            | ast::BinOp::Ne => Err(L3LowerError::ComparisonNotAllowed),
        },
        ast::Expr::Bool(_) => Err(L3LowerError::BooleanLiteralNotAllowed),
        ast::Expr::Match { .. } => Err(L3LowerError::MatchNotAllowed),
        ast::Expr::Prove(_) => Err(L3LowerError::ProveNotAllowed),
        ast::Expr::Why(_) => Err(L3LowerError::WhyNotAllowed),
        ast::Expr::Audit(_) => Err(L3LowerError::AuditNotAllowed),
    }
}

/// Node/byte/depth footprint of one closed value (ADR-0012 §3.3: "a value
/// node is one constructor ...; root depth is one").
struct ValueMetrics {
    nodes: u64,
    bytes: u64,
    depth: u64,
}

fn value_metrics(v: &L3ValueV1) -> ValueMetrics {
    match v {
        L3ValueV1::Int(_) => ValueMetrics {
            nodes: 1,
            bytes: 0,
            depth: 1,
        },
        L3ValueV1::Str(s) => ValueMetrics {
            nodes: 1,
            bytes: s.len() as u64,
            depth: 1,
        },
        L3ValueV1::NullaryVariant { .. } => ValueMetrics {
            nodes: 1,
            bytes: 0,
            depth: 1,
        },
        L3ValueV1::Record { fields, .. } => {
            let mut nodes: u64 = 1;
            let mut bytes: u64 = 0;
            let mut max_child_depth: u64 = 0;
            for (_, fv) in fields {
                let m = value_metrics(fv);
                nodes = nodes.saturating_add(m.nodes);
                bytes = bytes.saturating_add(m.bytes);
                max_child_depth = max_child_depth.max(m.depth);
            }
            ValueMetrics {
                nodes,
                bytes,
                depth: 1 + max_child_depth,
            }
        }
    }
}

/// Running node/byte budget across the whole plan (ADR-0012 §3.3: "Total
/// nodes/decoded string bytes count every visit while normalizing each let
/// and rule").
struct ValueBudget {
    limits: PlanLimitsV1,
    total_nodes: u64,
    total_bytes: u64,
}

impl ValueBudget {
    fn charge(&mut self, m: ValueMetrics) -> Result<(), L3LowerError> {
        if m.depth > self.limits.max_value_depth {
            return Err(L3LowerError::ValueDepthExceeded {
                limit: self.limits.max_value_depth,
                actual: m.depth,
            });
        }
        self.total_nodes = self.total_nodes.saturating_add(m.nodes);
        if self.total_nodes > self.limits.max_total_value_nodes {
            return Err(L3LowerError::ValueNodeLimitExceeded {
                limit: self.limits.max_total_value_nodes,
                actual: self.total_nodes,
            });
        }
        self.total_bytes = self.total_bytes.saturating_add(m.bytes);
        if self.total_bytes > self.limits.max_total_value_bytes {
            return Err(L3LowerError::ValueByteLimitExceeded {
                limit: self.limits.max_total_value_bytes,
                actual: self.total_bytes,
            });
        }
        Ok(())
    }
}

/// Reject any direct or mutually recursive nominal config, i.e. any cycle in
/// the "references a nominal config field/variant-parameter type" relation
/// (ADR-0012 §3.1).
fn detect_recursive_configs(
    config_order: &[String],
    configs: &BTreeMap<String, L3ConfigDecl>,
) -> Result<(), L3LowerError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Unvisited,
        InProgress,
        Done,
    }

    fn deps(body: &L3ConfigBody) -> Vec<String> {
        let mut out = Vec::new();
        match body {
            L3ConfigBody::Record(fields) => {
                for (_, ty) in fields {
                    if let L3TypeRef::Config(n) = ty {
                        out.push(n.clone());
                    }
                }
            }
            L3ConfigBody::Sum(variants) => {
                for (_, params) in variants {
                    for ty in params {
                        if let L3TypeRef::Config(n) = ty {
                            out.push(n.clone());
                        }
                    }
                }
            }
        }
        out
    }

    fn visit(
        name: &str,
        configs: &BTreeMap<String, L3ConfigDecl>,
        state: &mut BTreeMap<String, State>,
        stack: &mut Vec<String>,
    ) -> Result<(), L3LowerError> {
        match state.get(name).copied().unwrap_or(State::Done) {
            State::Done => return Ok(()),
            State::InProgress => {
                let pos = stack.iter().position(|n| n == name).unwrap_or(0);
                let mut cycle: Vec<String> = stack[pos..].to_vec();
                cycle.push(name.to_string());
                return Err(L3LowerError::RecursiveConfig { cycle });
            }
            State::Unvisited => {}
        }
        state.insert(name.to_string(), State::InProgress);
        stack.push(name.to_string());
        if let Some(decl) = configs.get(name) {
            for dep in deps(&decl.body) {
                visit(&dep, configs, state, stack)?;
            }
        }
        stack.pop();
        state.insert(name.to_string(), State::Done);
        Ok(())
    }

    let mut state: BTreeMap<String, State> = config_order
        .iter()
        .map(|n| (n.clone(), State::Unvisited))
        .collect();
    for name in config_order {
        if state.get(name).copied() == Some(State::Unvisited) {
            let mut stack = Vec::new();
            visit(name, configs, &mut state, &mut stack)?;
        }
    }
    Ok(())
}
