//! A bounded witness-frontier profile over ordinary closed `rule` declarations.
//!
//! A Brix rule already means propose -> commit. This profile changes only the
//! presentation: all rule witnesses start from one configuration and appear
//! together in one keyed frontier. It adds no source `regime`, `gen`, or
//! witness-declaration syntax, and does not alter SOC core semantics.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, Decomposition, GeneratorId, GeneratorRegistry};
use soc_core::adm::AdmAll;
use soc_core::audit::{audit_journal, AuditResult, GeneratorSemanticsV1};
use soc_core::calendar::Key;
use soc_core::commit::{
    try_commit_tick, CommitError, CommitTickError, Committed, SettlementWitnessProvider,
};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::Journal;
use soc_core::witness_provider::{Candidate, WitnessProvider};

use crate::l3::{
    lower_l3_plan, L3LowerError, L3PlanItem, L3PlanV1, L3ValueV1, PlanLimitsV1,
    L3_PROFILE_MARKER_V1,
};
use crate::l3_canon::{program_id, rule_id, RuleId};

/// Versioned profile marker, distinct from v1's serial rule agenda.
pub const L3_WITNESS_FRONTIER_PROFILE: &str = "brix.l3.witness-frontier@1";
const PLAN_MARKER: &[u8] = b"brix.l3.witness-frontier.plan";
const WORLD_MARKER: &[u8] = b"brix.l3.witness-frontier.world";
const POLICY_MARKER: &[u8] = b"brix.l3.witness-frontier.adm-all";
const CONTEXT_MARKER: &[u8] = b"brix.l3.witness-frontier.context";
const GENERATOR_TAG: &str = "brix.l3.witness-frontier.generator@1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WitnessFrontierProgramId(Digest);
impl WitnessFrontierProgramId {
    pub fn digest(self) -> Digest {
        self.0
    }
}
impl Canonical for WitnessFrontierProgramId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitnessRule {
    pub ordinal: u64,
    pub name: String,
    pub value: L3ValueV1,
}

/// A normalized, finite set of ordinary source rules under this profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitnessFrontierPlan {
    pub profile: String,
    pub rules: Vec<WitnessRule>,
    pub limits: PlanLimitsV1,
    source: L3PlanV1,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WitnessFrontierLowerError {
    ProfileMismatch { expected: String, found: String },
    ClosedRule(L3LowerError),
}

/// Lower existing finite, zero-argument closed rules. The v1 lowerer is used
/// only to validate values; its marker stays strict and no v1 identity moves.
pub fn lower_witness_frontier_plan(
    module: &brix_syntax::ast::Module,
    profile: &str,
    limits: &PlanLimitsV1,
) -> Result<WitnessFrontierPlan, WitnessFrontierLowerError> {
    if profile != L3_WITNESS_FRONTIER_PROFILE {
        return Err(WitnessFrontierLowerError::ProfileMismatch {
            expected: L3_WITNESS_FRONTIER_PROFILE.to_string(),
            found: profile.to_string(),
        });
    }
    let mut source = lower_l3_plan(module, L3_PROFILE_MARKER_V1, limits)
        .map_err(WitnessFrontierLowerError::ClosedRule)?;
    source.profile = L3_WITNESS_FRONTIER_PROFILE.to_string();
    let rules = source
        .items
        .iter()
        .filter_map(|item| match item {
            L3PlanItem::Rule {
                ordinal,
                name,
                value,
            } => Some(WitnessRule {
                ordinal: *ordinal,
                name: name.clone(),
                value: value.clone(),
            }),
            _ => None,
        })
        .collect();
    Ok(WitnessFrontierPlan {
        profile: L3_WITNESS_FRONTIER_PROFILE.to_string(),
        rules,
        limits: *limits,
        source,
    })
}

pub fn witness_frontier_program_id(plan: &WitnessFrontierPlan) -> WitnessFrontierProgramId {
    let source = program_id(&plan.source);
    let mut w = CanonWriter::new();
    w.write_bytes(PLAN_MARKER);
    w.write_uint(1);
    w.write_str(&plan.profile);
    w.write_bytes(source.digest().as_bytes());
    WitnessFrontierProgramId(Digest::of(Domain::Value, &w.finish()))
}

/// A successor stores a stable rule occurrence, not endpoint-bound generator.
/// The generator is derived after destination identity is fixed.
fn frontier_world(program: WitnessFrontierProgramId, selected: Option<RuleId>) -> ConfigId {
    let mut w = CanonWriter::new();
    w.write_bytes(WORLD_MARKER);
    w.write_uint(1);
    w.write_bytes(program.digest().as_bytes());
    match selected {
        None => w.write_enum(0, |_| {}),
        Some(rule) => w.write_enum(1, |w| w.write_bytes(rule.digest().as_bytes())),
    }
    ConfigId::from_canon(&w.finish())
}
fn policy_id(program: WitnessFrontierProgramId) -> ConfigId {
    let mut w = CanonWriter::new();
    w.write_bytes(POLICY_MARKER);
    w.write_uint(1);
    w.write_bytes(program.digest().as_bytes());
    ConfigId::from_canon(&w.finish())
}
fn context_id(
    program: WitnessFrontierProgramId,
    initial: ConfigId,
    policy: ConfigId,
    limits: PlanLimitsV1,
) -> ContextId {
    let mut w = CanonWriter::new();
    w.write_bytes(CONTEXT_MARKER);
    w.write_uint(1);
    w.write_bytes(program.digest().as_bytes());
    w.write_bytes(initial.digest().as_bytes());
    w.write_bytes(policy.digest().as_bytes());
    limits.canon_write(&mut w);
    ContextId::from_canon(&w.finish())
}
fn generator_id(
    program: WitnessFrontierProgramId,
    rule: RuleId,
    src: ConfigId,
    dst: ConfigId,
) -> GeneratorId {
    let mut w = CanonWriter::new();
    w.write_tag(GENERATOR_TAG);
    w.write_bytes(program.digest().as_bytes());
    w.write_bytes(rule.digest().as_bytes());
    w.write_bytes(src.digest().as_bytes());
    w.write_bytes(dst.digest().as_bytes());
    GeneratorId::from_canon(&w.finish())
}

/// Public description of a concurrent rule-witness proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WitnessCandidate {
    pub ordinal: u64,
    pub rule: RuleId,
}
#[derive(Clone, Copy, Debug)]
struct PresenterEntry {
    candidate: Candidate,
    proposal: WitnessCandidate,
    generator: GeneratorId,
    src: ConfigId,
    dst: ConfigId,
}

/// Runtime boundary for a simultaneous witness frontier. `WitnessPresenter`
/// is private: it only adapts witness discovery to the engine's provider
/// trait and neither creates nor owns semantic possibilities.
pub struct WitnessFrontierRuntime {
    pub program: WitnessFrontierProgramId,
    pub context: ContextId,
    pub initial_world: ConfigId,
    pub policy: ConfigId,
    interner: Interner,
    initial: Handle,
    policy_handle: Handle,
    entries: Vec<PresenterEntry>,
}
impl WitnessFrontierRuntime {
    pub fn build(plan: &WitnessFrontierPlan) -> Self {
        let program = witness_frontier_program_id(plan);
        let initial_world = frontier_world(program, None);
        let policy = policy_id(program);
        let context = context_id(program, initial_world, policy, plan.limits);
        let mut interner = Interner::new();
        let initial = interner.intern(initial_world.digest());
        let policy_handle = interner.intern(policy.digest());
        let source_program = program_id(&plan.source);
        let entries = plan
            .rules
            .iter()
            .map(|rule| {
                let stable_rule = rule_id(source_program, rule.ordinal, &rule.name);
                let dst = frontier_world(program, Some(stable_rule));
                let generator = generator_id(program, stable_rule, initial_world, dst);
                let witness = interner.intern(generator.witness_id().digest());
                let successor = interner.intern(dst.digest());
                PresenterEntry {
                    candidate: Candidate { witness, successor },
                    proposal: WitnessCandidate {
                        ordinal: rule.ordinal,
                        rule: stable_rule,
                    },
                    generator,
                    src: initial_world,
                    dst,
                }
            })
            .collect();
        Self {
            program,
            context,
            initial_world,
            policy,
            interner,
            initial,
            policy_handle,
            entries,
        }
    }
    pub fn initial_exec(&self) -> ExecConfig {
        ExecConfig::new(self.initial, self.policy_handle, History::empty().digest())
    }
    pub fn candidates_at_initial(&self) -> Vec<WitnessCandidate> {
        self.entries.iter().map(|entry| entry.proposal).collect()
    }
    fn presenter(&self) -> WitnessPresenter {
        WitnessPresenter {
            initial: self.initial,
            entries: self.entries.clone(),
        }
    }
    pub fn audit(&self, journal: &Journal) -> Vec<AuditResult> {
        let mut registry = GeneratorRegistry::new();
        let mut semantics = GeneratorSemanticsV1::new();
        for entry in &self.entries {
            registry.insert(entry.generator);
            semantics.declare_rows(entry.generator, [(entry.src, entry.dst)]);
        }
        audit_journal(journal, self.context, &registry, &semantics)
    }
}
#[derive(Clone)]
struct WitnessPresenter {
    initial: Handle,
    entries: Vec<PresenterEntry>,
}
impl WitnessProvider for WitnessPresenter {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if e.world == self.initial {
            self.entries.iter().map(|entry| entry.candidate).collect()
        } else {
            Vec::new()
        }
    }
}
impl SettlementWitnessProvider for WitnessPresenter {
    fn try_decompose(
        &self,
        e: &ExecConfig,
        candidate: &Candidate,
    ) -> Result<Decomposition, CommitError> {
        if e.world != self.initial {
            return Err(CommitError::UnresolvedHandle);
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.candidate == *candidate)
            .ok_or(CommitError::UnresolvedHandle)?;
        Decomposition::recorded(vec![entry.generator], vec![entry.src, entry.dst])
            .map_err(CommitError::from)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WitnessFrontierRunError {
    Commit(CommitTickError),
    UnexpectedEmptyFrontier,
}
#[derive(Clone, Debug)]
pub struct WitnessFrontierRun {
    pub journal: Journal,
    pub selected: WitnessCandidate,
    pub final_world: ConfigId,
}
/// Build one initial `B^uk` frontier and commit exactly its least witness.
pub fn run_witness_frontier_once(
    runtime: &WitnessFrontierRuntime,
) -> Result<WitnessFrontierRun, WitnessFrontierRunError> {
    let presenter = runtime.presenter();
    let proposal_by_witness: BTreeMap<Handle, WitnessCandidate> = runtime
        .entries
        .iter()
        .map(|entry| (entry.candidate.witness, entry.proposal))
        .collect();
    let mut keyer = |candidate: &Candidate, phase: u64| {
        let proposal = proposal_by_witness[&candidate.witness];
        Key::new(
            phase,
            proposal.ordinal,
            runtime
                .interner
                .try_resolve(candidate.witness)
                .expect("runtime witness interned"),
        )
    };
    let (committed, step, _) = try_commit_tick(
        &[&presenter],
        &AdmAll,
        &runtime.interner,
        &runtime.initial_exec(),
        runtime.context,
        0,
        &mut keyer,
    )
    .map_err(WitnessFrontierRunError::Commit)?;
    let Committed::Step { successor, .. } = committed else {
        return Err(WitnessFrontierRunError::UnexpectedEmptyFrontier);
    };
    let step = step.expect("Committed::Step carries its journal record");
    let selected = proposal_by_witness[&runtime
        .entries
        .iter()
        .find(|entry| {
            entry.dst
                == ConfigId(
                    runtime
                        .interner
                        .try_resolve(successor.world)
                        .expect("successor interned"),
                )
        })
        .expect("selected witness")
        .candidate
        .witness];
    let mut journal = Journal::new();
    journal.append(step);
    Ok(WitnessFrontierRun {
        journal,
        selected,
        final_world: ConfigId(
            runtime
                .interner
                .try_resolve(successor.world)
                .expect("successor interned"),
        ),
    })
}
