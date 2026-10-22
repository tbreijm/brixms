//! An ordinary long expression must not crash the compiler.
//!
//! **This is a regression test for a reachable abort, not a margin.** Before
//! the kernel's (→I) spine was made iterative, `brix check` on
//! `let x = 1 + 1 + ... ` with **50 terms** died with
//! `thread 'main' has overflowed its stack` — a process abort, no diagnostic,
//! exit by signal. Fifty terms is not a pathological input.
//!
//! The reachability is the point. `Limits::max_nesting_depth` caps *parser
//! recursion* at 128, and a binary-operator chain is parsed by a loop, so its
//! depth is not bounded by that limit at all — the expression tree is as deep
//! as the chain is long. Nothing between the lexer and the kernel bounded it.
//!
//! What actually overflowed was `brix_kernel::acceptance`: `brix-elaborate`
//! discharges a derivation's leaves as hypotheses and emits `λh₁…λhₘ. body`
//! against `H₁ → … → Hₘ → G`, so the lambda spine is as long as the derivation
//! has leaves, and the checker recursed once per binder.
//!
//! These run on ordinary test threads. A large-stack helper here would be
//! testing that a big stack is bigger than a small one — and it was precisely
//! the 8 MiB main thread that made this look survivable while the 2 MiB test
//! threads were already dying.
//!
//! **The residual, stated because this is where someone will look for it.**
//! The right-nested `RealizesComp` spine is now walked by a loop in
//! `infer_realizes_comp` (pinned directly by
//! `brix-kernel/tests/realizes_comp_spine.rs`), which is what lets the
//! 50- and 55-term chains below finish on a 2 MiB test thread. That is one
//! spine made iterative, not a general guarantee: left-nested composition,
//! tensor, and recursive `Clone`/`Hash`/`Eq`/`Drop` over the proof terms are
//! still recursive. `brix check` survives *any* input only because its 8 MiB
//! main thread outlasts the step budget that stops the search; the limits
//! have been measured only for the shapes tested here.
//! See `Type_Realization_Contract.md` §5.5.

use brix_lower::check_module;
use brix_syntax::parse;

fn chain(n: usize) -> String {
    format!("let x = {}", vec!["1"; n].join(" + "))
}

/// The exact shape that aborted, at the length that aborted.
#[test]
fn a_fifty_term_chain_checks() {
    let module = parse(&chain(50)).expect("parses");
    let results = check_module(&module);
    let checked = results.first().expect("one binding");
    // The *grade* is not asserted: `g_arith` is undischarged, so this is
    // capped at `Audited`, and pinning that here would make this a
    // discharge-status change-detector instead of a crash regression.
    checked
        .as_ref()
        .unwrap_or_else(|(n, e)| panic!("{n}: {e:?}"));
}

/// Long enough that the kernel's *step* budget answers first — a verdict.
///
/// This is the distinction worth holding. Exhaustion is a statement about the
/// search (`Type_Realization_Contract.md` §5.4): it comes back as
/// `ResourceExhausted`, the binding is left unproven, and the caller is told
/// why. A stack overflow is no statement at all, because the process is gone.
/// The assertion is only that `check_module` *returns*.
#[test]
fn a_chain_that_exhausts_the_budget_returns_a_verdict() {
    let module = parse(&chain(55)).expect("parses");
    assert_eq!(check_module(&module).len(), 1);
}
