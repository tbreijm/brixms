//! `ParseLimits` nesting-depth enforcement (ADR-0022 D6).
//!
//! The contract these pin is the one `LimitExceeded::NestingDepth` states:
//! a deep input is "refused before descending, so the stack is never at
//! risk". Each test below overflows the stack if its descent stops being
//! charged — the assertion is not that an error is *nicer*, it is that the
//! process survives to observe one.

use brix_syntax::{parse_bounded, LimitExceeded, ParseLimits};

fn assert_refused_for_depth(src: &str, what: &str) {
    // `ParseError` carries its refusal in the message rather than as a
    // variant, so the expected text is built from the same `LimitExceeded`
    // the parser would render.
    let expected = LimitExceeded::NestingDepth {
        limit: ParseLimits::strict().max_nesting_depth,
    }
    .to_string();
    match parse_bounded(src, ParseLimits::strict()) {
        Err(e) => assert!(
            e.to_string().contains(&expected),
            "{what}: expected a nesting-depth refusal ({expected}), got: {e}"
        ),
        Ok(_) => panic!("{what}: expected a nesting-depth refusal, parsed successfully"),
    }
}

/// `!` is one byte per level, so a hostile module reaches ~1e5 levels well
/// inside every other bound. It recurses through `parse_expr_prefix`, not
/// `parse_expr`, which is exactly the path that was descending uncharged.
#[test]
fn deep_not_prefix_is_refused_before_descending() {
    let src = format!("let x = {}true", "!".repeat(100_000));
    assert_refused_for_depth(&src, "! prefix");
}

/// Unary minus desugars to `0 - e` for anything but a bare numeral, and that
/// desugaring recurses through `parse_prefix_operand` exactly like `!` does.
/// A run of minuses ending in a non-numeral operand (`true`, not a digit)
/// never hits the O(1) literal-folding shortcut, so every `-` here is one
/// charged level of `max_nesting_depth`.
#[test]
fn deep_unary_minus_prefix_is_refused_before_descending() {
    let src = format!("let x = {}true", "-".repeat(100_000));
    assert_refused_for_depth(&src, "unary minus prefix");
}

#[test]
fn deep_prove_prefix_is_refused_before_descending() {
    let src = format!("let x = {}true", "prove ".repeat(100_000));
    assert_refused_for_depth(&src, "prove prefix");
}

#[test]
fn deep_audit_prefix_is_refused_before_descending() {
    let src = format!("let x = {}true", "audit ".repeat(100_000));
    assert_refused_for_depth(&src, "audit prefix");
}

/// Parenthesised nesting goes back through `parse_expr`, so it was already
/// charged; asserted here so the two descent paths are covered together.
#[test]
fn deep_paren_nesting_is_refused_before_descending() {
    // Each level costs two tokens here, so this stays well inside
    // `max_tokens` and the depth bound is the one under test.
    let n = 5_000;
    let src = format!("let x = {}true{}", "(".repeat(n), ")".repeat(n));
    assert_refused_for_depth(&src, "paren nesting");
}

/// Depth is a *ceiling*, not a budget spent across the module: a long chain
/// of shallow prefix expressions must still parse. This is what catches a
/// fix that charges on entry but forgets to release on the way out.
#[test]
fn depth_is_released_so_many_shallow_prefixes_still_parse() {
    let body: String = (0..500).map(|i| format!("let x{i} = !!true\n")).collect();
    parse_bounded(&body, ParseLimits::strict()).expect("shallow prefixes must parse");
}

/// A prefix chain just under the bound is accepted, so the limit is the
/// thing being enforced rather than an unconditional refusal.
#[test]
fn prefix_nesting_just_under_the_bound_is_accepted() {
    let limit = ParseLimits::strict().max_nesting_depth;
    // one level is charged by `parse_expr` itself before any prefix operand
    let src = format!("let x = {}true", "!".repeat(limit - 1));
    parse_bounded(&src, ParseLimits::strict()).expect("depth below the bound must parse");
}

fn nested_match(depth: usize) -> String {
    format!("{}1{}", "match x { A => ".repeat(depth), " }".repeat(depth))
}

#[test]
fn match_nesting_respects_default_and_custom_boundaries() {
    for limit in [8, ParseLimits::strict().max_nesting_depth] {
        let limits = ParseLimits {
            max_nesting_depth: limit,
            ..ParseLimits::strict()
        };
        // The outer expression uses one level; each match scrutinee and arm
        // body uses another. Splitting parser frames must not change this.
        let accepted = format!("show {}", nested_match(limit - 1));
        parse_bounded(&accepted, limits).expect("match nesting below the bound must parse");
        let refused = format!("show {}", nested_match(limit));
        let error = parse_bounded(&refused, limits).unwrap_err();
        assert!(error
            .to_string()
            .contains(&LimitExceeded::NestingDepth { limit }.to_string()));
    }
}

#[test]
fn depth_is_released_between_sibling_matches() {
    let expression = nested_match(4);
    let source = (0..500)
        .map(|i| format!("let x{i} = {expression}\n"))
        .collect::<String>();
    parse_bounded(&source, ParseLimits::strict()).expect("shallow sibling matches must parse");
}

#[test]
fn nested_matches_with_binary_operands_are_refused_before_stack_overflow() {
    // Precedence climbing retains additional frames when each match occurs
    // on a right-hand side. The same default stack must still reach the guard.
    let source = format!(
        "show {}1{}",
        "true || true && 1 < 2 + 3 * match x { A => ".repeat(200),
        " }".repeat(200)
    );
    assert_refused_for_depth(&source, "match in binary operands");
}
