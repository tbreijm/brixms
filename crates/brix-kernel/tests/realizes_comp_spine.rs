//! Right-nested `RealizesComp` spines.
//!
//! `infer_realizes_comp` walks a right-nested spine with a loop instead of
//! recursing on `right`, accounting for steps and depth by hand. The older
//! adversarial tests only nest to the left, which never enters that loop. These
//! tests use distinct witnesses and endpoints so a transposed boundary or a
//! swapped composition order cannot pass by symmetry.
//!
//! Scope: this pins one iterated spine. It does not claim that cloning,
//! hashing, equality, destruction, tensor or left-nested checking are stack
//! safe at arbitrary depth.

use brix_kernel::{
    acceptance, Budget, ExplicitTerm, ObjectTerm, Prop, ResourceBudgetReason, TermKind, Var,
    Verdict,
};
use brix_semantic::{ContextId, PropositionId};

fn ctx() -> ContextId {
    ContextId::from_canon(b"realizes_comp_spine")
}

fn obj(name: &[u8]) -> ObjectTerm {
    ObjectTerm::Const(PropositionId::from_canon(name))
}

fn hyp(name: &str) -> TermKind {
    TermKind::Hyp(Var::Named(name.into()))
}

fn comp(left: TermKind, right: TermKind) -> TermKind {
    TermKind::RealizesComp {
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn compose(outer: ObjectTerm, inner: ObjectTerm) -> ObjectTerm {
    ObjectTerm::Compose(Box::new(outer), Box::new(inner))
}

fn lams(names: &[&str], body: TermKind) -> TermKind {
    names.iter().rev().fold(body, |acc, n| TermKind::Lam {
        var_name: Some((*n).into()),
        body: Box::new(acc),
    })
}

fn impls(premises: Vec<Prop>, goal: Prop) -> Prop {
    premises
        .into_iter()
        .rev()
        .fold(goal, |acc, p| Prop::Impl(Box::new(p), Box::new(acc)))
}

/// Premises `g1: a→b`, `g2: b→c`, `g3: c→d`, `g4: d→e`.
struct Chain {
    g: [ObjectTerm; 4],
    p: [ObjectTerm; 5],
}

fn chain() -> Chain {
    Chain {
        g: [obj(b"g1"), obj(b"g2"), obj(b"g3"), obj(b"g4")],
        p: [obj(b"a"), obj(b"b"), obj(b"c"), obj(b"d"), obj(b"e")],
    }
}

impl Chain {
    fn premise(&self, i: usize) -> Prop {
        Prop::Realizes(self.g[i].clone(), self.p[i].clone(), self.p[i + 1].clone())
    }

    /// `compose(compose(g3, g2), g1)`: the witness the right-nested term
    /// `comp(h1, comp(h2, h3))` establishes (`comp(h2, h3)` is
    /// `compose(g3, g2)`, then `h1` is applied innermost-last).
    fn witness3(&self) -> ObjectTerm {
        compose(
            compose(self.g[2].clone(), self.g[1].clone()),
            self.g[0].clone(),
        )
    }

    /// `compose(g3, compose(g2, g1))`: what the left-nested term establishes.
    fn left_witness3(&self) -> ObjectTerm {
        compose(
            self.g[2].clone(),
            compose(self.g[1].clone(), self.g[0].clone()),
        )
    }

    fn goal3(&self) -> Prop {
        Prop::Realizes(self.witness3(), self.p[0].clone(), self.p[3].clone())
    }

    fn statement3(&self, goal: Prop) -> Prop {
        impls(
            vec![self.premise(0), self.premise(1), self.premise(2)],
            goal,
        )
    }

    /// `\h1 h2 h3. comp(h1, comp(h2, h3))` — right-nested.
    fn term3(&self) -> ExplicitTerm {
        ExplicitTerm::new(
            ctx(),
            lams(
                &["h1", "h2", "h3"],
                comp(hyp("h1"), comp(hyp("h2"), hyp("h3"))),
            ),
        )
    }
}

fn run(prop: &Prop, term: &ExplicitTerm, steps: usize, depth: usize) -> Verdict {
    acceptance(&ctx(), prop, term, Budget::new(steps, depth))
}

#[test]
fn a_right_nested_three_node_composition_accepts_exactly_the_expected_witness() {
    let c = chain();
    let prop = c.statement3(c.goal3());
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Accepted(_)
    ));
}

#[test]
fn the_left_and_right_nestings_establish_distinct_witnesses_and_do_not_cross() {
    // The witness is structural, so the two nestings of the same premises
    // establish different terms; neither proves the other's claim.
    let c = chain();
    let right_claim = c.statement3(c.goal3());
    let left_claim = c.statement3(Prop::Realizes(
        c.left_witness3(),
        c.p[0].clone(),
        c.p[3].clone(),
    ));
    let left_nested = ExplicitTerm::new(
        ctx(),
        lams(
            &["h1", "h2", "h3"],
            comp(comp(hyp("h1"), hyp("h2")), hyp("h3")),
        ),
    );
    assert!(matches!(
        run(&left_claim, &left_nested, 1_000, 256),
        Verdict::Accepted(_)
    ));
    assert!(matches!(
        run(&right_claim, &c.term3(), 1_000, 256),
        Verdict::Accepted(_)
    ));
    assert!(matches!(
        run(&right_claim, &left_nested, 1_000, 256),
        Verdict::Rejected(_)
    ));
    assert!(matches!(
        run(&left_claim, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));
}

#[test]
fn a_swapped_composition_order_in_the_claimed_witness_rejects() {
    let c = chain();
    let wrong = compose(compose(c.g[0].clone(), c.g[1].clone()), c.g[2].clone());
    let prop = c.statement3(Prop::Realizes(wrong, c.p[0].clone(), c.p[3].clone()));
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));
}

#[test]
fn a_wrong_final_witness_rejects() {
    let c = chain();
    let wrong = compose(c.g[3].clone(), compose(c.g[1].clone(), c.g[0].clone()));
    let prop = c.statement3(Prop::Realizes(wrong, c.p[0].clone(), c.p[3].clone()));
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));
}

#[test]
fn a_wrong_claimed_endpoint_rejects() {
    let c = chain();
    let prop = c.statement3(Prop::Realizes(c.witness3(), c.p[0].clone(), c.p[4].clone()));
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));
}

#[test]
fn an_interior_boundary_mismatch_rejects_at_each_position() {
    let c = chain();
    // Break the b/c boundary (between h1·h2) and then the c/d boundary
    // (between h2·h3); each must be caught inside the iterated spine.
    let broken_first = Prop::Realizes(c.g[1].clone(), obj(b"b-prime"), c.p[2].clone());
    let prop = impls(vec![c.premise(0), broken_first, c.premise(2)], c.goal3());
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));

    let broken_second = Prop::Realizes(c.g[2].clone(), obj(b"c-prime"), c.p[3].clone());
    let prop = impls(vec![c.premise(0), c.premise(1), broken_second], c.goal3());
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));
}

#[test]
fn a_non_realizes_node_inside_the_spine_rejects() {
    let c = chain();
    let atom = Prop::Atom(PropositionId::from_canon(b"not-a-realizes"));
    let prop = impls(vec![c.premise(0), atom, c.premise(2)], c.goal3());
    assert!(matches!(
        run(&prop, &c.term3(), 1_000, 256),
        Verdict::Rejected(_)
    ));
}

/// Smallest `n` in `0..=limit` for which `accepts(n)` first holds.
fn minimum(limit: usize, accepts: impl Fn(usize) -> bool) -> usize {
    (0..=limit)
        .find(|n| accepts(*n))
        .expect("the proof must be accepted within the search limit")
}

#[test]
fn the_minimum_step_and_depth_budgets_are_exact_and_one_short_is_exhaustion() {
    let c = chain();
    let prop = c.statement3(c.goal3());
    let term = c.term3();

    let min_steps = minimum(1_000, |s| {
        matches!(run(&prop, &term, s, 256), Verdict::Accepted(_))
    });
    let min_depth = minimum(256, |d| {
        matches!(run(&prop, &term, 1_000, d), Verdict::Accepted(_))
    });
    assert!(min_steps > 0 && min_depth > 0);

    assert!(matches!(
        run(&prop, &term, min_steps - 1, 256),
        Verdict::ResourceExhausted(ResourceBudgetReason::StepLimitExceeded)
    ));
    assert!(matches!(
        run(&prop, &term, 1_000, min_depth - 1),
        Verdict::ResourceExhausted(ResourceBudgetReason::DepthLimitExceeded)
    ));
    // Exhaustion is absence of a verdict, never a logical refutation.
    assert_eq!(
        run(&prop, &term, min_steps - 1, 256).outcome(),
        Some(brix_semantic::Outcome::Unknown)
    );

    // Pin the measured values for this fixed proof so an accounting change in
    // the spine loop is a visible, deliberate edit.
    assert_eq!((min_steps, min_depth), PINNED);
}

/// `(steps, depth)` measured for the three-node right-nested proof.
const PINNED: (usize, usize) = (8, 6);

#[test]
fn every_one_short_budget_across_spine_lengths_exhausts_and_never_refutes() {
    // Longer right spines than the pinned fixture: errors at every descent
    // boundary must return a bounded refusal, not panic.
    for n in 2usize..=4 {
        let c = chain();
        let names: Vec<String> = (1..=n).map(|i| format!("h{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let premises: Vec<Prop> = (0..n).map(|i| c.premise(i)).collect();
        let witness = (0..n - 1)
            .rev()
            .fold(c.g[n - 1].clone(), |acc, i| compose(acc, c.g[i].clone()));
        let goal = Prop::Realizes(witness, c.p[0].clone(), c.p[n].clone());
        let prop = impls(premises, goal);
        // Right-nested: comp(h1, comp(h2, ... hn)).
        let mut it = (1..=n).rev().map(|i| hyp(&format!("h{i}")));
        let last = it.next().unwrap();
        let body = it.fold(last, |acc, h| comp(h, acc));
        let term = ExplicitTerm::new(ctx(), lams(&refs, body));

        let min_steps = minimum(2_000, |s| {
            matches!(run(&prop, &term, s, 256), Verdict::Accepted(_))
        });
        let min_depth = minimum(256, |d| {
            matches!(run(&prop, &term, 2_000, d), Verdict::Accepted(_))
        });
        for s in 0..min_steps {
            assert!(
                matches!(run(&prop, &term, s, 256), Verdict::ResourceExhausted(_)),
                "n={n} steps={s}"
            );
        }
        for d in 0..min_depth {
            assert!(
                matches!(run(&prop, &term, 2_000, d), Verdict::ResourceExhausted(_)),
                "n={n} depth={d}"
            );
        }
    }
}

#[test]
fn depth_is_restored_after_a_nested_composition_so_a_sibling_branch_still_fits() {
    // `\h1 h2 h3. (comp(h1, comp(h2, h3)), then a sibling comp(h1, h2))`
    // is not a single typed term, so exercise restoration through a
    // `Pair`-free route: a left-nested composition whose *left* operand is the
    // right-nested spine. After the spine returns, the outer node still needs
    // depth for its right sibling; if the spine leaked its depth units the
    // minimum depth here would exceed that of the spine alone plus one node.
    let c = chain();
    let premises = vec![c.premise(0), c.premise(1), c.premise(2), c.premise(3)];
    let witness = compose(c.g[3].clone(), c.witness3());
    let prop = impls(
        premises,
        Prop::Realizes(witness, c.p[0].clone(), c.p[4].clone()),
    );
    let term = ExplicitTerm::new(
        ctx(),
        lams(
            &["h1", "h2", "h3", "h4"],
            comp(comp(hyp("h1"), comp(hyp("h2"), hyp("h3"))), hyp("h4")),
        ),
    );
    assert!(matches!(
        run(&prop, &term, 2_000, 256),
        Verdict::Accepted(_)
    ));

    // The spine alone (three premises) and the composed whole differ by a
    // bounded amount of depth; a leak would scale with the spine length.
    let spine_only = c.statement3(c.goal3());
    let min_whole = minimum(256, |d| {
        matches!(run(&prop, &term, 2_000, d), Verdict::Accepted(_))
    });
    let min_spine = minimum(256, |d| {
        matches!(run(&spine_only, &c.term3(), 2_000, d), Verdict::Accepted(_))
    });
    assert!(
        min_whole <= min_spine + 4,
        "depth leaked across the sibling: whole={min_whole}, spine={min_spine}"
    );
}
