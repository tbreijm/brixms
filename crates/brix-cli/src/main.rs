//! `brix` — the Brix command-line driver (ADR-0010).
//!
//! `brix check <file.brix>`: parse a `.brix` program, lower it, and type-check
//! each `let` binding, printing its inferred type and epistemic grade
//! (`@Derived`/`@Audited`/`@Proven`). This is the first runnable Brix tool: it
//! exposes the L1 parser + L2 lowering as a command. Bindings outside the
//! current lowering fragment are reported honestly as not-yet-supported rather
//! than failing the whole file.
//!
//! A `run` subcommand (settlement to a fixpoint) arrives with L3.

use std::process::ExitCode;

use brix_lower::check_module;
use brix_syntax::parse;
use soc_regimes::type_realization::Ty;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check") => match args.get(2) {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(src) => {
                    let (report, had_error) = check_report(&src);
                    print!("{report}");
                    if had_error {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => {
                    eprintln!("brix: cannot read {path}: {e}");
                    ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("usage: brix check <file.brix>");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: brix check <file.brix>");
            ExitCode::FAILURE
        }
    }
}

/// Parse + lower + type-check `source`, returning a human-readable report and
/// whether any binding failed (parse error, or a binding that did not lower /
/// type-check / prove). Separated from `main` so it can be unit-tested.
fn check_report(source: &str) -> (String, bool) {
    let module = match parse(source) {
        Ok(m) => m,
        Err(e) => return (format!("parse error: {e}\n"), true),
    };

    let results = check_module(&module);
    if results.is_empty() {
        return ("(no `let` bindings to check)\n".to_string(), false);
    }

    let mut out = String::new();
    let mut had_error = false;
    for r in &results {
        match r {
            Ok(cr) => {
                out.push_str(&format!(
                    "  {} : {} @{:?}\n",
                    cr.name,
                    fmt_ty(cr.ty.as_ref()),
                    cr.outcome
                ));
            }
            Err((name, err)) => {
                had_error = true;
                out.push_str(&format!("  {name} : — (not checked: {err:?})\n"));
            }
        }
    }
    (out, had_error)
}

/// Render an inferred type for display.
fn fmt_ty(ty: Option<&Ty>) -> String {
    match ty {
        None => "?".to_string(),
        Some(Ty::Con(name)) => name.to_string(),
        Some(Ty::Var(v)) => format!("?{v}"),
        Some(Ty::Fn(a, b)) => format!("({} -> {})", fmt_ty(Some(a)), fmt_ty(Some(b))),
        Some(Ty::Record(fields)) => {
            let elems: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", fmt_ty(Some(v))))
                .collect();
            format!("{{{}}}", elems.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_identity_application_to_proven_int() {
        let src = "fn id(n) = n\nlet r = id(42)\n";
        let (report, had_error) = check_report(src);
        assert!(!had_error, "report: {report}");
        assert!(
            report.contains("r : Int @Proven"),
            "expected `r : Int @Proven`, got:\n{report}"
        );
    }

    #[test]
    fn checks_literal_to_proven_int() {
        let (report, had_error) = check_report("let x = 42\n");
        assert!(!had_error);
        assert!(report.contains("x : Int @Proven"), "{report}");
    }

    #[test]
    fn checks_record_and_field_access_to_proven() {
        let src = "let p = Item { x: 1, y: 2 }\nlet a = p.x\n";
        let (report, had_error) = check_report(src);
        assert!(!had_error, "report: {report}");
        assert!(
            report.contains("p : {x: Int, y: Int} @Proven"),
            "expected `p : {{x: Int, y: Int}} @Proven`, got:\n{report}"
        );
        assert!(
            report.contains("a : Int @Proven"),
            "expected `a : Int @Proven`, got:\n{report}"
        );
    }

    #[test]
    fn unsupported_binding_is_reported_not_crashed() {
        let (report, had_error) = check_report("let y = 1 then 2\n");
        assert!(had_error);
        assert!(report.contains("y : — (not checked"), "{report}");
    }

    #[test]
    fn parse_error_is_reported() {
        let (report, had_error) = check_report("let = 42\n");
        assert!(had_error);
        assert!(report.contains("parse error"), "{report}");
    }
}
