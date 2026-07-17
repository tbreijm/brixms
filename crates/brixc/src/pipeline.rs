//! The compiler pipeline as trait seams: `ast → ir → phase → plan → emit`.
//!
//! Each arrow is a trait so the stages can be developed and tested in isolation,
//! and so this crate compiles today against sibling lanes (`brix-ast`, `brix-ir`,
//! `brix-phase`) that are not yet on this branch. The two "real now" stages —
//! [`crate::plan`] and [`crate::emit`] — have concrete implementations; the three
//! upstream stages are `todo` seams that fail closed with a clear
//! [`PipelineError::Unimplemented`] naming the stage and its owning crate, so a
//! premature call is a legible error, not a silent wrong answer.
//!
//! The artifact types (`Ast`, `Ir`, `Phased`) are associated types on the seams,
//! not concrete structs here — when `brix-ir` merges, its `CoreIr` becomes
//! `Lower::Ir` with no change to this file's shape. That is the point of wiring
//! the seams before the lanes land (the task brief: "design the compiler pipeline
//! against documented interfaces so wiring is thin later").

use std::fmt;

use brix_diag::Diagnostic;

/// Which pipeline stage a not-yet-wired seam belongs to, and which sibling crate
/// owns it. Used to make `todo` seams self-describing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Parse: source → AST/CST. Owned by `brix-ast`.
    Parse,
    /// Lower: AST → Core IR (name res, type/effect inference, checks). `brix-ir`.
    Lower,
    /// Phase: IR → phase-assigned IR (App. F, SCC condensation). `brix-phase`.
    Phase,
    /// Plan: phased IR → per-rule join plans. `brixc` (real, `crate::plan`).
    Plan,
    /// Emit: plan → generated Rust workspace. `brixc` (real, `crate::emit`).
    Emit,
}

impl Stage {
    /// The sibling crate that owns this stage.
    pub fn owner(self) -> &'static str {
        match self {
            Stage::Parse => "brix-ast",
            Stage::Lower => "brix-ir",
            Stage::Phase => "brix-phase",
            Stage::Plan | Stage::Emit => "brixc",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Stage::Parse => "parse",
            Stage::Lower => "lower",
            Stage::Phase => "phase",
            Stage::Plan => "plan",
            Stage::Emit => "emit",
        };
        write!(f, "{name}")
    }
}

/// A pipeline error. Real diagnostics flow through `brix-diag` once that crate's
/// API is on this branch; until then a stage reports either a not-yet-wired seam
/// or a stage-local failure message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineError {
    /// A `todo` seam that depends on a sibling lane not yet merged.
    Unimplemented { stage: Stage, owner: &'static str },
    /// A stage ran but failed (placeholder for a `brix-diag` Diagnostic).
    Stage { stage: Stage, message: String },
    /// A stage failed with a structured, source-map-ready diagnostic.
    Diagnostic {
        stage: Stage,
        diagnostic: Diagnostic,
    },
}

impl PipelineError {
    /// Construct the standard "this seam waits on lane X" error.
    pub fn unimplemented(stage: Stage) -> Self {
        PipelineError::Unimplemented {
            stage,
            owner: stage.owner(),
        }
    }
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::Unimplemented { stage, owner } => write!(
                f,
                "brixc pipeline stage `{stage}` is not yet wired \
                 (waiting on lane `{owner}`)"
            ),
            PipelineError::Stage { stage, message } => {
                write!(f, "brixc `{stage}` stage failed: {message}")
            }
            PipelineError::Diagnostic { stage, diagnostic } => {
                write!(f, "brixc `{stage}` stage failed: {diagnostic:?}")
            }
        }
    }
}

impl std::error::Error for PipelineError {}

/// Stage 1 — parse source into this frontend's AST. Owned by `brix-ast`.
pub trait Frontend {
    type Ast;
    fn parse(&self, source: &str) -> Result<Self::Ast, PipelineError>;
}

/// Stage 2 — lower an AST into Core IR. Owned by `brix-ir`. Codegen must be a
/// semantics-free translation of this IR (Ring0_Build_Plan §1.4).
pub trait Lower {
    type Ast;
    type Ir;
    fn lower(&self, ast: Self::Ast) -> Result<Self::Ir, PipelineError>;
}

/// Stage 3 — assign phases (App. F). Owned by `brix-phase`.
pub trait PhaseAssign {
    type Ir;
    type Phased;
    fn assign_phases(&self, ir: Self::Ir) -> Result<Self::Phased, PipelineError>;
}

/// Stage 4 — choose join plans per rule. Owned by `brixc` ([`crate::plan`]).
pub trait Plan {
    type Phased;
    type Planned;
    fn plan(&self, phased: Self::Phased) -> Result<Self::Planned, PipelineError>;
}

/// Stage 5 — emit the generated Rust workspace. Owned by `brixc`
/// ([`crate::emit`]). The output is the on-disk generated cargo workspace's
/// source (here modeled as the crate-root string; the real writer fans it out to
/// files).
pub trait Emit {
    type Planned;
    fn emit(&self, planned: Self::Planned) -> Result<String, PipelineError>;
}

/// A `todo` frontend seam: every method fails closed until `brix-ast` merges.
/// Kept as a real type (not a bare `todo!()`) so callers get a legible error and
/// the CLI can dispatch to it today.
pub struct UnwiredFrontend;

impl Frontend for UnwiredFrontend {
    type Ast = ();
    fn parse(&self, _source: &str) -> Result<Self::Ast, PipelineError> {
        Err(PipelineError::unimplemented(Stage::Parse))
    }
}

/// A `todo` lowering seam until `brix-ir` merges.
pub struct UnwiredLower;

impl Lower for UnwiredLower {
    type Ast = ();
    type Ir = ();
    fn lower(&self, _ast: Self::Ast) -> Result<Self::Ir, PipelineError> {
        Err(PipelineError::unimplemented(Stage::Lower))
    }
}

/// A `todo` phase-assignment seam until `brix-phase` merges.
pub struct UnwiredPhase;

impl PhaseAssign for UnwiredPhase {
    type Ir = ();
    type Phased = ();
    fn assign_phases(&self, _ir: Self::Ir) -> Result<Self::Phased, PipelineError> {
        Err(PipelineError::unimplemented(Stage::Phase))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwired_stages_name_their_owning_lane() {
        let e = UnwiredFrontend.parse("package x @ 1.0.0").unwrap_err();
        assert_eq!(
            e,
            PipelineError::Unimplemented {
                stage: Stage::Parse,
                owner: "brix-ast"
            }
        );
        assert!(e.to_string().contains("brix-ast"));

        let e = UnwiredLower.lower(()).unwrap_err();
        assert!(e.to_string().contains("brix-ir"));

        let e = UnwiredPhase.assign_phases(()).unwrap_err();
        assert!(e.to_string().contains("brix-phase"));
    }

    #[test]
    fn plan_and_emit_are_owned_by_brixc() {
        assert_eq!(Stage::Plan.owner(), "brixc");
        assert_eq!(Stage::Emit.owner(), "brixc");
    }
}
