//! Executable scenario subset for the public `brix test` gate.
//!
//! Contract: [`TEST_SCENARIOS.md`](../TEST_SCENARIOS.md).

use std::collections::BTreeMap;

use brix_ast::ast::{AssertMode, BinOp, Decl, Expr, ExprKind, File, ScenarioDecl, SeedDecl, UnOp};
use brix_diag::CanonValue;

/// Stable version tag for the supported executable scenario subset.
pub const SUPPORTED_SUBSET_VERSION: &str = "1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScenarioClass {
    Executable,
    Unavailable { reasons: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertionResult {
    pub mode: AssertMode,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScenarioRun {
    Executed { assertions: Vec<AssertionResult> },
    Unavailable { reasons: Vec<String> },
}

impl ScenarioRun {
    pub fn passed(&self) -> bool {
        match self {
            Self::Executed { assertions } => assertions.iter().all(|item| item.passed),
            Self::Unavailable { .. } => false,
        }
    }
}

/// Classify whether `scenario` is in the executable subset documented in
/// [`TEST_SCENARIOS.md`](../TEST_SCENARIOS.md).
pub fn classify(scenario: &ScenarioDecl) -> ScenarioClass {
    let mut reasons = Vec::new();
    match &scenario.seed {
        SeedDecl::Each(_, _) => reasons.push("seed each is not supported".into()),
        SeedDecl::Nat(_, _) => {}
    }
    if !scenario.binds.is_empty() {
        reasons.push("clock/protocol bindings are not supported".into());
    }
    if scenario.setup.is_some() {
        reasons.push("setup blocks are not supported".into());
    }
    if !scenario.steps.is_empty() {
        reasons.push("step blocks are not supported".into());
    }
    if !scenario.ats.is_empty() {
        reasons.push("at transaction blocks are not supported".into());
    }
    if !scenario
        .asserts
        .iter()
        .any(|assert| assert.mode == AssertMode::AtEnd)
    {
        reasons.push("requires at least one `assert at end`".into());
    }
    for assert in &scenario.asserts {
        if assert.mode == AssertMode::Eventually {
            reasons.push("`assert eventually` is not supported".into());
            break;
        }
        if let Err(reason) = boolean_expr_supported(&assert.cond) {
            reasons.push(format!("unsupported assertion expression: {reason}"));
        }
    }
    if reasons.is_empty() {
        ScenarioClass::Executable
    } else {
        ScenarioClass::Unavailable { reasons }
    }
}

/// Evaluate every supported assertion in source order.
pub fn execute(scenario: &ScenarioDecl) -> ScenarioRun {
    match classify(scenario) {
        ScenarioClass::Unavailable { reasons } => ScenarioRun::Unavailable { reasons },
        ScenarioClass::Executable => {
            let mut assertions = Vec::new();
            for assert in &scenario.asserts {
                let passed = eval_boolean(&assert.cond).unwrap_or(false);
                let detail = if passed {
                    format!("evaluated to {passed}")
                } else {
                    eval_boolean(&assert.cond)
                        .err()
                        .unwrap_or_else(|| "evaluated to false".into())
                };
                assertions.push(AssertionResult {
                    mode: assert.mode,
                    passed,
                    detail,
                });
            }
            ScenarioRun::Executed { assertions }
        }
    }
}

pub fn scenarios_in_source_order(file: &File) -> Vec<&ScenarioDecl> {
    file.decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Scenario(scenario) => Some(scenario),
            _ => None,
        })
        .collect()
}

pub fn duplicate_scenario_names(file: &File) -> Vec<String> {
    let mut counts = BTreeMap::<String, u32>::new();
    for scenario in scenarios_in_source_order(file) {
        *counts.entry(scenario.name.text.clone()).or_default() += 1;
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

pub fn resolve_selectors<'a>(
    scenarios: Vec<&'a ScenarioDecl>,
    selectors: &[String],
) -> Result<Vec<&'a ScenarioDecl>, SelectorError> {
    if !duplicate_scenario_names_from_slice(&scenarios).is_empty() {
        return Err(SelectorError::Ambiguous);
    }
    if selectors.is_empty() {
        return Ok(scenarios);
    }
    let mut selected = Vec::with_capacity(selectors.len());
    for selector in selectors {
        let matches: Vec<_> = scenarios
            .iter()
            .copied()
            .filter(|scenario| scenario.name.text == *selector)
            .collect();
        match matches.len() {
            0 => return Err(SelectorError::Unknown(selector.clone())),
            1 => selected.push(matches[0]),
            _ => return Err(SelectorError::Ambiguous),
        }
    }
    Ok(selected)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectorError {
    Unknown(String),
    Ambiguous,
}

fn duplicate_scenario_names_from_slice(scenarios: &[&ScenarioDecl]) -> Vec<String> {
    let mut counts = BTreeMap::<String, u32>::new();
    for scenario in scenarios {
        *counts.entry(scenario.name.text.clone()).or_default() += 1;
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

pub fn scenario_evidence(name: &str, run: &ScenarioRun) -> CanonValue {
    match run {
        ScenarioRun::Executed { assertions } => CanonValue::Object(BTreeMap::from([
            ("name".into(), CanonValue::String(name.into())),
            ("status".into(), CanonValue::String("executed".into())),
            (
                "assertions".into(),
                CanonValue::List(
                    assertions
                        .iter()
                        .map(|assert| {
                            CanonValue::Object(BTreeMap::from([
                                (
                                    "mode".into(),
                                    CanonValue::String(assert_mode_name(assert.mode).into()),
                                ),
                                ("passed".into(), CanonValue::Bool(assert.passed)),
                                ("detail".into(), CanonValue::String(assert.detail.clone())),
                            ]))
                        })
                        .collect(),
                ),
            ),
        ])),
        ScenarioRun::Unavailable { reasons } => CanonValue::Object(BTreeMap::from([
            ("name".into(), CanonValue::String(name.into())),
            ("status".into(), CanonValue::String("unavailable".into())),
            (
                "unsupported".into(),
                CanonValue::List(reasons.iter().cloned().map(CanonValue::String).collect()),
            ),
        ])),
    }
}

fn assert_mode_name(mode: AssertMode) -> &'static str {
    match mode {
        AssertMode::Always => "always",
        AssertMode::Eventually => "eventually",
        AssertMode::AtEnd => "at_end",
    }
}

fn boolean_expr_supported(expr: &Expr) -> Result<(), String> {
    match expr.kind.as_ref() {
        ExprKind::Bool(_) => Ok(()),
        ExprKind::Unary {
            op: UnOp::Not,
            expr,
        } => boolean_expr_supported(expr),
        ExprKind::Binary {
            op: BinOp::And | BinOp::Or,
            lhs,
            rhs,
        } => {
            boolean_expr_supported(lhs)?;
            boolean_expr_supported(rhs)
        }
        ExprKind::Paren(inner) => boolean_expr_supported(inner),
        other => Err(format!("{other:?}")),
    }
}

fn eval_boolean(expr: &Expr) -> Result<bool, String> {
    match expr.kind.as_ref() {
        ExprKind::Bool(value) => Ok(*value),
        ExprKind::Unary {
            op: UnOp::Not,
            expr,
        } => eval_boolean(expr).map(|value| !value),
        ExprKind::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => Ok(eval_boolean(lhs)? && eval_boolean(rhs)?),
        ExprKind::Binary {
            op: BinOp::Or,
            lhs,
            rhs,
        } => Ok(eval_boolean(lhs)? || eval_boolean(rhs)?),
        ExprKind::Paren(inner) => eval_boolean(inner),
        other => Err(format!("unsupported boolean expression: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_ast::parse_file;

    #[test]
    fn classifies_boolean_at_end_scenario_as_executable() {
        let src = r#"
package t @ 1.0.0
scenario Smoke {
  seed 1
  assert at end { true }
}
"#;
        let (file, diags) = parse_file(src);
        assert!(!diags.has_errors());
        let scenario = match &file.decls[0] {
            Decl::Scenario(scenario) => scenario,
            _ => panic!("expected scenario"),
        };
        assert_eq!(classify(scenario), ScenarioClass::Executable);
        assert!(execute(scenario).passed());
    }

    #[test]
    fn rejects_seed_each_and_transaction_blocks() {
        let src = r#"
package t @ 1.0.0
scenario S {
  seed each 1
  setup { }
  assert at end { true }
}
"#;
        let (file, diags) = parse_file(src);
        assert!(!diags.has_errors());
        let scenario = match &file.decls[0] {
            Decl::Scenario(scenario) => scenario,
            _ => panic!("expected scenario"),
        };
        assert!(matches!(
            classify(scenario),
            ScenarioClass::Unavailable { .. }
        ));
    }

    #[test]
    fn evaluates_boolean_operators() {
        let src = r#"
package t @ 1.0.0
scenario Logic {
  seed 1
  assert at end { (true or false) and !false }
}
"#;
        let (file, _) = parse_file(src);
        let scenario = match &file.decls[0] {
            Decl::Scenario(scenario) => scenario,
            _ => panic!("expected scenario"),
        };
        assert!(execute(scenario).passed());
    }

    #[test]
    fn false_assertion_fails_execution() {
        let src = r#"
package t @ 1.0.0
scenario Bad {
  seed 1
  assert at end { false }
}
"#;
        let (file, _) = parse_file(src);
        let scenario = match &file.decls[0] {
            Decl::Scenario(scenario) => scenario,
            _ => panic!("expected scenario"),
        };
        assert!(!execute(scenario).passed());
    }

    #[test]
    fn parser_rejects_missing_and_duplicate_seed() {
        let missing = r#"
package t @ 1.0.0
scenario S { assert at end { true } }
"#;
        let (_, diags) = parse_file(missing);
        assert!(diags.has_errors());

        let duplicate = r#"
package t @ 1.0.0
scenario S {
  seed 1
  seed 2
  assert at end { true }
}
"#;
        let (_, diags) = parse_file(duplicate);
        assert!(diags.has_errors());
    }
}
