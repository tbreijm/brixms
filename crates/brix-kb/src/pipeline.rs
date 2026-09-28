//! The parse → resolve-imports → strip-`show` → lower → build → run pipeline,
//! independent of the `brix-cli` copy in `crates/brix-cli/src/commands/*.rs`
//! (ADR-0041 §6: `brix-kb` must not depend on `brix-cli`, and existing CLI
//! commands are out of scope for this change).
//!
//! This is exactly the pipeline `crates/brix-cli/src/commands/run.rs` and
//! `audit.rs` run today; `brix-kb` replays it on every revision.

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionPlan, FiniteDecisionRun,
    FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};
use brix_lower::input::InputSnapshot;
use brix_syntax::ast;

use crate::error::KbError;
use crate::packages::make_package_loader;

/// Remove surface `show` items before lowering — program identity and the
/// finite-decision plan are computed from the show-free module (matches
/// `crate::commands::prepare_finite_decision_module` in `brix-cli`).
pub fn strip_show_items(module: &mut ast::Module) {
    module.items.retain(|i| !matches!(i, ast::Item::Show(_)));
}

/// Parse, resolve imports, and strip `show` from `source` — the module shape
/// program identity and the finite-decision plan are both computed from.
/// Exposed separately from [`load_plan`] because verifying an audit bundle
/// (`brix_lower::audit_bundle::check_finite_decision_audit_input_bundle_from_module_with_inputs_v1`,
/// exactly what `brix verify` calls) takes this resolved module, not the
/// lowered plan.
pub fn load_resolved_module(
    source: &str,
    package_paths: &[PathBuf],
) -> Result<ast::Module, KbError> {
    let module = brix_syntax::parse_bounded(source, brix_syntax::ParseLimits::strict())
        .map_err(|e| KbError::rejected("kb-parse-error", format!("parse error: {e}")))?;
    let loader = make_package_loader(package_paths);
    let mut resolved = brix_lower::imports::resolve_imports(&module, &loader)?;
    strip_show_items(&mut resolved);
    Ok(resolved)
}

/// Parse, resolve imports, strip `show`, and lower `source` into a
/// [`FiniteDecisionPlan`].
pub fn load_plan(source: &str, package_paths: &[PathBuf]) -> Result<FiniteDecisionPlan, KbError> {
    let resolved = load_resolved_module(source, package_paths)?;
    let plan = lower_finite_decision_plan(&resolved, FINITE_DECISION_PROFILE)?;
    Ok(plan)
}

/// Read a `.brix` source file with the same strict 1 MiB bound the rest of
/// the toolchain applies.
pub fn read_program_source(path: &Path) -> Result<String, KbError> {
    crate::packages::read_source_bounded(path).map_err(|msg| KbError::io("kb-source-io-error", msg))
}

/// The result of replaying one revision's program against its input snapshot.
pub enum ReplayResult {
    /// The runtime built successfully and ran to completion (which may still
    /// be `Selected`, `Quiescent`, or `Unknown` inside `run.stop` — a faulted
    /// deliberation is not the same thing as an incomplete input contract).
    Ran {
        runtime: Box<FiniteDecisionRuntime>,
        run: Box<FiniteDecisionRun>,
    },
    /// The declared input contract was not fully satisfied by the snapshot.
    /// This is the honest outcome for a knowledge base that starts (or is
    /// retracted down to) an incomplete input set — not a fault.
    MissingInputs { missing: Vec<String> },
}

/// Build a runtime and run the plan against `snapshot`, distinguishing an
/// honest missing-input outcome from every other build failure.
pub fn replay(
    plan: &FiniteDecisionPlan,
    snapshot: &InputSnapshot,
) -> Result<ReplayResult, KbError> {
    match FiniteDecisionRuntime::build_with_inputs(plan, snapshot) {
        Ok(runtime) => {
            let run = runtime.run();
            Ok(ReplayResult::Ran {
                runtime: Box::new(runtime),
                run: Box::new(run),
            })
        }
        Err(brix_lower::FiniteDecisionBuildError::InputValidation(
            brix_lower::input::InputValidationError::MissingInput { .. },
        )) => {
            let missing: Vec<String> = plan
                .inputs
                .iter()
                .filter(|decl| snapshot.get(&decl.name).is_none())
                .map(|decl| decl.name.clone())
                .collect();
            Ok(ReplayResult::MissingInputs { missing })
        }
        Err(other) => Err(KbError::from(other)),
    }
}

/// The stable content-addressed identity of a plan — a thin, discoverable
/// re-export so callers need not reach into `brix_lower::finite_decision`
/// directly for this one function.
pub fn program_id(
    plan: &FiniteDecisionPlan,
) -> brix_lower::finite_decision::FiniteDecisionProgramId {
    finite_decision_program_id(plan)
}
