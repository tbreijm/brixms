//! Native analyzer for detecting type conflicts (ADR-0009 §5).

use std::collections::BTreeMap;

use super::syntax::{NExpr, NLit, NRow, NTy, NativeQuery, NativeSource, Origin, Sym};

/// Native type conflicts (grows per ADR-0009 slice).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NConflict {
    Mismatch {
        left: NTy,
        right: NTy,
    },
    Occurs {
        var: u32,
        into: NTy,
    },
    /// A call whose argument count matches NO candidate signature (N2).
    Arity {
        expected: u32,
        found: u32,
    },
    /// Unknown field on record/rel or closed row mismatch.
    UnknownField {
        field: Sym,
    },
}

/// Analysis report for the native checker.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NativeReport {
    pub has_types: Vec<(Origin, NTy)>,
    pub conflicts: Vec<NConflict>,
}

impl NativeReport {
    pub fn is_consistent(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Resolves type variable indirections in `ty` under `subst`.
pub fn resolve<'a>(ty: &'a NTy, subst: &'a BTreeMap<u32, NTy>) -> &'a NTy {
    let mut curr = ty;
    while let NTy::Var(v) = curr {
        if let Some(next) = subst.get(v) {
            curr = next;
        } else {
            break;
        }
    }
    curr
}

/// Occurs check: returns true if variable `v` appears free in `ty` under `subst`.
pub fn occurs(v: u32, ty: &NTy, subst: &BTreeMap<u32, NTy>) -> bool {
    match resolve(ty, subst) {
        NTy::Var(v2) => v == *v2,
        NTy::Fn { params, ret } => {
            params.iter().any(|p| occurs(v, p, subst)) || occurs(v, ret, subst)
        }
        NTy::Option(inner) => occurs(v, inner, subst),
        NTy::Record(row) | NTy::Rel(row) => row.fields.iter().any(|(_, fty)| occurs(v, fty, subst)),
        _ => false,
    }
}

/// Zonks `ty` recursively, replacing bound type variables using `subst`.
pub fn zonk(ty: &NTy, subst: &BTreeMap<u32, NTy>) -> NTy {
    match resolve(ty, subst) {
        NTy::Unit => NTy::Unit,
        NTy::Bool => NTy::Bool,
        NTy::Str => NTy::Str,
        NTy::Int => NTy::Int,
        NTy::F64 => NTy::F64,
        NTy::Var(v) => NTy::Var(*v),
        NTy::Error => NTy::Error,
        NTy::Fn { params, ret } => NTy::Fn {
            params: params.iter().map(|p| zonk(p, subst)).collect(),
            ret: Box::new(zonk(ret, subst)),
        },
        NTy::Option(inner) => NTy::Option(Box::new(zonk(inner, subst))),
        NTy::Record(row) => NTy::Record(zonk_row(row, subst)),
        NTy::Rel(row) => NTy::Rel(zonk_row(row, subst)),
    }
}

fn zonk_row(row: &NRow, subst: &BTreeMap<u32, NTy>) -> NRow {
    NRow {
        fields: row
            .fields
            .iter()
            .map(|(name, fty)| (name.clone(), zonk(fty, subst)))
            .collect(),
        open: row.open,
    }
}

fn unify_rows(
    r1: &NRow,
    r2: &NRow,
    subst: &mut BTreeMap<u32, NTy>,
    conflicts: &mut Vec<NConflict>,
) -> (Vec<(Sym, NTy)>, bool, bool) {
    let mut unified_fields = Vec::new();
    let mut has_err = false;

    for (name1, fty1) in &r1.fields {
        if let Some((_, fty2)) = r2.fields.iter().find(|(n, _)| n == name1) {
            let u = unify(fty1, fty2, subst, conflicts);
            if u == NTy::Error {
                has_err = true;
            }
            unified_fields.push((name1.clone(), u));
        } else {
            if !r2.open {
                conflicts.push(NConflict::UnknownField {
                    field: name1.clone(),
                });
                has_err = true;
            }
            unified_fields.push((name1.clone(), zonk(fty1, subst)));
        }
    }

    for (name2, fty2) in &r2.fields {
        if !r1.fields.iter().any(|(n, _)| n == name2) {
            if !r1.open {
                conflicts.push(NConflict::UnknownField {
                    field: name2.clone(),
                });
                has_err = true;
            }
            unified_fields.push((name2.clone(), zonk(fty2, subst)));
        }
    }

    (unified_fields, r1.open && r2.open, has_err)
}

/// Unifies two native types `t1` and `t2` under `subst`.
///
/// On failure, emits matching `NConflict` into `conflicts` and returns `NTy::Error`
/// for error isolation (matching brix_ir Ty::Error semantics).
pub fn unify(
    t1: &NTy,
    t2: &NTy,
    subst: &mut BTreeMap<u32, NTy>,
    conflicts: &mut Vec<NConflict>,
) -> NTy {
    let r1 = resolve(t1, subst).clone();
    let r2 = resolve(t2, subst).clone();

    match (&r1, &r2) {
        (NTy::Error, _) | (_, NTy::Error) => NTy::Error,
        (NTy::Var(v1), NTy::Var(v2)) if v1 == v2 => NTy::Var(*v1),
        (NTy::Var(v1), t) => {
            let v = *v1;
            if occurs(v, t, subst) {
                conflicts.push(NConflict::Occurs {
                    var: v,
                    into: zonk(t, subst),
                });
                NTy::Error
            } else {
                subst.insert(v, t.clone());
                t.clone()
            }
        }
        (t, NTy::Var(v2)) => {
            let v = *v2;
            if occurs(v, t, subst) {
                conflicts.push(NConflict::Occurs {
                    var: v,
                    into: zonk(t, subst),
                });
                NTy::Error
            } else {
                subst.insert(v, t.clone());
                t.clone()
            }
        }
        (NTy::Unit, NTy::Unit) => NTy::Unit,
        (NTy::Bool, NTy::Bool) => NTy::Bool,
        (NTy::Str, NTy::Str) => NTy::Str,
        (NTy::Int, NTy::Int) => NTy::Int,
        (NTy::F64, NTy::F64) => NTy::F64,
        (NTy::Option(a), NTy::Option(b)) => {
            let u = unify(a, b, subst, conflicts);
            if u == NTy::Error {
                NTy::Error
            } else {
                NTy::Option(Box::new(u))
            }
        }
        (NTy::Record(row1), NTy::Record(row2)) => {
            let (fields, open, has_err) = unify_rows(row1, row2, subst, conflicts);
            if has_err {
                NTy::Error
            } else {
                NTy::Record(NRow { fields, open })
            }
        }
        (NTy::Rel(row1), NTy::Rel(row2)) => {
            let (fields, open, has_err) = unify_rows(row1, row2, subst, conflicts);
            if has_err {
                NTy::Error
            } else {
                NTy::Rel(NRow { fields, open })
            }
        }
        (
            NTy::Fn {
                params: p1,
                ret: ret1,
            },
            NTy::Fn {
                params: p2,
                ret: ret2,
            },
        ) => {
            if p1.len() != p2.len() {
                conflicts.push(NConflict::Mismatch {
                    left: zonk(&r1, subst),
                    right: zonk(&r2, subst),
                });
                NTy::Error
            } else {
                let mut unified_params = Vec::with_capacity(p1.len());
                let mut has_err = false;
                for (a, b) in p1.iter().zip(p2.iter()) {
                    let u = unify(a, b, subst, conflicts);
                    if u == NTy::Error {
                        has_err = true;
                    }
                    unified_params.push(u);
                }
                let u_ret = unify(ret1, ret2, subst, conflicts);
                if u_ret == NTy::Error {
                    has_err = true;
                }
                if has_err {
                    NTy::Error
                } else {
                    NTy::Fn {
                        params: unified_params,
                        ret: Box::new(u_ret),
                    }
                }
            }
        }
        _ => {
            conflicts.push(NConflict::Mismatch {
                left: zonk(&r1, subst),
                right: zonk(&r2, subst),
            });
            NTy::Error
        }
    }
}

/// Helper context for inference during `analyze`.
struct InferContext<'a> {
    src: &'a NativeSource,
    env: BTreeMap<Sym, NTy>,
    subst: BTreeMap<u32, NTy>,
    has_types: Vec<(Origin, NTy)>,
    conflicts: Vec<NConflict>,
    next_var: u32,
}

impl<'a> InferContext<'a> {
    fn fresh_var(&mut self) -> u32 {
        let v = self.next_var;
        self.next_var += 1;
        v
    }

    fn infer_expr(&mut self, expr: &NExpr) -> NTy {
        let origin = expr.origin();
        let ty = match expr {
            NExpr::Lit { lit, .. } => match lit {
                NLit::Int(_) => NTy::Int,
                NLit::Bool(_) => NTy::Bool,
                NLit::Str(_) => NTy::Str,
                NLit::Unit => NTy::Unit,
            },
            NExpr::Var { name, .. } => {
                if let Some(t) = self.env.get(name) {
                    t.clone()
                } else {
                    let v = self.fresh_var();
                    let t = NTy::Var(v);
                    self.env.insert(name.clone(), t.clone());
                    t
                }
            }
            NExpr::Call { func, args, .. } => {
                let arg_tys: Vec<NTy> = args.iter().map(|a| self.infer_expr(a)).collect();
                if let Some(sigs) = self.src.sigs.get(func) {
                    if let Some(sig) = sigs.iter().find(|s| s.params.len() == arg_tys.len()) {
                        for (arg_ty, param_ty) in arg_tys.iter().zip(&sig.params) {
                            unify(arg_ty, param_ty, &mut self.subst, &mut self.conflicts);
                        }
                        sig.ret.clone()
                    } else {
                        // Function resolves, but NO candidate signature matches the
                        // argument count (N2). This is an Arity conflict — distinct
                        // from a same-arity type Mismatch, which the unify branch
                        // above reports. `expected` reports the first candidate's
                        // arity (category-set parity does not depend on the value;
                        // brix-ir's `arity_ok` filter likewise keys on the count).
                        let expected = sigs.first().map(|s| s.params.len()).unwrap_or(0) as u32;
                        self.conflicts.push(NConflict::Arity {
                            expected,
                            found: arg_tys.len() as u32,
                        });
                        NTy::Error
                    }
                } else {
                    // Unknown function
                    let dummy_ret = NTy::Var(self.fresh_var());
                    let expected = NTy::Fn {
                        params: arg_tys.clone(),
                        ret: Box::new(dummy_ret.clone()),
                    };
                    let found = NTy::Error;
                    self.conflicts.push(NConflict::Mismatch {
                        left: expected,
                        right: found,
                    });
                    NTy::Error
                }
            }
            NExpr::Record { fields, .. } => {
                let inf_fields = fields
                    .iter()
                    .map(|(name, expr)| (name.clone(), self.infer_expr(expr)))
                    .collect();
                NTy::Record(NRow {
                    fields: inf_fields,
                    open: false,
                })
            }
            NExpr::Field { base, field, .. } => {
                let base_ty = self.infer_expr(base);
                let resolved = resolve(&base_ty, &self.subst).clone();
                match resolved {
                    NTy::Record(row) | NTy::Rel(row) => {
                        if let Some((_, fty)) = row.fields.iter().find(|(n, _)| n == field) {
                            fty.clone()
                        } else {
                            self.conflicts.push(NConflict::UnknownField {
                                field: field.clone(),
                            });
                            NTy::Error
                        }
                    }
                    NTy::Var(_) | NTy::Error => NTy::Error,
                    _ => {
                        self.conflicts.push(NConflict::UnknownField {
                            field: field.clone(),
                        });
                        NTy::Error
                    }
                }
            }
        };

        self.has_types.push((origin, ty.clone()));
        ty
    }

    fn analyze_query(&mut self, query: &NativeQuery) {
        for (param_name, param_ty) in &query.params {
            self.env.insert(param_name.clone(), param_ty.clone());
        }
        let yielded_ty = self.infer_expr(&query.yields);
        let expected = match (&yielded_ty, resolve(&query.result, &self.subst)) {
            (_, NTy::Rel(_)) => match yielded_ty {
                NTy::Record(ref row) | NTy::Rel(ref row) => NTy::Rel(row.clone()),
                ref ty => NTy::Rel(NRow {
                    fields: vec![("value".to_string(), ty.clone())],
                    open: false,
                }),
            },
            _ => yielded_ty,
        };
        unify(
            &expected,
            &query.result,
            &mut self.subst,
            &mut self.conflicts,
        );
    }
}

/// Runs type analysis over a `NativeSource` and returns a `NativeReport`.
pub fn analyze(src: &NativeSource) -> NativeReport {
    let mut cx = InferContext {
        src,
        env: BTreeMap::new(),
        subst: BTreeMap::new(),
        has_types: Vec::new(),
        conflicts: Vec::new(),
        next_var: 100_000,
    };

    for query in &src.queries {
        cx.analyze_query(query);
    }

    let has_types = cx
        .has_types
        .into_iter()
        .map(|(origin, ty)| (origin, zonk(&ty, &cx.subst)))
        .collect();

    NativeReport {
        has_types,
        conflicts: cx.conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::super::syntax::SigTable;
    use super::*;

    #[test]
    fn unify_occurs_into_fn_is_detected() {
        // unify(?0, Fn(?0) -> Int) must fail the occurs-check (?0 occurs in the
        // function type it would be bound to). Exercises the native occurs path
        // directly — the container corpus fixtures cover it differentially later.
        let mut subst = BTreeMap::new();
        let mut conflicts = Vec::new();
        let fn_ty = NTy::Fn {
            params: vec![NTy::Var(0)],
            ret: Box::new(NTy::Int),
        };
        let result = unify(&NTy::Var(0), &fn_ty, &mut subst, &mut conflicts);
        assert_eq!(result, NTy::Error, "occurs failure must yield Error");
        assert_eq!(conflicts.len(), 1);
        assert!(
            matches!(conflicts[0], NConflict::Occurs { var: 0, .. }),
            "expected Occurs, got {:?}",
            conflicts[0]
        );
        // ?0 must NOT have been bound (Error never enters the substitution).
        assert!(!subst.contains_key(&0), "occurs var must stay unbound");
    }

    #[test]
    fn unify_scalar_mismatch_is_detected() {
        let mut subst = BTreeMap::new();
        let mut conflicts = Vec::new();
        let result = unify(&NTy::Int, &NTy::Bool, &mut subst, &mut conflicts);
        assert_eq!(result, NTy::Error);
        assert!(matches!(conflicts.as_slice(), [NConflict::Mismatch { .. }]));
    }

    #[test]
    fn call_with_no_arity_matching_candidate_is_arity() {
        use crate::native::syntax::{NSig, NativeQuery, SigTable};
        // f() called with 0 args, but f : (Int) -> Int (1 param) => Arity, not Mismatch.
        let mut sigs = SigTable::new();
        sigs.insert(
            "f".to_string(),
            NSig {
                params: vec![NTy::Int],
                ret: NTy::Int,
            },
        );
        let src = NativeSource {
            queries: vec![NativeQuery {
                name: "Q".to_string(),
                params: vec![],
                yields: NExpr::Call {
                    origin: 0,
                    func: "f".to_string(),
                    args: vec![],
                },
                result: NTy::Var(0),
            }],
            sigs,
        };
        let report = analyze(&src);
        assert!(
            matches!(
                report.conflicts.as_slice(),
                [NConflict::Arity {
                    expected: 1,
                    found: 0
                }]
            ),
            "expected one Arity{{1,0}}, got {:?}",
            report.conflicts
        );
    }

    #[test]
    fn call_matching_a_non_first_overload_candidate_is_no_conflict() {
        use crate::native::syntax::{NSig, NativeQuery, SigTable};
        // g(Int, Int) with overloads g(Int) and g(Int,Int) => the 2-param
        // candidate matches => NO Arity conflict (discriminator).
        let mut sigs = SigTable::new();
        sigs.insert(
            "g".to_string(),
            NSig {
                params: vec![NTy::Int],
                ret: NTy::Int,
            },
        );
        sigs.insert(
            "g".to_string(),
            NSig {
                params: vec![NTy::Int, NTy::Int],
                ret: NTy::Int,
            },
        );
        let src = NativeSource {
            queries: vec![NativeQuery {
                name: "Q".to_string(),
                params: vec![],
                yields: NExpr::Call {
                    origin: 0,
                    func: "g".to_string(),
                    args: vec![
                        NExpr::Lit {
                            origin: 1,
                            lit: NLit::Int(1),
                        },
                        NExpr::Lit {
                            origin: 2,
                            lit: NLit::Int(2),
                        },
                    ],
                },
                result: NTy::Var(0),
            }],
            sigs,
        };
        let report = analyze(&src);
        assert!(
            report.is_consistent(),
            "matching a non-first candidate must not conflict, got {:?}",
            report.conflicts
        );
    }

    #[test]
    fn error_unifies_with_anything_without_binding() {
        let mut subst = BTreeMap::new();
        let mut conflicts = Vec::new();
        let result = unify(&NTy::Error, &NTy::Var(0), &mut subst, &mut conflicts);
        assert_eq!(result, NTy::Error);
        assert!(
            conflicts.is_empty(),
            "Error isolation raises no new conflict"
        );
        assert!(subst.is_empty(), "Error must not bind the variable");
    }

    #[test]
    fn record_field_present_is_ok() {
        let rec_ty = NTy::Record(NRow {
            fields: vec![("a".to_string(), NTy::Int)],
            open: false,
        });
        let src = NativeSource {
            queries: vec![NativeQuery {
                name: "Q".to_string(),
                params: vec![("r".to_string(), rec_ty)],
                yields: NExpr::Field {
                    origin: 0,
                    base: Box::new(NExpr::Var {
                        origin: 1,
                        name: "r".to_string(),
                    }),
                    field: "a".to_string(),
                },
                result: NTy::Int,
            }],
            sigs: SigTable::new(),
        };
        let report = analyze(&src);
        assert!(
            report.is_consistent(),
            "expected clean field access, got {:?}",
            report.conflicts
        );
    }

    #[test]
    fn record_field_absent_closed_gives_unknown_field() {
        let rec_ty = NTy::Record(NRow {
            fields: vec![("a".to_string(), NTy::Int)],
            open: false,
        });
        let src = NativeSource {
            queries: vec![NativeQuery {
                name: "Q".to_string(),
                params: vec![("r".to_string(), rec_ty)],
                yields: NExpr::Field {
                    origin: 0,
                    base: Box::new(NExpr::Var {
                        origin: 1,
                        name: "r".to_string(),
                    }),
                    field: "absent".to_string(),
                },
                result: NTy::Int,
            }],
            sigs: SigTable::new(),
        };
        let report = analyze(&src);
        assert!(
            matches!(report.conflicts.as_slice(), [NConflict::UnknownField { field }] if field == "absent"),
            "expected UnknownField for 'absent', got {:?}",
            report.conflicts
        );
    }

    #[test]
    fn open_row_extra_field_gives_no_conflict() {
        let mut subst = BTreeMap::new();
        let mut conflicts = Vec::new();
        let r1 = NRow {
            fields: vec![("a".to_string(), NTy::Int), ("b".to_string(), NTy::Bool)],
            open: false,
        };
        let r2 = NRow {
            fields: vec![("a".to_string(), NTy::Int)],
            open: true,
        };
        let res = unify(
            &NTy::Record(r1),
            &NTy::Record(r2),
            &mut subst,
            &mut conflicts,
        );
        assert_ne!(res, NTy::Error);
        assert!(
            conflicts.is_empty(),
            "open row extra field must give no conflict, got {:?}",
            conflicts
        );
    }

    #[test]
    fn occurs_into_option_gives_occurs() {
        let mut subst = BTreeMap::new();
        let mut conflicts = Vec::new();
        let opt_ty = NTy::Option(Box::new(NTy::Var(0)));
        let res = unify(&NTy::Var(0), &opt_ty, &mut subst, &mut conflicts);
        assert_eq!(res, NTy::Error);
        assert!(
            matches!(conflicts.as_slice(), [NConflict::Occurs { var: 0, .. }]),
            "expected Occurs into Option, got {:?}",
            conflicts
        );
    }

    #[test]
    fn occurs_into_rel_row_gives_occurs() {
        let mut subst = BTreeMap::new();
        let mut conflicts = Vec::new();
        let rel_ty = NTy::Rel(NRow {
            fields: vec![("inner".to_string(), NTy::Var(0))],
            open: false,
        });
        let res = unify(&NTy::Var(0), &rel_ty, &mut subst, &mut conflicts);
        assert_eq!(res, NTy::Error);
        assert!(
            matches!(conflicts.as_slice(), [NConflict::Occurs { var: 0, .. }]),
            "expected Occurs into Rel row, got {:?}",
            conflicts
        );
    }
}
