//! Finite-decision alpha plan lowering and canonical program identity (ADR-0030).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_syntax::ast;

use crate::finite_decision::runtime::L3ValueType;
use crate::l3_v2::{
    lower_expr_v2, L3ConfigBodyV2, L3ConfigDeclV2, L3ExprV2, L3PatternV2, L3V2LowerError,
};

/// The profile marker for finite-decision alpha (ADR-0030 ⟨D-PROFILE⟩).
pub const FINITE_DECISION_PROFILE: &str = "brix.l3.finite-decision@1";

/// A declared external input in a finite-decision plan (ADR-0031).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionInput {
    pub ordinal: u64,
    pub name: String,
    pub ty: L3ValueType,
}

/// A normalized rule in a finite-decision plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionRule {
    pub ordinal: u64,
    pub name: String,
    pub body: L3ExprV2,
    pub depends_on: Vec<String>,
}

/// A candidate proposal in a finite-decision plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionProposal {
    pub ordinal: u64,
    pub name: String,
    pub deps: Vec<String>,
    pub priority: u64,
    pub guard: L3ExprV2,
    pub value: L3ExprV2,
}

/// The single commit block in a finite-decision plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionCommit {
    pub name: String,
    pub candidates: Vec<String>,
}

/// A lowered finite-decision plan (ADR-0030, ADR-0031).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionPlan {
    pub profile: String,
    pub configs: Vec<L3ConfigDeclV2>,
    pub inputs: Vec<FiniteDecisionInput>,
    pub lets: Vec<(String, L3ExprV2)>,
    pub rules: Vec<FiniteDecisionRule>,
    pub proposals: Vec<FiniteDecisionProposal>,
    pub commit: FiniteDecisionCommit,
    pub shows: Vec<L3ExprV2>,
}

impl FiniteDecisionPlan {
    /// Look up an input by declared name.
    pub fn find_input(&self, name: &str) -> Option<&FiniteDecisionInput> {
        self.inputs.iter().find(|i| i.name == name)
    }

    /// Look up a proposal by candidate name.
    pub fn find_proposal(&self, name: &str) -> Option<&FiniteDecisionProposal> {
        self.proposals.iter().find(|p| p.name == name)
    }
}

/// Errors occurring during finite-decision lowering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FiniteDecisionLowerError {
    ProfileMismatch { expected: String, found: String },
    ItemNotAllowed(String),
    NoCommit,
    MultipleCommits(usize),
    EmptyCommit(String),
    DuplicateProposalName(String),
    DuplicateInputName(String),
    DuplicateItemName(String),
    InputNameTooLong { limit: usize },
    UnsupportedInputType { name: String, ty: String },
    UnknownCandidateInCommit { commit: String, candidate: String },
    DuplicateCandidateInCommit { commit: String, candidate: String },
    UndeclaredDependency { proposal: String, dep: String },
    ForwardOrSelfDependency { proposal: String, dep: String },
    UndeclaredFactRead { proposal: String, fact: String },
    RuleDependencyError(L3V2LowerError),
    ExprError(L3V2LowerError),
    UnresolvedImport(String),
}

impl fmt::Display for FiniteDecisionLowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileMismatch { expected, found } => {
                write!(
                    f,
                    "profile mismatch: expected '{expected}', found '{found}'"
                )
            }
            Self::ItemNotAllowed(item) => write!(f, "item not allowed in finite-decision: {item}"),
            Self::NoCommit => write!(
                f,
                "finite-decision module must contain exactly one commit declaration, found none"
            ),
            Self::MultipleCommits(count) => write!(
                f,
                "finite-decision module must contain exactly one commit declaration, found {count}"
            ),
            Self::EmptyCommit(name) => {
                write!(f, "commit declaration '{name}' has no candidate members")
            }
            Self::DuplicateProposalName(name) => write!(f, "duplicate proposal name: '{name}'"),
            Self::DuplicateInputName(name) => write!(f, "duplicate input name: '{name}'"),
            Self::DuplicateItemName(name) => write!(f, "duplicate top-level item name: '{name}'"),
            Self::InputNameTooLong { limit } => {
                write!(f, "input name exceeds length limit ({limit} bytes)")
            }
            Self::UnsupportedInputType { name, ty } => {
                write!(f, "unsupported input type for '{name}': '{ty}' (only Int, Bool, Str are supported)")
            }
            Self::UnknownCandidateInCommit { commit, candidate } => {
                write!(
                    f,
                    "commit '{commit}' references undeclared proposal '{candidate}'"
                )
            }
            Self::DuplicateCandidateInCommit { commit, candidate } => {
                write!(
                    f,
                    "commit '{commit}' contains duplicate candidate '{candidate}'"
                )
            }
            Self::UndeclaredDependency { proposal, dep } => {
                write!(f, "proposal '{proposal}' declares dependency '{dep}' which is not an earlier rule")
            }
            Self::ForwardOrSelfDependency { proposal, dep } => {
                write!(
                    f,
                    "proposal '{proposal}' has forward or self-dependency on '{dep}'"
                )
            }
            Self::UndeclaredFactRead { proposal, fact } => {
                write!(f, "proposal '{proposal}' reads fact '{fact}' without declaring it as a dependency")
            }
            Self::RuleDependencyError(err) => write!(f, "rule dependency error: {err:?}"),
            Self::ExprError(err) => write!(f, "expression lowering error: {err:?}"),
            Self::UnresolvedImport(path) => write!(f, "unresolved import: {path}"),
        }
    }
}

impl std::error::Error for FiniteDecisionLowerError {}

/// Lower a parsed syntax module into a validated [`FiniteDecisionPlan`].
pub fn lower_finite_decision_plan(
    module: &ast::Module,
    profile: &str,
) -> Result<FiniteDecisionPlan, FiniteDecisionLowerError> {
    if profile != FINITE_DECISION_PROFILE {
        return Err(FiniteDecisionLowerError::ProfileMismatch {
            expected: FINITE_DECISION_PROFILE.to_string(),
            found: profile.to_string(),
        });
    }

    // Pass 0: check item admissibility.
    let mut commit_items: Vec<&ast::CommitDecl> = Vec::new();
    for item in &module.items {
        match item {
            ast::Item::Config(_)
            | ast::Item::Let(_)
            | ast::Item::Rule(_)
            | ast::Item::Propose(_)
            | ast::Item::Show(_)
            | ast::Item::Input(_) => {}
            ast::Item::Commit(c) => {
                commit_items.push(c);
            }
            ast::Item::Fn(c) => {
                return Err(FiniteDecisionLowerError::ItemNotAllowed(format!(
                    "fn {}",
                    c.name
                )));
            }
            ast::Item::Regime(r) => {
                return Err(FiniteDecisionLowerError::ItemNotAllowed(format!(
                    "regime {}",
                    r.name
                )));
            }
            ast::Item::Witness { name, .. } => {
                return Err(FiniteDecisionLowerError::ItemNotAllowed(format!(
                    "witness {name}"
                )));
            }
            ast::Item::Use(path) => {
                return Err(FiniteDecisionLowerError::UnresolvedImport(path.clone()));
            }
        }
    }

    // Exactly one nonempty commit.
    if commit_items.is_empty() {
        return Err(FiniteDecisionLowerError::NoCommit);
    }
    if commit_items.len() > 1 {
        return Err(FiniteDecisionLowerError::MultipleCommits(
            commit_items.len(),
        ));
    }
    let commit_decl = commit_items[0];
    if commit_decl.candidates.is_empty() {
        return Err(FiniteDecisionLowerError::EmptyCommit(
            commit_decl.name.clone(),
        ));
    }
    let mut commit_candidates_seen = BTreeSet::new();
    for cand in &commit_decl.candidates {
        if !commit_candidates_seen.insert(cand.clone()) {
            return Err(FiniteDecisionLowerError::DuplicateCandidateInCommit {
                commit: commit_decl.name.clone(),
                candidate: cand.clone(),
            });
        }
    }

    // Scopes for lowering.
    let mut input_names: BTreeSet<String> = BTreeSet::new();
    let mut let_names: BTreeSet<String> = BTreeSet::new();
    let mut rule_names: BTreeSet<String> = BTreeSet::new();
    let mut proposal_names: BTreeSet<String> = BTreeSet::new();
    let mut all_top_level_names: BTreeSet<String> = BTreeSet::new();
    let mut variants_of: BTreeMap<String, String> = BTreeMap::new();
    let mut nullary: BTreeMap<String, String> = BTreeMap::new();

    let mut configs = Vec::new();
    let mut inputs = Vec::new();
    let mut lets = Vec::new();
    let mut rules = Vec::new();
    let mut proposals = Vec::new();
    let mut shows = Vec::new();

    for item in &module.items {
        match item {
            ast::Item::Input(inp) => {
                if inp.name.len() > crate::input::MAX_INPUT_NAME_BYTES {
                    return Err(FiniteDecisionLowerError::InputNameTooLong {
                        limit: crate::input::MAX_INPUT_NAME_BYTES,
                    });
                }
                if !input_names.insert(inp.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateInputName(
                        inp.name.clone(),
                    ));
                }
                if !all_top_level_names.insert(inp.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(
                        inp.name.clone(),
                    ));
                }
                let ty = match &inp.ty {
                    ast::Ty::Named(n) => match n.as_str() {
                        "Int" => L3ValueType::Int,
                        "Bool" => L3ValueType::Bool,
                        "Str" => L3ValueType::Str,
                        other => {
                            return Err(FiniteDecisionLowerError::UnsupportedInputType {
                                name: inp.name.clone(),
                                ty: other.to_string(),
                            });
                        }
                    },
                    other => {
                        return Err(FiniteDecisionLowerError::UnsupportedInputType {
                            name: inp.name.clone(),
                            ty: format!("{other:?}"),
                        });
                    }
                };
                inputs.push(FiniteDecisionInput {
                    ordinal: inputs.len() as u64,
                    name: inp.name.clone(),
                    ty,
                });
            }
            ast::Item::Config(c) => {
                if !all_top_level_names.insert(c.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(c.name.clone()));
                }
                if let ast::ConfigBody::Sum(variants) = &c.body {
                    for v in variants {
                        variants_of.insert(v.name.clone(), c.name.clone());
                        if v.params.is_empty() {
                            nullary.insert(v.name.clone(), c.name.clone());
                        }
                    }
                }
                let body = match &c.body {
                    ast::ConfigBody::Sum(variants) => L3ConfigBodyV2::Sum(
                        variants
                            .iter()
                            .map(|v| (v.name.clone(), v.params.len()))
                            .collect(),
                    ),
                    ast::ConfigBody::Record(fields) => {
                        L3ConfigBodyV2::Record(fields.iter().map(|f| f.name.clone()).collect())
                    }
                };
                configs.push(L3ConfigDeclV2 {
                    name: c.name.clone(),
                    body,
                });
            }
            ast::Item::Let(l) => {
                if !all_top_level_names.insert(l.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(l.name.clone()));
                }
                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let value = lower_expr_v2(
                    &l.value,
                    &visible_bindings,
                    &BTreeSet::new(),
                    &nullary,
                    &variants_of,
                    false,
                )
                .map_err(FiniteDecisionLowerError::ExprError)?;
                let_names.insert(l.name.clone());
                lets.push((l.name.clone(), value));
            }
            ast::Item::Rule(r) => {
                if !all_top_level_names.insert(r.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(r.name.clone()));
                }
                let mut depends_on: Vec<String> = Vec::new();
                for param in &r.params {
                    if param.name == r.name {
                        return Err(FiniteDecisionLowerError::RuleDependencyError(
                            L3V2LowerError::ForwardOrSelfDependency {
                                rule: r.name.clone(),
                                depends_on: param.name.clone(),
                            },
                        ));
                    }
                    if !rule_names.contains(&param.name) {
                        return Err(FiniteDecisionLowerError::RuleDependencyError(
                            L3V2LowerError::UndeclaredDependency {
                                rule: r.name.clone(),
                                param: param.name.clone(),
                            },
                        ));
                    }
                    if !depends_on.contains(&param.name) {
                        depends_on.push(param.name.clone());
                    }
                }
                let readable: BTreeSet<String> = depends_on.iter().cloned().collect();
                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let body = lower_expr_v2(
                    &r.body,
                    &visible_bindings,
                    &readable,
                    &nullary,
                    &variants_of,
                    true,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if rule_names.contains(&n) => {
                        FiniteDecisionLowerError::RuleDependencyError(
                            L3V2LowerError::UndeclaredFactRead {
                                rule: r.name.clone(),
                                fact: n,
                            },
                        )
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;
                rule_names.insert(r.name.clone());
                rules.push(FiniteDecisionRule {
                    ordinal: rules.len() as u64,
                    name: r.name.clone(),
                    body,
                    depends_on,
                });
            }
            ast::Item::Propose(p) => {
                if !proposal_names.insert(p.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateProposalName(
                        p.name.clone(),
                    ));
                }
                if !all_top_level_names.insert(p.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(p.name.clone()));
                }
                let mut deps: Vec<String> = Vec::new();
                for dep in &p.deps {
                    if dep == &p.name {
                        return Err(FiniteDecisionLowerError::ForwardOrSelfDependency {
                            proposal: p.name.clone(),
                            dep: dep.clone(),
                        });
                    }
                    if !rule_names.contains(dep) {
                        return Err(FiniteDecisionLowerError::UndeclaredDependency {
                            proposal: p.name.clone(),
                            dep: dep.clone(),
                        });
                    }
                    if !deps.contains(dep) {
                        deps.push(dep.clone());
                    }
                }
                let readable: BTreeSet<String> = deps.iter().cloned().collect();
                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let guard = lower_expr_v2(
                    &p.guard,
                    &visible_bindings,
                    &readable,
                    &nullary,
                    &variants_of,
                    true,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if rule_names.contains(&n) => {
                        FiniteDecisionLowerError::UndeclaredFactRead {
                            proposal: p.name.clone(),
                            fact: n,
                        }
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;
                let value = lower_expr_v2(
                    &p.value,
                    &visible_bindings,
                    &readable,
                    &nullary,
                    &variants_of,
                    true,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if rule_names.contains(&n) => {
                        FiniteDecisionLowerError::UndeclaredFactRead {
                            proposal: p.name.clone(),
                            fact: n,
                        }
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;
                proposals.push(FiniteDecisionProposal {
                    ordinal: proposals.len() as u64,
                    name: p.name.clone(),
                    deps,
                    priority: p.priority,
                    guard,
                    value,
                });
            }
            ast::Item::Show(expr) => {
                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let show = lower_expr_v2(
                    expr,
                    &visible_bindings,
                    &rule_names,
                    &nullary,
                    &variants_of,
                    true,
                )
                .map_err(FiniteDecisionLowerError::ExprError)?;
                shows.push(show);
            }
            ast::Item::Commit(_) => {
                // Handled via commit_decl.
            }
            _ => unreachable!("pass 0 filtered unexpected items"),
        }
    }

    // Every candidate in commit must reference a declared proposal.
    for cand in &commit_decl.candidates {
        if !proposal_names.contains(cand) {
            return Err(FiniteDecisionLowerError::UnknownCandidateInCommit {
                commit: commit_decl.name.clone(),
                candidate: cand.clone(),
            });
        }
    }

    let commit = FiniteDecisionCommit {
        name: commit_decl.name.clone(),
        candidates: commit_decl.candidates.clone(),
    };

    Ok(FiniteDecisionPlan {
        profile: FINITE_DECISION_PROFILE.to_string(),
        configs,
        inputs,
        lets,
        rules,
        proposals,
        commit,
        shows,
    })
}

// ---------------------------------------------------------------------------
// Canonical Program Identity (ADR-0030 ⟨D-PROGID⟩)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FiniteDecisionProgramId(pub Digest);

impl FiniteDecisionProgramId {
    pub fn digest(&self) -> Digest {
        self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    pub fn from_canon(payload: &[u8]) -> Self {
        FiniteDecisionProgramId(Digest::of(Domain::Value, payload))
    }
}

impl Canonical for FiniteDecisionProgramId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

fn encode_config_body_v2(w: &mut CanonWriter, body: &L3ConfigBodyV2) {
    match body {
        L3ConfigBodyV2::Sum(variants) => {
            w.write_enum(0, |w| {
                w.write_uint(variants.len() as u64);
                for (name, arity) in variants {
                    w.write_ident(name);
                    w.write_uint(*arity as u64);
                }
            });
        }
        L3ConfigBodyV2::Record(fields) => {
            w.write_enum(1, |w| {
                w.write_uint(fields.len() as u64);
                for f in fields {
                    w.write_ident(f);
                }
            });
        }
    }
}

fn encode_pattern_v2(w: &mut CanonWriter, pat: &L3PatternV2) {
    match pat {
        L3PatternV2::Ctor { variant, binders } => {
            w.write_ident(variant);
            w.write_uint(binders.len() as u64);
            for b in binders {
                match b {
                    None => w.write_enum(0, |_| {}),
                    Some(name) => w.write_enum(1, |w| w.write_ident(name)),
                }
            }
        }
    }
}

fn encode_expr_v2(w: &mut CanonWriter, e: &L3ExprV2) {
    match e {
        L3ExprV2::Int(n) => w.write_enum(0, |w| w.write_int(*n)),
        L3ExprV2::Str(s) => w.write_enum(1, |w| w.write_str(s)),
        L3ExprV2::Bool(b) => w.write_enum(2, |w| w.write_bool(*b)),
        L3ExprV2::LetRef(name) => w.write_enum(3, |w| w.write_ident(name)),
        L3ExprV2::RuleFact(rule) => w.write_enum(4, |w| w.write_ident(rule)),
        L3ExprV2::NullaryVariant {
            nominal_sum,
            variant,
        } => w.write_enum(5, |w| {
            w.write_ident(nominal_sum);
            w.write_ident(variant);
        }),
        L3ExprV2::Ctor {
            nominal_sum,
            variant,
            args,
        } => w.write_enum(6, |w| {
            w.write_ident(nominal_sum);
            w.write_ident(variant);
            w.write_uint(args.len() as u64);
            for a in args {
                encode_expr_v2(w, a);
            }
        }),
        L3ExprV2::Record {
            nominal_config,
            fields,
        } => w.write_enum(7, |w| {
            w.write_ident(nominal_config);
            w.write_uint(fields.len() as u64);
            for (name, value) in fields {
                w.write_ident(name);
                encode_expr_v2(w, value);
            }
        }),
        L3ExprV2::Field(base, field) => w.write_enum(8, |w| {
            encode_expr_v2(w, base);
            w.write_ident(field);
        }),
        L3ExprV2::Arith(op, a, b) => w.write_enum(9, |w| {
            w.write_uint(op.ordinal());
            encode_expr_v2(w, a);
            encode_expr_v2(w, b);
        }),
        L3ExprV2::Cmp(op, a, b) => w.write_enum(10, |w| {
            w.write_uint(op.ordinal());
            encode_expr_v2(w, a);
            encode_expr_v2(w, b);
        }),
        L3ExprV2::Match { scrutinee, arms } => w.write_enum(11, |w| {
            encode_expr_v2(w, scrutinee);
            w.write_uint(arms.len() as u64);
            for (pat, body) in arms {
                encode_pattern_v2(w, pat);
                encode_expr_v2(w, body);
            }
        }),
    }
}

fn encode_input_type(w: &mut CanonWriter, ty: &L3ValueType) {
    match ty {
        L3ValueType::Int => w.write_enum(0, |_| {}),
        L3ValueType::Bool => w.write_enum(1, |_| {}),
        L3ValueType::Str => w.write_enum(2, |_| {}),
        other => panic!("non-scalar input type in plan: {other:?}"),
    }
}

/// The canonical program preimage uniquely binding normalized rules, proposals,
/// guards, values, priorities, commit membership and order, show directives,
/// and profile marker (ADR-0030 ⟨D-PROGID⟩, ADR-0031 ⟨D-IDENTITY⟩).
pub fn finite_decision_program_preimage(plan: &FiniteDecisionPlan) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.finite-decision.program@1");
    w.write_str(&plan.profile);

    // Configs
    w.write_uint(plan.configs.len() as u64);
    for c in &plan.configs {
        w.write_ident(&c.name);
        encode_config_body_v2(&mut w, &c.body);
    }

    // Inputs (ADR-0031):
    // Bound into program identity when present. When empty, omitted to maintain
    // byte-for-byte preimage and ProgramId compatibility with alpha.2 programs.
    if !plan.inputs.is_empty() {
        w.write_tag("brix.l3.finite-decision.inputs@1");
        w.write_uint(plan.inputs.len() as u64);
        for inp in &plan.inputs {
            w.write_uint(inp.ordinal);
            w.write_ident(&inp.name);
            encode_input_type(&mut w, &inp.ty);
        }
    }

    // Lets
    w.write_uint(plan.lets.len() as u64);
    for (name, expr) in &plan.lets {
        w.write_ident(name);
        encode_expr_v2(&mut w, expr);
    }

    // Rules
    w.write_uint(plan.rules.len() as u64);
    for r in &plan.rules {
        w.write_uint(r.ordinal);
        w.write_ident(&r.name);
        w.write_uint(r.depends_on.len() as u64);
        for d in &r.depends_on {
            w.write_ident(d);
        }
        encode_expr_v2(&mut w, &r.body);
    }

    // Proposals
    w.write_uint(plan.proposals.len() as u64);
    for p in &plan.proposals {
        w.write_uint(p.ordinal);
        w.write_ident(&p.name);
        w.write_uint(p.deps.len() as u64);
        for d in &p.deps {
            w.write_ident(d);
        }
        w.write_uint(p.priority);
        encode_expr_v2(&mut w, &p.guard);
        encode_expr_v2(&mut w, &p.value);
    }

    // Commit membership and order
    w.write_ident(&plan.commit.name);
    w.write_uint(plan.commit.candidates.len() as u64);
    for cand in &plan.commit.candidates {
        w.write_ident(cand);
    }

    // Show directives
    w.write_uint(plan.shows.len() as u64);
    for s in &plan.shows {
        encode_expr_v2(&mut w, s);
    }

    w.finish()
}

/// Compute the canonical program identity for a finite-decision plan.
pub fn finite_decision_program_id(plan: &FiniteDecisionPlan) -> FiniteDecisionProgramId {
    FiniteDecisionProgramId::from_canon(&finite_decision_program_preimage(plan))
}
