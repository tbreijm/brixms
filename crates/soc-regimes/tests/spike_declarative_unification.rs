// SPIKE — proof-of-concept for ADR-0005 declarative unification; not production; validates the mechanism before the full type-realization regime.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{compose, ConfigId, RegimeId, Witness, WitnessId};

/// Minimal type representation for the unification spike.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ty {
    Con(&'static str),
    Fn(Box<Ty>, Box<Ty>),
    Var(u32),
}

impl Canonical for Ty {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Ty::Con(name) => {
                w.write_enum(0, |w| w.write_str(name));
            }
            Ty::Fn(param, ret) => {
                w.write_enum(1, |w| {
                    param.canon_write(w);
                    ret.canon_write(w);
                });
            }
            Ty::Var(v) => {
                w.write_enum(2, |w| w.write_uint(*v as u64));
            }
        }
    }
}

impl Ty {
    pub fn digest(&self) -> Digest {
        self.canon_digest(Domain::Value)
    }
}

/// Substitution as an explicit, immutable, content-addressed context.
/// Each unification step produces a new `Subst` rather than mutating state in place.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Subst(pub BTreeMap<u32, Ty>);

impl Subst {
    pub fn empty() -> Self {
        Self(BTreeMap::new())
    }

    /// Pure structural extension: returns a NEW `Subst` extended with `var |-> ty`.
    /// Does NOT mutate `self`.
    pub fn extend(&self, var: u32, ty: Ty) -> Self {
        let mut map = self.0.clone();
        map.insert(var, ty);
        Self(map)
    }

    pub fn get(&self, var: u32) -> Option<&Ty> {
        self.0.get(&var)
    }

    pub fn digest(&self) -> Digest {
        self.canon_digest(Domain::Value)
    }

    /// Resolves `ty`'s top-level constructor in this substitution context by walking variable chains.
    pub fn resolve<'a>(&'a self, ty: &'a Ty) -> &'a Ty {
        let mut curr = ty;
        while let Ty::Var(v) = curr {
            if let Some(next) = self.0.get(v) {
                curr = next;
            } else {
                break;
            }
        }
        curr
    }

    /// Fully resolves (zonks) a type, replacing all bound variables recursively.
    pub fn zonk(&self, ty: &Ty) -> Ty {
        match self.resolve(ty) {
            Ty::Con(name) => Ty::Con(name),
            Ty::Var(v) => Ty::Var(*v),
            Ty::Fn(a, b) => Ty::Fn(Box::new(self.zonk(a)), Box::new(self.zonk(b))),
        }
    }

    /// Fully resolves all variables in the domain of the substitution.
    pub fn zonk_all(&self) -> BTreeMap<u32, Ty> {
        self.0
            .keys()
            .map(|&v| (v, self.zonk(&Ty::Var(v))))
            .collect()
    }
}

impl Canonical for Subst {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_map(
            self.0
                .iter()
                .map(|(k, v)| ((*k as u64).canon_bytes(), v.canon_bytes())),
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conflict {
    Mismatch,
    InfiniteType,
}

/// Occurs-check: checks if `v` occurs free in `ty` under substitution context `s`.
fn occurs(v: u32, ty: &Ty, s: &Subst) -> bool {
    match s.resolve(ty) {
        Ty::Var(v2) => v == *v2,
        Ty::Con(_) => false,
        Ty::Fn(a, b) => occurs(v, a, s) || occurs(v, b, s),
    }
}

/// Declarative unification as SOC context narrowing over explicit immutable context `s`.
pub fn unify(t1: &Ty, t2: &Ty, s: &Subst) -> Result<Subst, Conflict> {
    let r1 = s.resolve(t1);
    let r2 = s.resolve(t2);

    match (r1, r2) {
        (Ty::Var(v1), Ty::Var(v2)) if v1 == v2 => Ok(s.clone()),
        (Ty::Var(v1), t) => {
            if occurs(*v1, t, s) {
                Err(Conflict::InfiniteType)
            } else {
                Ok(s.extend(*v1, t.clone()))
            }
        }
        (t, Ty::Var(v2)) => {
            if occurs(*v2, t, s) {
                Err(Conflict::InfiniteType)
            } else {
                Ok(s.extend(*v2, t.clone()))
            }
        }
        (Ty::Con(a), Ty::Con(b)) => {
            if a == b {
                Ok(s.clone())
            } else {
                Err(Conflict::Mismatch)
            }
        }
        (Ty::Fn(a1, b1), Ty::Fn(a2, b2)) => {
            let s1 = unify(a1, a2, s)?;
            unify(b1, b2, &s1)
        }
        _ => Err(Conflict::Mismatch),
    }
}

/// Reference imperative HM unification engine (uses mutable map state).
#[derive(Clone, Debug, Default)]
pub struct RefSubst {
    pub map: BTreeMap<u32, Ty>,
}

impl RefSubst {
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub fn resolve(&self, ty: &Ty) -> Ty {
        let mut curr = ty.clone();
        while let Ty::Var(v) = curr {
            if let Some(next) = self.map.get(&v) {
                curr = next.clone();
            } else {
                break;
            }
        }
        curr
    }

    pub fn occurs(&self, v: u32, ty: &Ty) -> bool {
        match self.resolve(ty) {
            Ty::Var(v2) => v == v2,
            Ty::Con(_) => false,
            Ty::Fn(a, b) => self.occurs(v, &a) || self.occurs(v, &b),
        }
    }

    pub fn unify(&mut self, t1: &Ty, t2: &Ty) -> Result<(), Conflict> {
        let r1 = self.resolve(t1);
        let r2 = self.resolve(t2);

        match (r1, r2) {
            (Ty::Var(v1), Ty::Var(v2)) if v1 == v2 => Ok(()),
            (Ty::Var(v1), t) => {
                if self.occurs(v1, &t) {
                    Err(Conflict::InfiniteType)
                } else {
                    self.map.insert(v1, t);
                    Ok(())
                }
            }
            (t, Ty::Var(v2)) => {
                if self.occurs(v2, &t) {
                    Err(Conflict::InfiniteType)
                } else {
                    self.map.insert(v2, t);
                    Ok(())
                }
            }
            (Ty::Con(a), Ty::Con(b)) => {
                if a == b {
                    Ok(())
                } else {
                    Err(Conflict::Mismatch)
                }
            }
            (Ty::Fn(a1, b1), Ty::Fn(a2, b2)) => {
                self.unify(&a1, &a2)?;
                self.unify(&b1, &b2)
            }
            _ => Err(Conflict::Mismatch),
        }
    }

    pub fn zonk(&self, ty: &Ty) -> Ty {
        match self.resolve(ty) {
            Ty::Con(name) => Ty::Con(name),
            Ty::Var(v) => Ty::Var(v),
            Ty::Fn(a, b) => Ty::Fn(Box::new(self.zonk(&a)), Box::new(self.zonk(&b))),
        }
    }

    pub fn zonk_all(&self) -> BTreeMap<u32, Ty> {
        self.map
            .keys()
            .map(|&v| (v, self.zonk(&Ty::Var(v))))
            .collect()
    }
}

/// Computes the `WitnessId` of a single narrowing step `before -> before.extend(var, ty)`.
pub fn narrowing_witness(before: &Subst, var: u32, ty: &Ty) -> WitnessId {
    let after = before.extend(var, ty.clone());
    let src = ConfigId::from_canon(&before.canon_bytes());
    let dst = ConfigId::from_canon(&after.canon_bytes());
    let regime = RegimeId::named("brix.type.unify.narrow@0.1");
    Witness::new(src, dst, regime).id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty_con(name: &'static str) -> Ty {
        Ty::Con(name)
    }

    fn ty_var(v: u32) -> Ty {
        Ty::Var(v)
    }

    fn ty_fn(param: Ty, ret: Ty) -> Ty {
        Ty::Fn(Box::new(param), Box::new(ret))
    }

    #[test]
    fn test_var_concrete() {
        let t1 = ty_var(0);
        let t2 = ty_con("Int");
        let s0 = Subst::empty();

        let decl_subst = unify(&t1, &t2, &s0).expect("declarative unification should succeed");
        let mut ref_subst = RefSubst::new();
        ref_subst
            .unify(&t1, &t2)
            .expect("reference unification should succeed");

        let expected_map: BTreeMap<u32, Ty> = [(0, ty_con("Int"))].into_iter().collect();
        assert_eq!(decl_subst.zonk_all(), expected_map);
        assert_eq!(decl_subst.zonk_all(), ref_subst.zonk_all());
    }

    #[test]
    fn test_structural() {
        // unify(Fn(Var(0), Con("Bool")), Fn(Con("Int"), Var(1)))
        let t1 = ty_fn(ty_var(0), ty_con("Bool"));
        let t2 = ty_fn(ty_con("Int"), ty_var(1));
        let s0 = Subst::empty();

        let decl_subst = unify(&t1, &t2, &s0).expect("declarative unification should succeed");
        let mut ref_subst = RefSubst::new();
        ref_subst
            .unify(&t1, &t2)
            .expect("reference unification should succeed");

        let expected_map: BTreeMap<u32, Ty> = [(0, ty_con("Int")), (1, ty_con("Bool"))]
            .into_iter()
            .collect();
        assert_eq!(decl_subst.zonk_all(), expected_map);
        assert_eq!(decl_subst.zonk_all(), ref_subst.zonk_all());
    }

    #[test]
    fn test_mismatch() {
        let t1 = ty_con("Int");
        let t2 = ty_con("Bool");
        let s0 = Subst::empty();

        let decl_res = unify(&t1, &t2, &s0);
        let mut ref_subst = RefSubst::new();
        let ref_res = ref_subst.unify(&t1, &t2);

        assert_eq!(decl_res, Err(Conflict::Mismatch));
        assert_eq!(ref_res, Err(Conflict::Mismatch));
    }

    #[test]
    fn test_occurs_check() {
        // unify(Var(0), Fn(Var(0), Con("Int")))
        let t1 = ty_var(0);
        let t2 = ty_fn(ty_var(0), ty_con("Int"));
        let s0 = Subst::empty();

        let decl_res = unify(&t1, &t2, &s0);
        let mut ref_subst = RefSubst::new();
        let ref_res = ref_subst.unify(&t1, &t2);

        assert_eq!(decl_res, Err(Conflict::InfiniteType));
        assert_eq!(ref_res, Err(Conflict::InfiniteType));
    }

    #[test]
    fn test_determinism() {
        let t1 = ty_fn(ty_var(0), ty_con("Bool"));
        let t2 = ty_fn(ty_con("Int"), ty_var(1));
        let s0 = Subst::empty();

        let res1 = unify(&t1, &t2, &s0).expect("unify 1");
        let res2 = unify(&t1, &t2, &s0).expect("unify 2");

        assert_eq!(res1.digest(), res2.digest());
        assert_eq!(res1, res2);
    }

    #[test]
    fn test_witness_composition() {
        let s0 = Subst::empty();
        let var0_ty = ty_con("Int");
        let var1_ty = ty_con("Bool");

        let s1 = s0.extend(0, var0_ty.clone());
        let w1 = narrowing_witness(&s0, 0, &var0_ty);
        let w2 = narrowing_witness(&s1, 1, &var1_ty);

        let composite1 = compose(w2, w1);
        let composite2 = compose(w2, w1);

        assert_eq!(composite1, composite2);

        // Composition is non-commutative
        let reverse_composite = compose(w1, w2);
        assert_ne!(composite1, reverse_composite);
    }
}
