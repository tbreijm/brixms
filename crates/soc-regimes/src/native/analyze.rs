//! Native analyzer for detecting type conflicts (ADR-0009 §5).

use std::collections::BTreeMap;

use super::syntax::{NExpr, NLit, NTy, NativeQuery, NativeSource, Origin, Sym};

/// Native type conflicts supported in Slice N1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NConflict {
    Mismatch { left: NTy, right: NTy },
    Occurs { var: u32, into: NTy },
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
    }
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
                        // Arity or overload mismatch
                        let expected = sigs
                            .first()
                            .map(|s| NTy::Fn {
                                params: s.params.clone(),
                                ret: Box::new(s.ret.clone()),
                            })
                            .unwrap_or(NTy::Error);
                        let found = NTy::Fn {
                            params: arg_tys,
                            ret: Box::new(NTy::Error),
                        };
                        self.conflicts.push(NConflict::Mismatch {
                            left: expected,
                            right: found,
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
        };

        self.has_types.push((origin, ty.clone()));
        ty
    }

    fn analyze_query(&mut self, query: &NativeQuery) {
        for (param_name, param_ty) in &query.params {
            self.env.insert(param_name.clone(), param_ty.clone());
        }
        let yielded_ty = self.infer_expr(&query.yields);
        unify(
            &yielded_ty,
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
}
