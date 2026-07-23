//! The shared type algebra behind both type checkers (#15 PR2: "one
//! algorithm, two observers").
//!
//! [`crate::infer`] (the trusted, non-self-hosted bootstrap unification
//! checker that zonks `Expr.ty` and accumulates `TypeError`s) and
//! [`crate::reflect`] (the fact-oriented reference analyzer that records
//! `Fact`/`Derivation`/`TypeConflict`s) used to carry independent copies of
//! `resolve`/`occurs`/row-matching/dimension logic. They silently diverged
//! in five ways because there was no single place either author had to keep
//! in sync. This module is that place: every *pure* decision (does this
//! bind a variable, does this row shape match, do these dimensions agree)
//! lives here and only here. A checker's `unify` wrapper calls into this
//! module and turns the answer into its own observation — it must never
//! re-derive the answer itself.
//!
//! The five divergences this module resolves, per the frozen #15 PR2
//! ruling (see the issue's "Trajectory plan" comment):
//! - **Probability↔F64 bridge**: kept, in [`step`] (a tracked v1 erratum;
//!   PR5 fences it behind an explicit checked op).
//! - **Dimension-vs-variable**: solving wins, in [`same_dimension_step`] /
//!   [`dimension_binary_step`] — a missing ground dimension unifies instead
//!   of conflicting.
//! - **Row symmetry**: [`match_rows`] checks both directions.
//! - **Option/Result descent**: [`step`] descends into both.
//! - **Occurs-check depth**: [`occurs`] descends into `Record`/`Rel` rows.

use std::collections::BTreeMap;

use crate::ident::Ident;
use crate::types::{
    dimensions_div, dimensions_mul, money_dimensions, quantity_dimensions, Dimensions, Row,
    RowField, RowTail, Ty, TyVar,
};

/// Fully zonk `ty` through `subst`, recursing into every structural
/// position (`Option`/`Result`/`Record`/`Rel` rows), not just the top
/// level. Both checkers must resolve this deeply: the parity contract's
/// "every zonked `Expr.ty` mirrors a `Fact::HasType` with the equal
/// resolved type" only holds if both sides zonk rows the same way.
pub fn resolve(subst: &BTreeMap<TyVar, Ty>, ty: Ty) -> Ty {
    match ty {
        Ty::Var(v) => subst
            .get(&v)
            .cloned()
            .map(|bound| resolve(subst, bound))
            .unwrap_or(Ty::Var(v)),
        Ty::Option(t) => Ty::option(resolve(subst, *t)),
        Ty::Result(a, b) => Ty::Result(Box::new(resolve(subst, *a)), Box::new(resolve(subst, *b))),
        Ty::Record(row) => Ty::record(resolve_row(subst, *row)),
        Ty::Rel(row) => Ty::rel(resolve_row(subst, *row)),
        // #15 PR5: `Missing<T>` must zonk `T` the same way `Option<T>` does,
        // so a `Missing<?t>` solved via `Step::Descend` (below) actually
        // shows its resolved inner type at the top level too — the parity
        // harness's type-mirror check needs the *fully* zonked `Expr.ty`,
        // not just a zonked top-level shape with an unresolved var hiding
        // one layer down.
        Ty::Missing(t) => Ty::missing(resolve(subst, *t)),
        other => other,
    }
}

fn resolve_row(subst: &BTreeMap<TyVar, Ty>, row: Row) -> Row {
    Row {
        fields: row
            .fields
            .into_iter()
            .map(|field| RowField {
                name: field.name,
                ty: resolve(subst, field.ty),
            })
            .collect(),
        tail: row.tail,
    }
}

/// Occurs check: does `v` appear (transitively, through `subst`) inside
/// `ty`? Descends into container types *and* row fields (`Record`/`Rel`) —
/// the frozen ruling ("occurs-check depth: row-descending wins") — so a
/// cycle hidden inside a record field is caught rather than silently bound.
pub fn occurs(v: TyVar, ty: &Ty, subst: &BTreeMap<TyVar, Ty>) -> bool {
    match ty {
        Ty::Var(x) => *x == v || subst.get(x).is_some_and(|bound| occurs(v, bound, subst)),
        Ty::Option(x)
        | Ty::List(x)
        | Ty::Vector(x)
        | Ty::Set(x)
        | Ty::Bag(x)
        | Ty::Estimate(x)
        | Ty::Missing(x) => occurs(v, x, subst),
        Ty::Result(a, b) | Ty::Map(a, b) => occurs(v, a, subst) || occurs(v, b, subst),
        Ty::Record(row) | Ty::Rel(row) => row.fields.iter().any(|f| occurs(v, &f.ty, subst)),
        _ => false,
    }
}

/// One decision produced by structurally comparing two already-`resolve`d
/// types. Each checker turns a `Step` into its own observation (push a
/// `TypeError`/mutate `Expr.ty`, or record a `Fact`/`TypeConflict`) — this
/// enum, and [`step`] that produces it, *is* the "one algorithm" both
/// checkers share for the top-level unification decision.
pub enum Step {
    /// The two sides are already known-equal; nothing to do.
    Done,
    /// Bind an unresolved variable to a type. The caller still runs its own
    /// occurs-check via [`occurs`] before committing the binding — binding
    /// itself is not pure (it mutates the substitution), so it stays with
    /// the caller.
    Bind(TyVar, Ty),
    /// Recurse structurally: every pair must be unified in turn.
    Descend(Vec<(Ty, Ty)>),
    /// Row-match two `Record`/`Rel` rows; see [`match_rows`].
    Rows(Row, Row),
    /// The two sides are incompatible.
    Mismatch(Ty, Ty),
    /// #15 PR5 (§19.1 "The epistemic type system" / conformance I.22.2): one
    /// side is an epistemic-status-bearing type (`Estimate<T>`,
    /// `Missing<T>`, or `Probability`) and the other is its "plain" payload
    /// type (or, for `Probability`, `Bool`) — an implicit conversion the
    /// spec's erasure table forbids outright, distinct from an ordinary
    /// [`Step::Mismatch`] between two unrelated types. `from` is always the
    /// epistemic side, `to` the plain side, regardless of which operand
    /// order the caller unified in — see [`epistemic_erasure`].
    Erasure(Ty, Ty),
}

/// Classify one unification step between two already-`resolve`d types.
/// Both checkers call this and nothing else to decide "what should happen
/// here" — only *how to record the answer* differs.
pub fn step(a: Ty, b: Ty) -> Step {
    if a == b {
        return Step::Done;
    }
    match (a, b) {
        // An already-erred type absorbs silently against anything — including
        // a type variable. This arm MUST precede the variable-binding arm
        // below: `Ty::Error` is a non-bindable error marker, so
        // `step(Var(v), Error)` (or the symmetric `step(Error, Var(v))`) must
        // absorb to `Done`, never `Bind(v, Error)`. Binding the marker into
        // the substitution would leak it into every later `resolve` of `v`,
        // re-introducing exactly the cross-contamination the poison kill set
        // out to remove.
        //
        // The failure that produced `Ty::Error` was already reported at its
        // origin site (unknown field, bad arity, dimension conflict, ...);
        // letting it flow into a later unify and report a *second*,
        // derivative "expected X found <error>" would be pure cascade, not
        // a new finding. This is what actually keeps error-recovery
        // sentinels from generating noise now that they can no longer hide
        // behind a shared, bindable poison variable (#15 PR2) — binding the
        // old `TyVar(u32::MAX)` sentinel into whatever a downstream check
        // expected silenced this exact cascade by accident, at the cost of
        // corrupting every other unrelated error-recovery site that reused
        // the same sentinel. `Error` gets the cascade-suppression on
        // purpose, isolated per call site, without the contamination.
        (Ty::Error, _) | (_, Ty::Error) => Step::Done,
        (Ty::Var(v), t) | (t, Ty::Var(v)) => Step::Bind(v, t),
        // `Probability` is the constrained [0,1] `F64` domain. Full range
        // validation is a numeric/strict-IEEE follow-up; v1 admits the
        // representation-level bridge the flagship's clamp relies on. This
        // is a tracked v1 erratum (#15 PR5 fences it behind an explicit
        // checked op) — kept here, in the one place both checkers unify
        // from, so they cannot disagree about it.
        (Ty::Probability, Ty::F64) | (Ty::F64, Ty::Probability) => Step::Done,
        (Ty::Record(a), Ty::Record(b)) | (Ty::Rel(a), Ty::Rel(b)) => Step::Rows(*a, *b),
        // Option/Result descent (ruling: reflect.rs's descending behavior
        // wins) — `Option<?t> ~ Option<Int>` solves `?t := Int` instead of
        // reporting a top-level mismatch between two `Option`s.
        (Ty::Option(a), Ty::Option(b)) => Step::Descend(vec![(*a, *b)]),
        (Ty::Result(a_ok, a_err), Ty::Result(b_ok, b_err)) => {
            Step::Descend(vec![(*a_ok, *b_ok), (*a_err, *b_err)])
        }
        // `Missing<T> ~ Missing<U>` descends into `T ~ U`, same shape as
        // `Option`/`Result` above — two `Missing`s over compatible payloads
        // unify structurally; they only become a forbidden erasure when one
        // side is `Missing<T>` and the other is a *plain*, non-`Missing`
        // type (handled by the `epistemic_erasure` fallback below).
        (Ty::Missing(a), Ty::Missing(b)) => Step::Descend(vec![(*a, *b)]),
        (expected, found) => match epistemic_erasure(&expected, &found) {
            Some((from, to)) => Step::Erasure(from, to),
            None => Step::Mismatch(expected, found),
        },
    }
}

/// Trial-unify `a` against `b` into a *copy* of `subst`. Returns the extended
/// substitution on success, or `None` on mismatch / occurs / erasure. Used by
/// overload selection so failed candidates do not contaminate the live subst.
pub fn try_unify(subst: &BTreeMap<TyVar, Ty>, a: Ty, b: Ty) -> Option<BTreeMap<TyVar, Ty>> {
    let mut subst = subst.clone();
    if try_unify_mut(&mut subst, a, b) {
        Some(subst)
    } else {
        None
    }
}

/// Trial-unify each `actual[i]` against `params[i]`. Arity must already match.
pub fn try_unify_args(
    subst: &BTreeMap<TyVar, Ty>,
    actual: &[Ty],
    params: &[Ty],
) -> Option<BTreeMap<TyVar, Ty>> {
    if actual.len() != params.len() {
        return None;
    }
    let mut subst = subst.clone();
    for (a, p) in actual.iter().cloned().zip(params.iter().cloned()) {
        if !try_unify_mut(&mut subst, a, p) {
            return None;
        }
    }
    Some(subst)
}

fn try_unify_mut(subst: &mut BTreeMap<TyVar, Ty>, a: Ty, b: Ty) -> bool {
    let a = resolve(subst, a);
    let b = resolve(subst, b);
    match step(a, b) {
        Step::Done => true,
        Step::Bind(v, t) => {
            if occurs(v, &t, subst) {
                false
            } else {
                subst.insert(v, t);
                true
            }
        }
        Step::Descend(pairs) => pairs.into_iter().all(|(x, y)| try_unify_mut(subst, x, y)),
        Step::Rows(a, b) => {
            let matched = match_rows(&a, &b);
            if !matched.missing_in_left.is_empty() || !matched.missing_in_right.is_empty() {
                return false;
            }
            matched
                .pairs
                .into_iter()
                .all(|(x, y)| try_unify_mut(subst, x, y))
        }
        Step::Mismatch(_, _) | Step::Erasure(_, _) => false,
    }
}

/// Classify whether unifying `a` against `b` — in *either* operand order —
/// attempts one of the §19.1 forbidden epistemic-status erasures (extended
/// by #15 PR5 / conformance I.22.2 to cover `Missing<T>` the same way):
/// implicitly converting `Estimate<T>`/`Missing<T>` to a plain, non-wrapped
/// type, or `Probability` to `Bool`. Returns `(from, to)` with `from`
/// always the epistemic side and `to` the plain side, so callers don't have
/// to know which of `a`/`b` was "expected" vs "found" for the diagnostic to
/// come out named correctly in both directions — this is what lets both
/// [`crate::infer`] and [`crate::reflect`] report the identical erasure
/// regardless of which order their own call sites happen to pass operands
/// in (unify order is not itself part of the frozen parity contract).
///
/// Deliberately narrow: two *different* epistemic wrappers (e.g.
/// `Estimate<T>` against `Missing<T>`, or `Probability` against
/// `Estimate<Bool>`) are an ordinary [`Step::Mismatch`], not an erasure —
/// neither side is "plain," so nothing is being laundered into an
/// unprotected type. `Var`/`Error` are also never "plain" here: both have
/// their own absorption/binding semantics (handled by earlier arms in
/// [`step`]), so they must never be misreported as an erasure target.
fn epistemic_erasure(a: &Ty, b: &Ty) -> Option<(Ty, Ty)> {
    fn is_plain(t: &Ty) -> bool {
        !matches!(
            t,
            Ty::Var(_) | Ty::Error | Ty::Estimate(_) | Ty::Missing(_) | Ty::Probability
        )
    }
    match (a, b) {
        (Ty::Estimate(_), other) | (Ty::Missing(_), other) if is_plain(other) => {
            Some((a.clone(), b.clone()))
        }
        (other, Ty::Estimate(_)) | (other, Ty::Missing(_)) if is_plain(other) => {
            Some((b.clone(), a.clone()))
        }
        (Ty::Probability, Ty::Bool) => Some((a.clone(), b.clone())),
        (Ty::Bool, Ty::Probability) => Some((b.clone(), a.clone())),
        _ => None,
    }
}

/// Outcome of structurally matching two rows' fields, checked
/// **symmetrically** (ruling: "row symmetry: reflect.rs's symmetric check
/// wins" — `{a} ~ closed {a,b}` is a mismatch regardless of which side is
/// treated as `left`/`right`; a left-only check would miss the case where
/// `right` is closed and has an extra field `left` lacks).
#[derive(Default)]
pub struct RowMatch {
    /// Field types present (by name) on both sides, to be unified pairwise.
    pub pairs: Vec<(Ty, Ty)>,
    /// Fields present on `left` with no counterpart in a `Closed` `right`.
    pub missing_in_right: Vec<Ident>,
    /// Fields present on `right` with no counterpart in a `Closed` `left`.
    pub missing_in_left: Vec<Ident>,
}

pub fn match_rows(left: &Row, right: &Row) -> RowMatch {
    let mut out = RowMatch::default();
    for field in &left.fields {
        match right.fields.iter().find(|x| x.name == field.name) {
            Some(other) => out.pairs.push((field.ty.clone(), other.ty.clone())),
            None if matches!(right.tail, RowTail::Closed) => {
                out.missing_in_right.push(field.name.clone())
            }
            None => {}
        }
    }
    for field in &right.fields {
        if left.fields.iter().all(|x| x.name != field.name) && matches!(left.tail, RowTail::Closed)
        {
            out.missing_in_left.push(field.name.clone());
        }
    }
    out
}

/// Ground physical dimensions of a type, if it names one.
pub fn dims(ty: &Ty) -> Option<Dimensions> {
    match ty {
        Ty::Quantity(m) => Some(quantity_dimensions(m)),
        Ty::Money(c) => Some(money_dimensions(c)),
        Ty::Dimensioned(d) => Some(d.clone()),
        _ => None,
    }
}

/// The inverse of [`dims`]: fold an exponent vector back to the most
/// specific `Ty` (a bare `Quantity`/`Money` when it is a single exponent-1
/// dimension, else a compound `Dimensioned`).
pub fn from_dims(d: Dimensions) -> Ty {
    if d.len() == 1 && d[0].exponent == 1 {
        if let Some(c) = d[0].name.as_str().strip_prefix("money:") {
            return Ty::Money(Ident::new(c));
        }
        return Ty::Quantity(d[0].name.clone());
    }
    Ty::Dimensioned(d)
}

pub fn has_money(dims: &Dimensions) -> bool {
    dims.iter().any(|d| d.name.as_str().starts_with("money:"))
}

pub fn has_distinct_currencies(left: &Dimensions, right: &Dimensions) -> bool {
    let currency = |dims: &Dimensions| {
        dims.iter()
            .find_map(|d| d.name.as_str().strip_prefix("money:").map(str::to_owned))
    };
    matches!((currency(left), currency(right)), (Some(a), Some(b)) if a != b)
}

/// Outcome of comparing two operand types for a same-dimension operator
/// (`add`/`sub`/`eq`/`ne`/`lt`/`le`/`gt`/`ge`). Ruling ("dimension-vs-
/// variable: infer.rs's solving wins"): when at least one side lacks ground
/// dimensions, the caller must *unify* the two sides rather than report a
/// dimension conflict — only two ground, unequal dimension sets are an
/// actual dimension error.
pub enum DimStep {
    /// Both sides have equal ground dimensions: the resulting type.
    Ok(Ty),
    /// Both sides have ground dimensions and they disagree.
    Conflict,
    /// At least one side is not a ground-dimensioned type: solve/unify the
    /// pair instead (e.g. binds a type variable to the other side).
    Solve(Ty, Ty),
}

pub fn same_dimension_step(a: &Ty, b: &Ty) -> DimStep {
    // Temporal arithmetic (issue #47): `Instant`/`Duration` are not
    // ground-`Dimensions` types, but `add`/`sub` over them has fixed algebra —
    // `Instant - Instant = Duration`, `Instant ± Duration = Instant`,
    // `Duration ± Duration = Duration`. Comparisons (`le`/`eq`/...) discard this
    // result Ty and only need the pair to be non-conflicting, so the same arms
    // serve both.
    match (a, b) {
        (Ty::Instant, Ty::Instant) => return DimStep::Ok(Ty::Duration),
        (Ty::Duration, Ty::Duration) => return DimStep::Ok(Ty::Duration),
        (Ty::Instant, Ty::Duration) | (Ty::Duration, Ty::Instant) => {
            return DimStep::Ok(Ty::Instant)
        }
        _ => {}
    }
    match (dims(a), dims(b)) {
        (Some(x), Some(y)) if x == y => DimStep::Ok(a.clone()),
        (Some(_), Some(_)) => DimStep::Conflict,
        _ => DimStep::Solve(a.clone(), b.clone()),
    }
}

/// Outcome of `mul`/`div` dimension combination (currency-mixing and
/// money-times-money are conflicts; anything without two ground dimension
/// sets solves/unifies instead, same "dimension-vs-variable" ruling as
/// [`same_dimension_step`]).
pub enum DimBinaryStep {
    Ok(Ty),
    Conflict,
    Solve(Ty, Ty),
}

pub fn dimension_binary_step(a: &Ty, b: &Ty, mul: bool) -> DimBinaryStep {
    // Temporal ratio (issue #47): `Duration / Duration` is a dimensionless
    // scalar — typed `F64` so it composes with float literals (`1.0 - r/24h`).
    if !mul {
        if let (Ty::Duration, Ty::Duration) = (a, b) {
            return DimBinaryStep::Ok(Ty::F64);
        }
    }
    match (dims(a), dims(b)) {
        (Some(x), Some(y)) => {
            if has_distinct_currencies(&x, &y) || (mul && has_money(&x) && has_money(&y)) {
                DimBinaryStep::Conflict
            } else {
                DimBinaryStep::Ok(from_dims(if mul {
                    dimensions_mul(&x, &y)
                } else {
                    dimensions_div(&x, &y)
                }))
            }
        }
        _ => DimBinaryStep::Solve(a.clone(), b.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IntWidth;

    #[test]
    fn probability_f64_bridge_is_a_no_op_step_in_both_directions() {
        assert!(matches!(step(Ty::Probability, Ty::F64), Step::Done));
        assert!(matches!(step(Ty::F64, Ty::Probability), Step::Done));
    }

    #[test]
    fn option_and_result_descend_instead_of_mismatching() {
        match step(
            Ty::option(Ty::Var(TyVar(1))),
            Ty::option(Ty::Int(IntWidth::Int)),
        ) {
            Step::Descend(pairs) => assert_eq!(pairs.len(), 1),
            _ => panic!("expected descent"),
        }
        match step(
            Ty::Result(Box::new(Ty::Var(TyVar(1))), Box::new(Ty::Var(TyVar(2)))),
            Ty::Result(Box::new(Ty::Bool), Box::new(Ty::Str)),
        ) {
            Step::Descend(pairs) => assert_eq!(pairs.len(), 2),
            _ => panic!("expected descent"),
        }
    }

    #[test]
    fn row_match_is_symmetric() {
        let a = Row::closed(vec![RowField {
            name: Ident::new("a"),
            ty: Ty::Bool,
        }]);
        let b = Row::closed(vec![
            RowField {
                name: Ident::new("a"),
                ty: Ty::Bool,
            },
            RowField {
                name: Ident::new("b"),
                ty: Ty::Bool,
            },
        ]);
        let forward = match_rows(&a, &b);
        assert!(forward.missing_in_right.is_empty());
        assert_eq!(forward.missing_in_left, vec![Ident::new("b")]);

        let backward = match_rows(&b, &a);
        assert_eq!(backward.missing_in_right, vec![Ident::new("b")]);
        assert!(backward.missing_in_left.is_empty());
    }

    #[test]
    fn occurs_descends_into_record_rows() {
        let row = Row::closed(vec![RowField {
            name: Ident::new("x"),
            ty: Ty::Var(TyVar(7)),
        }]);
        assert!(occurs(TyVar(7), &Ty::record(row), &BTreeMap::new()));
    }

    #[test]
    fn error_absorbs_silently_instead_of_cascading_a_second_mismatch() {
        assert!(matches!(
            step(Ty::Error, Ty::Money(Ident::new("EUR"))),
            Step::Done
        ));
        assert!(matches!(step(Ty::Bool, Ty::Error), Step::Done));
        // Error is not itself Var: it must not be equal to a concrete type
        // either, only absorbed by this explicit rule.
        assert!(Ty::Error != Ty::Bool);
    }

    #[test]
    fn error_absorbs_against_a_variable_and_is_never_bound() {
        // Regression: the `Ty::Error` absorption arm must precede the
        // variable-binding arm, in BOTH operand orders. If it does not,
        // `step` returns `Bind(v, Error)` and the non-bindable error marker
        // leaks into the substitution — corrupting every later `resolve` of
        // `v` and re-opening the poison-contamination hole this PR closes.
        let v = TyVar(3);
        assert!(
            matches!(step(Ty::Var(v), Ty::Error), Step::Done),
            "step(Var, Error) must absorb to Done, never Bind"
        );
        assert!(
            matches!(step(Ty::Error, Ty::Var(v)), Step::Done),
            "step(Error, Var) must absorb to Done, never Bind"
        );
    }

    #[test]
    fn estimate_to_plain_is_an_erasure_not_a_generic_mismatch() {
        match step(
            Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            Ty::Int(IntWidth::Int),
        ) {
            Step::Erasure(from, to) => {
                assert_eq!(from, Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))));
                assert_eq!(to, Ty::Int(IntWidth::Int));
            }
            _ => panic!("expected Erasure, got a different Step"),
        }
        // Symmetric: the epistemic side may be either operand.
        match step(
            Ty::Int(IntWidth::Int),
            Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
        ) {
            Step::Erasure(from, to) => {
                assert_eq!(from, Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))));
                assert_eq!(to, Ty::Int(IntWidth::Int));
            }
            _ => panic!("expected Erasure in the reversed operand order too"),
        }
    }

    #[test]
    fn probability_to_bool_is_an_erasure_distinct_from_the_probability_f64_bridge() {
        match step(Ty::Probability, Ty::Bool) {
            Step::Erasure(from, to) => {
                assert_eq!(from, Ty::Probability);
                assert_eq!(to, Ty::Bool);
            }
            _ => panic!("expected Erasure for Probability ~ Bool"),
        }
        // The F64 bridge is untouched by this — a different pairing.
        assert!(matches!(step(Ty::Probability, Ty::F64), Step::Done));
    }

    #[test]
    fn missing_to_plain_is_an_erasure_no_implicit_coercion() {
        match step(Ty::missing(Ty::Bool), Ty::Bool) {
            Step::Erasure(from, to) => {
                assert_eq!(from, Ty::missing(Ty::Bool));
                assert_eq!(to, Ty::Bool);
            }
            _ => panic!("expected Erasure for Missing<Bool> ~ Bool"),
        }
    }

    #[test]
    fn cross_epistemic_wrappers_are_a_plain_mismatch_not_an_erasure() {
        // Neither side is "plain," so this is not laundering anything —
        // it must stay an ordinary Mismatch.
        match step(
            Ty::Estimate(Box::new(Ty::Int(IntWidth::Int))),
            Ty::missing(Ty::Int(IntWidth::Int)),
        ) {
            Step::Mismatch(_, _) => {}
            _ => panic!(
                "expected a plain Mismatch for two different epistemic wrappers, got a different Step"
            ),
        }
    }

    #[test]
    fn missing_of_equal_inner_type_unifies_cleanly() {
        assert!(matches!(
            step(Ty::missing(Ty::Bool), Ty::missing(Ty::Bool)),
            Step::Done
        ));
    }

    #[test]
    fn missing_descends_to_solve_its_inner_variable() {
        match step(Ty::missing(Ty::Var(TyVar(70))), Ty::missing(Ty::Bool)) {
            Step::Descend(pairs) => assert_eq!(pairs, vec![(Ty::Var(TyVar(70)), Ty::Bool)]),
            _ => panic!("expected Missing<T> ~ Missing<U> to descend into T ~ U"),
        }
    }

    #[test]
    fn dimension_vs_variable_solves_instead_of_conflicting() {
        let km = Ty::Quantity(Ident::new("Kilometre"));
        let var = Ty::Var(TyVar(3));
        match same_dimension_step(&km, &var) {
            DimStep::Solve(_, _) => {}
            _ => panic!("expected the missing-dimension side to solve, not conflict"),
        }
    }
}
