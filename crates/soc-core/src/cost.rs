//! Per-step cost records (ADR-0001 §4 "Honest resource semantics — split",
//! stage-4a; ADR-0002 §9.1 "THE invariant").
//!
//! ADR-0001 stage-4a mandates a per-step cost record as a **parallel graded
//! row**, purely observational, with one non-negotiable discipline: an
//! unmeasured step is *never* silently defaulted to zero cost.
//!
//! > **4a (now):** cost *records* ... Purely **observational**; nothing
//! > fails closed; `UnknownCost` everywhere.
//!
//! ADR-0002 §9.1 is what actually *consumes* these records: THE invariant —
//! cost per committed step MUST be ∝ |Δ| × index fanout, and MUST NOT be ∝
//! |world| — is measured via stage-4a cost records (`Build_Plan_v3_SOC.md`
//! Step 6, the O(Δ) gate, `tests/o_delta_gate.rs`).
//!
//! This module is deliberately minimal. A full graded cost algebra (named
//! input-size vars, ℕ constants, `+`/`×`, sparse polynomials, output-size
//! substitution; `time`/`space`/`value-bits`/`output-bits`/`proof-bytes`/
//! `verifier-work` categories) is stage-4a's eventual shape once it lands in
//! `brix-semantic` proper. `soc-core` needs only enough of it right now to
//! make the naive oracle's cost *measurable* — a single deterministic
//! work-unit count per call — which is exactly the one category the O(Δ)
//! gate needs to tell "cost grew with `|world|`" apart from "cost stayed
//! flat."
//!
//! **The invariant this type itself enforces:** cost is never silently zero
//! or defaulted. `CostRecord` deliberately has no `Default` impl and no
//! variant meaning "zero, unmeasured" — an unmeasured step must say so
//! explicitly, via [`CostRecord::UnknownCost`].

/// A per-step cost record. Every instrumented step on the oracle's path
/// emits one of these — never omitted, never silently zero.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CostRecord {
    /// A measured, deterministic work-unit count for this step. Work units
    /// count discrete operations performed (e.g. "one regime was asked for
    /// its candidates", "one raw candidate was scanned for admissibility")
    /// — **never** wall-clock time, which is flaky under CI load and would
    /// make the O(Δ) gate nondeterministic (ADR-0002 §9.1 gate discipline;
    /// there is no `criterion` in the workspace Ring-0 whitelist and none
    /// is added here).
    Steps(u64),
    /// This step's cost was not measured. `reason` states why (e.g. "not
    /// yet instrumented", "delegates to an unmeasured subsystem"). Per
    /// ADR-0001 stage-4a, an unmeasured step is *never* treated as
    /// zero-cost — an absent measurement must be explicit, not implicit.
    UnknownCost(&'static str),
}

impl CostRecord {
    /// The measured work-unit count, if any. `None` for `UnknownCost` —
    /// callers MUST NOT treat `None` as zero cost; an absent measurement is
    /// not evidence of no work having been done.
    pub fn work_units(&self) -> Option<u64> {
        match self {
            CostRecord::Steps(n) => Some(*n),
            CostRecord::UnknownCost(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_reports_its_work_unit_count() {
        assert_eq!(CostRecord::Steps(42).work_units(), Some(42));
    }

    #[test]
    fn unknown_cost_is_not_zero_it_is_none() {
        let c = CostRecord::UnknownCost("not yet instrumented");
        assert_eq!(
            c.work_units(),
            None,
            "an unmeasured step must never read back as zero cost"
        );
    }

    #[test]
    fn distinct_unknown_reasons_are_distinguishable() {
        assert_ne!(
            CostRecord::UnknownCost("reason a"),
            CostRecord::UnknownCost("reason b")
        );
    }

    #[test]
    fn equal_records_compare_equal() {
        assert_eq!(CostRecord::Steps(7), CostRecord::Steps(7));
        assert_eq!(
            CostRecord::UnknownCost("same"),
            CostRecord::UnknownCost("same")
        );
    }
}
