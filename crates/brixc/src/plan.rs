//! The `plan` stage: choose a logical evaluation plan per rule.
//!
//! v0 policy (Ring0_Build_Plan §1.8): **heuristic join order —
//! bound-variables-first, prefer key/index access, and a cross-product requires
//! the explicit `cross` clause** (never inferred). Plans are compile-time and
//! recorded in `meta.Plan` (Part XXVIII §28.1: "Plans are compile-time …
//! canonical-result-preserving"), so a plan choice may change *cost* but never an
//! observable value — the oracle, which ignores plans entirely, is the check.
//!
//! Plan adaptivity (stats-driven recompile) is post-G3 polish (§2). This stage is
//! deliberately a small, deterministic heuristic whose output is a `meta.Plan`
//! record. It is real and testable now because it needs only the pattern's
//! variable-binding structure, not the full IR.

/// A single relation access inside a rule body, reduced to what the join-order
/// heuristic needs: the relation name and which of its columns are already bound
/// by earlier clauses vs. introduced fresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub relation: String,
    /// Variables this access *reads* (must be bound before it, or it binds them).
    pub vars: Vec<String>,
    /// Whether this access can use a key/primary index (its bound vars cover the
    /// relation's key). Cheaper than a scan; the heuristic prefers it.
    pub key_accessible: bool,
}

/// The chosen order of accesses for one rule, plus a flag for whether an explicit
/// `cross` was required. Recorded into `meta.Plan`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinPlan {
    pub ordered: Vec<Access>,
}

/// Order accesses bound-variables-first, preferring key-accessible ones.
///
/// The rule: repeatedly pick the access with the most already-bound variables;
/// break ties toward key-accessible accesses, then by relation name (canonical,
/// so the plan is deterministic). An access that shares *no* variable with the
/// bound set and cannot key in is a cross product — the caller must have supplied
/// it via an explicit `cross` clause; `plan_join` flags when that assumption is
/// violated so the emit stage can raise a diagnostic instead of silently
/// generating a cartesian blowup.
pub fn plan_join(accesses: &[Access]) -> (JoinPlan, bool) {
    let mut remaining: Vec<Access> = accesses.to_vec();
    let mut bound: Vec<String> = Vec::new();
    let mut ordered: Vec<Access> = Vec::with_capacity(remaining.len());
    let mut implicit_cross = false;

    while !remaining.is_empty() {
        // Score each remaining access by (#bound vars it shares, key-accessible).
        let mut best_idx = 0usize;
        let mut best_score = (usize::MIN, false, std::cmp::Reverse(String::new()));
        for (i, a) in remaining.iter().enumerate() {
            let shared = a.vars.iter().filter(|v| bound.contains(v)).count();
            let score = (
                shared,
                a.key_accessible,
                std::cmp::Reverse(a.relation.clone()),
            );
            if i == 0 || score > best_score {
                best_score = score;
                best_idx = i;
            }
        }
        let chosen = remaining.remove(best_idx);
        // If nothing was bound yet, the first access legitimately seeds the join.
        // Otherwise, a chosen access that shares no bound variable and cannot key
        // in is an implicit cross product.
        if !bound.is_empty() {
            let shares = chosen.vars.iter().any(|v| bound.contains(v));
            if !shares && !chosen.key_accessible {
                implicit_cross = true;
            }
        }
        for v in &chosen.vars {
            if !bound.contains(v) {
                bound.push(v.clone());
            }
        }
        ordered.push(chosen);
    }

    (JoinPlan { ordered }, implicit_cross)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_variables_first() {
        // A(x) then B(x, y) then C(y): a linear chain should stay in a
        // connected order, not scan C before it is reachable.
        let accesses = vec![
            Access {
                relation: "C".into(),
                vars: vec!["y".into()],
                key_accessible: false,
            },
            Access {
                relation: "A".into(),
                vars: vec!["x".into()],
                key_accessible: true,
            },
            Access {
                relation: "B".into(),
                vars: vec!["x".into(), "y".into()],
                key_accessible: true,
            },
        ];
        let (plan, cross) = plan_join(&accesses);
        assert!(!cross, "a connected chain is not a cross product");
        let order: Vec<&str> = plan.ordered.iter().map(|a| a.relation.as_str()).collect();
        // A seeds (key-accessible), B joins on x, C joins on y.
        assert_eq!(order, vec!["A", "B", "C"]);
    }

    #[test]
    fn disconnected_access_flags_implicit_cross() {
        let accesses = vec![
            Access {
                relation: "A".into(),
                vars: vec!["x".into()],
                key_accessible: false,
            },
            Access {
                relation: "B".into(),
                vars: vec!["y".into()],
                key_accessible: false,
            },
        ];
        let (_plan, cross) = plan_join(&accesses);
        assert!(cross, "two variable-disjoint scans are a cross product");
    }

    #[test]
    fn plan_is_deterministic() {
        let accesses = vec![
            Access {
                relation: "A".into(),
                vars: vec!["x".into()],
                key_accessible: true,
            },
            Access {
                relation: "B".into(),
                vars: vec!["x".into()],
                key_accessible: true,
            },
        ];
        assert_eq!(plan_join(&accesses), plan_join(&accesses));
    }
}
