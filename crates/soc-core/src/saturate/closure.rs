//! One-step closure for a safety predicate (ADR-0014 §8; Stage D).
//!
//! ADR-0002 §8.2 Candidate A pins *"safety = one-step closure"*. The Build
//! Plan's third Step-8 clause requires it; #61's issue text omits it, which is
//! why it is carried by the ADR rather than by the issue.
//!
//! # The rule
//!
//! `Φ` is an invariant from `e₀` iff `Φ` holds at `e₀`'s projection and, for
//! every saturated-reachable `e` satisfying `Φ`, a realizing step `e ⟹o⟹ e'`
//! implies `Φ(e')`. Then `Φ` holds at every saturated-reachable state.
//!
//! The induction is sound for two specific reasons, both worth stating because
//! both are properties of *this* engine rather than of safety checking in
//! general. `D_O` is deterministic, so reachability is a **path, not a tree** —
//! there is no branching to quantify over and no fixpoint to iterate. And the
//! `1` summand discharges no successor obligation, so quiescence ends the
//! induction rather than leaving it open.
//!
//! # Why the mode is a real choice
//!
//! Saturation makes the *scope* of the obligation a decision, not a detail.
//! [`ClosureMode::Visible`] constrains only the states an observer at the
//! declared boundary can see; [`ClosureMode::Raw`] constrains every committed
//! γ-state including the administrative intermediates the profile declared
//! unobservable.
//!
//! These genuinely differ, and the acceptance fixture shows it in one graph:
//! `w0 -τ→ w_bad -τ→ w1 -o→ w2` with `Φ = (world ≠ w_bad)` is **closed** under
//! `Visible` and **violated** under `Raw`. A system may pass through a state
//! that would be unacceptable to expose, and whether that matters is a policy
//! question the caller must answer — so there is no default here.
//!
//! # What a pass means
//!
//! `Closed` is a claim about the states this walk actually reached, under the
//! presentation, profile, and budget it was given. A run that stopped at an
//! `Unknown` yields [`ClosureResult::Unknown`], never `Closed`: an unexplored
//! reachable set cannot support an invariant claim.
//!
//! Certified divergence, by contrast, **can** support one. A closed
//! administrative lasso repeats forever, so the states already seen are all the
//! states there will ever be — under either mode. That is a small but real
//! payoff from Stage B certifying divergence instead of merely bounding it.

use brix_semantic::ConfigId;

use crate::exec::ExecConfig;

use super::{
    project, sat_step, ObservableState, PresentationV1, SaturatedStep, SaturationBudget,
    SaturationUnknown,
};

/// The scope of a closure obligation.
///
/// No `Default`: which states a safety property must hold at is a decision the
/// caller owns, and silently picking one would be exactly the kind of implicit
/// reading SOC-LAW-10 requires to be stated.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ClosureMode {
    /// `Φ` holds at every visible (saturated) state; administrative
    /// intermediates are unconstrained, because the profile declared them
    /// unobservable. The reading ADR-0014 §8 calls default.
    Visible,
    /// `Φ` holds at every committed γ-state, administrative intermediates
    /// included.
    Raw,
}

/// The state a safety predicate is evaluated at.
///
/// The canonical rendering of an [`ObservableState`]: same information, with
/// `ConfigId` digests in place of interner-local [`crate::intern::Handle`]s.
/// `history` is excluded for the same reason [`project`] excludes it — a
/// predicate that could read history would not be a predicate on states at all.
///
/// **Digests, not handles**, so a violation is reportable in a durable artifact
/// and a predicate can be written once against canonical configuration
/// identities rather than against one run's allocation order. Same reasoning
/// that puts `ConfigId`s in the divergence certificate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SafetyState {
    /// The configuration.
    pub world: ConfigId,
    /// The policy in force.
    pub policy: ConfigId,
}

/// Where a predicate first failed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ViolationSite {
    /// At the initial state — so `Φ` was never an invariant to begin with, and
    /// the closure question does not even arise.
    Initial,
    /// At the successor of a saturated realizing step. **A genuine closure
    /// failure**: the predicate held before the step and not after it.
    VisibleSuccessor,
    /// At an administrative intermediate. Only reachable under
    /// [`ClosureMode::Raw`]; under `Visible` this state is not in scope.
    AdministrativeIntermediate,
}

/// A predicate that failed, and where.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ClosureViolation {
    /// The scope that was in force.
    pub mode: ClosureMode,
    /// Where the failure occurred.
    pub site: ViolationSite,
    /// The state the predicate rejected.
    pub state: SafetyState,
    /// How many visible steps had been taken when it failed.
    pub visible_depth: u64,
}

/// Why closure could not be decided.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ClosureUnknown {
    /// A saturated step established nothing, so the reachable set is not known
    /// and no invariant claim is available.
    Unestablished {
        /// Visible steps taken before the walk stalled.
        visible_depth: u64,
        /// Why the step established nothing.
        reason: SaturationUnknown,
    },
    /// The whole-run visible budget ran out with states still unexplored.
    VisibleBudgetExhausted {
        /// Visible steps taken.
        visible_steps: u64,
        /// The bound that was hit.
        budget: u64,
    },
    /// A configuration handle could not be resolved, so its canonical state is
    /// unavailable. An internal-consistency failure, surfaced rather than
    /// panicked on.
    UnresolvedConfiguration,
}

/// The outcome of a closure check.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ClosureResult {
    /// `Φ` held at every state in scope, over the whole walk.
    Closed {
        /// States the predicate was evaluated at, including the initial one.
        states_checked: u64,
        /// Visible steps the walk took.
        visible_steps: u64,
    },
    /// `Φ` failed. Never a claim that the negation is invariant.
    Violated(Box<ClosureViolation>),
    /// Neither established. Never a pass.
    Unknown(ClosureUnknown),
}

impl ClosureResult {
    /// Whether the predicate was established as an invariant over this walk.
    pub fn is_closed(&self) -> bool {
        matches!(self, ClosureResult::Closed { .. })
    }
}

/// Check that `predicate` is closed under saturated stepping from `e0`.
///
/// Walks the saturated trajectory, evaluating `predicate` at every state
/// `mode` puts in scope. Stops at the first violation with the state that
/// failed — determinism means there is one path, so this is the *first*
/// violation along it, not merely *a* violation.
///
/// A run that ends in certified quiescence or certified divergence yields a
/// decided answer; one that ends in any `Unknown` does not.
pub fn check_closure<F>(
    pres: &PresentationV1<'_>,
    e0: ExecConfig,
    predicate: &dyn Fn(&SafetyState) -> bool,
    mode: ClosureMode,
    keyer: &mut F,
    budget: SaturationBudget,
) -> ClosureResult
where
    F: FnMut(&crate::witness_provider::Candidate, u64) -> crate::calendar::Key,
{
    let Some(policy) = pres.interner.try_resolve(e0.policy).map(ConfigId) else {
        return ClosureResult::Unknown(ClosureUnknown::UnresolvedConfiguration);
    };
    let Some(initial) = canonical_state(pres, &project(&e0), policy) else {
        return ClosureResult::Unknown(ClosureUnknown::UnresolvedConfiguration);
    };

    let mut states_checked: u64 = 1;
    if !predicate(&initial) {
        return violated(mode, ViolationSite::Initial, initial, 0);
    }

    let mut current = e0;
    let mut visible_depth: u64 = 0;
    let mut phase: u64 = 0;

    loop {
        if visible_depth >= budget.max_visible_steps {
            return ClosureResult::Unknown(ClosureUnknown::VisibleBudgetExhausted {
                visible_steps: visible_depth,
                budget: budget.max_visible_steps,
            });
        }

        let (step, consumed, _) = sat_step(pres, &current, phase, keyer, budget);
        phase = phase.saturating_add(consumed.len() as u64);

        // Under `Raw`, every committed step's destination is in scope — the
        // administrative intermediates included. The last consumed step of a
        // realizing outcome is the visible successor, so it is attributed to
        // `VisibleSuccessor` rather than to the intermediates it followed.
        if mode == ClosureMode::Raw {
            let realizing = matches!(step, SaturatedStep::Realizing { .. });
            let last = consumed.len().saturating_sub(1);
            for (index, committed) in consumed.iter().enumerate() {
                let state = SafetyState {
                    world: committed.dst,
                    policy,
                };
                states_checked += 1;
                if !predicate(&state) {
                    let site = if realizing && index == last {
                        ViolationSite::VisibleSuccessor
                    } else {
                        ViolationSite::AdministrativeIntermediate
                    };
                    return violated(mode, site, state, visible_depth);
                }
            }
        }

        match step {
            SaturatedStep::Realizing { successor, .. } => {
                if mode == ClosureMode::Visible {
                    let Some(state) = canonical_state(pres, &project(&successor), policy) else {
                        return ClosureResult::Unknown(ClosureUnknown::UnresolvedConfiguration);
                    };
                    states_checked += 1;
                    if !predicate(&state) {
                        return violated(
                            mode,
                            ViolationSite::VisibleSuccessor,
                            state,
                            visible_depth + 1,
                        );
                    }
                }
                current = successor;
                visible_depth += 1;
            }
            // Both certified endings decide the question. Quiescence has no
            // successor to constrain; a closed lasso repeats states already
            // checked, so no unchecked state remains under either mode.
            SaturatedStep::Quiescent(_) | SaturatedStep::Divergent(_) => {
                return ClosureResult::Closed {
                    states_checked,
                    visible_steps: visible_depth,
                }
            }
            SaturatedStep::Unknown(reason) => {
                return ClosureResult::Unknown(ClosureUnknown::Unestablished {
                    visible_depth,
                    reason,
                })
            }
        }
    }
}

fn canonical_state(
    pres: &PresentationV1<'_>,
    observable: &ObservableState,
    policy: ConfigId,
) -> Option<SafetyState> {
    Some(SafetyState {
        world: ConfigId(pres.interner.try_resolve(observable.world)?),
        policy,
    })
}

fn violated(
    mode: ClosureMode,
    site: ViolationSite,
    state: SafetyState,
    visible_depth: u64,
) -> ClosureResult {
    ClosureResult::Violated(Box::new(ClosureViolation {
        mode,
        site,
        state,
        visible_depth,
    }))
}
