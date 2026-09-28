//! Property-based robustness ("fuzz") tests for the hostile-input surface of
//! the source frontend: `parse_bounded(src, ParseLimits::strict())` must
//! never panic on *any* input, and must always return `Ok` or `Err` — never
//! hang, never overflow the stack, never leave a `ParseError`'s reported
//! position outside the source it was raised against.
//!
//! Scope note: this is the frontier ADR-0022 D6 names as attacker-controlled
//! (see `crates/brix-syntax/src/limits.rs`'s module docs) — an offline
//! verifier's trusted closure includes this parser over source it did not
//! write. `crates/brix-syntax/tests/parse_limits.rs` already pins specific
//! historical depth-charging defects by hand; this file explores the same
//! surface by generation rather than by named example.
//!
//! Case counts are proptest's own default (256) unless overridden by the
//! standard `PROPTEST_CASES` env var, which every `proptest!` block here
//! honors automatically. See `docs/performance.md` "Added test runtime" for
//! measured wall-clock cost at the default count and at `PROPTEST_CASES=5000`.

use proptest::prelude::*;

use brix_syntax::{parse_bounded, ParseLimits};

// ---------------------------------------------------------------------------
// (a) Arbitrary strings.
// ---------------------------------------------------------------------------

proptest! {
    /// Any byte-ish string, valid UTF-8 or not particularly structured, must
    /// parse to `Ok` or `Err` without panicking. `proptest::string::string_regex`
    /// underneath `any::<String>()` already includes control characters,
    /// empty strings, and non-ASCII text.
    #[test]
    fn arbitrary_strings_never_panic(src in any::<String>()) {
        let _ = parse_bounded(&src, ParseLimits::strict());
    }

    /// The same, but capped near `ParseLimits::strict().max_source_bytes` so
    /// generation explores inputs close to the byte-length boundary itself,
    /// not only small ones `any::<String>()` favors.
    #[test]
    fn arbitrary_strings_near_the_byte_limit_never_panic(
        src in proptest::collection::vec(any::<char>(), 0..4096).prop_map(|cs| cs.into_iter().collect::<String>())
    ) {
        let _ = parse_bounded(&src, ParseLimits::strict());
    }
}

// ---------------------------------------------------------------------------
// (b) Token soup drawn from the lexer's own vocabulary.
// ---------------------------------------------------------------------------

const KEYWORDS: &[&str] = &[
    "config",
    "regime",
    "gen",
    "rule",
    "fn",
    "let",
    "show",
    "witness",
    "use",
    "match",
    "prove",
    "why",
    "audit",
    "then",
    "and",
    "propose",
    "priority",
    "when",
    "commit",
    "from",
    "input",
    "true",
    "false",
    "proving",
    "exhaustive",
];

const SYMBOLS: &[&str] = &[
    "{", "}", "(", ")", ":", "=", "|", ",", ".", "@", "=>", "+", "-", "*", "/", "<", "<=", ">",
    ">=", "==", "!=", "&&", "||", "!", "_", "&",
];

const IDENTS: &[&str] = &[
    "x",
    "y",
    "z",
    "Foo",
    "Bar",
    "abc123",
    "_underscore",
    "A",
    "B",
    "C",
    "Decision",
    "Item",
];

const NUMS: &[&str] = &[
    "0",
    "1",
    "42",
    "9223372036854775807",
    "-9223372036854775808",
    "3.14",
    "0.0",
    "007",
];

const STR_LITS: &[&str] = &["\"\"", "\"hello\"", "\"line\\nbreak\"", "\"unterminated"];

fn token_vocab() -> Vec<&'static str> {
    KEYWORDS
        .iter()
        .chain(SYMBOLS.iter())
        .chain(IDENTS.iter())
        .chain(NUMS.iter())
        .chain(STR_LITS.iter())
        .chain(std::iter::once(&"\n"))
        .copied()
        .collect()
}

fn token_soup_strategy() -> impl Strategy<Value = String> {
    let vocab = token_vocab();
    let len = vocab.len();
    proptest::collection::vec(0..len, 0..400).prop_map(move |indices| {
        indices
            .into_iter()
            .map(|i| vocab[i])
            .collect::<Vec<_>>()
            .join(" ")
    })
}

proptest! {
    /// A random sequence of real tokens (keywords, operators, identifiers,
    /// literals, braces, newlines) in arbitrary order is mostly grammatically
    /// invalid, but the parser must still only ever return `Ok`/`Err`.
    #[test]
    fn token_soup_never_panics(src in token_soup_strategy()) {
        let _ = parse_bounded(&src, ParseLimits::strict());
    }
}

// ---------------------------------------------------------------------------
// (c) Random mutations of the shipped example programs.
// ---------------------------------------------------------------------------

const FIXTURES: &[&str] = &[
    include_str!("../../../examples/allocation.brix"),
    include_str!("../../../examples/order-policy.brix"),
    include_str!("../../../examples/shipping-functions.brix"),
    include_str!("../../../examples/shipping-input.brix"),
    include_str!("../../../examples/shipping.brix"),
    include_str!("../../../packages/brix.soc/src/soc.brix"),
];

#[derive(Clone, Debug)]
enum Mutation {
    DeleteLine(f64),
    InsertLine(f64, usize),
    DuplicateLine(f64),
    SwapLines(f64, f64),
    DeleteToken(f64),
    InsertToken(f64, usize),
    DuplicateToken(f64),
    SwapTokens(f64, f64),
}

fn mutation_strategy() -> impl Strategy<Value = Mutation> {
    let unit = 0.0f64..1.0f64;
    prop_oneof![
        unit.clone().prop_map(Mutation::DeleteLine),
        (unit.clone(), 0..token_vocab().len()).prop_map(|(f, v)| Mutation::InsertLine(f, v)),
        unit.clone().prop_map(Mutation::DuplicateLine),
        (unit.clone(), unit.clone()).prop_map(|(a, b)| Mutation::SwapLines(a, b)),
        unit.clone().prop_map(Mutation::DeleteToken),
        (unit.clone(), 0..token_vocab().len()).prop_map(|(f, v)| Mutation::InsertToken(f, v)),
        unit.clone().prop_map(Mutation::DuplicateToken),
        (unit.clone(), unit).prop_map(|(a, b)| Mutation::SwapTokens(a, b)),
    ]
}

/// Map a `0.0..1.0` fraction to a valid index into a non-empty slice of
/// length `len`.
fn frac_index(f: f64, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (((f.clamp(0.0, 1.0)) * len as f64) as usize).min(len - 1)
    }
}

fn apply_mutation(lines: &mut Vec<String>, m: &Mutation) {
    if lines.is_empty() {
        lines.push(String::new());
    }
    let n = lines.len();
    match m {
        Mutation::DeleteLine(f) => {
            let i = frac_index(*f, n);
            lines.remove(i);
        }
        Mutation::InsertLine(f, vocab_i) => {
            let i = frac_index(*f, n + 1).min(lines.len());
            let vocab = token_vocab();
            lines.insert(i, vocab[vocab_i % vocab.len()].to_string());
        }
        Mutation::DuplicateLine(f) => {
            let i = frac_index(*f, n);
            let l = lines[i].clone();
            lines.insert(i, l);
        }
        Mutation::SwapLines(fa, fb) => {
            let i = frac_index(*fa, n);
            let j = frac_index(*fb, n);
            lines.swap(i, j);
        }
        Mutation::DeleteToken(f) => {
            let i = frac_index(*f, n);
            let mut toks: Vec<&str> = lines[i].split_whitespace().collect();
            if !toks.is_empty() {
                let k = frac_index(*f, toks.len());
                toks.remove(k);
            }
            lines[i] = toks.join(" ");
        }
        Mutation::InsertToken(f, vocab_i) => {
            let i = frac_index(*f, n);
            let mut toks: Vec<String> = lines[i].split_whitespace().map(String::from).collect();
            let k = frac_index(*f, toks.len() + 1).min(toks.len());
            let vocab = token_vocab();
            toks.insert(k, vocab[vocab_i % vocab.len()].to_string());
            lines[i] = toks.join(" ");
        }
        Mutation::DuplicateToken(f) => {
            let i = frac_index(*f, n);
            let mut toks: Vec<String> = lines[i].split_whitespace().map(String::from).collect();
            if !toks.is_empty() {
                let k = frac_index(*f, toks.len());
                let t = toks[k].clone();
                toks.insert(k, t);
            }
            lines[i] = toks.join(" ");
        }
        Mutation::SwapTokens(fa, fb) => {
            let i = frac_index(*fa, n);
            let mut toks: Vec<String> = lines[i].split_whitespace().map(String::from).collect();
            if toks.len() >= 2 {
                let a = frac_index(*fa, toks.len());
                let b = frac_index(*fb, toks.len());
                toks.swap(a, b);
            }
            lines[i] = toks.join(" ");
        }
    }
}

proptest! {
    /// 0..12 random delete/insert/duplicate/swap edits (of lines or of
    /// whitespace-delimited tokens within a line) applied to one of the
    /// shipped example programs (or the `brix.soc` package source). The
    /// mutated text is very often no longer valid Brix, but `parse_bounded`
    /// must still only ever return `Ok`/`Err`.
    #[test]
    fn mutated_examples_never_panic(
        fixture_idx in 0..FIXTURES.len(),
        muts in proptest::collection::vec(mutation_strategy(), 0..12),
    ) {
        let mut lines: Vec<String> = FIXTURES[fixture_idx].lines().map(String::from).collect();
        for m in &muts {
            apply_mutation(&mut lines, m);
        }
        let src = lines.join("\n");
        let _ = parse_bounded(&src, ParseLimits::strict());
    }
}

// ---------------------------------------------------------------------------
// (d) Every `ParseError` position lies within the source it was raised
//     against.
// ---------------------------------------------------------------------------

/// A loose, generation-agnostic sanity envelope for a reported `(line, col)`:
/// both are 1-based and non-zero; `line` does not run past one line beyond
/// the source's own line count (a trailing-EOF error legitimately reports
/// the line *after* the last content line); `col` does not exceed the
/// source's byte length by more than a small constant (byte length is
/// always >= char length, so this never spuriously rejects a legitimate
/// multi-byte-aware column count — it only catches a position that is
/// obviously outside the source).
fn position_is_plausible(src: &str, line: usize, col: usize) -> bool {
    let total_lines = src.lines().count().max(1);
    line >= 1 && line <= total_lines + 1 && col >= 1 && col <= src.len() + 2
}

proptest! {
    #[test]
    fn parse_error_positions_lie_within_the_source(src in token_soup_strategy()) {
        if let Err(e) = parse_bounded(&src, ParseLimits::strict()) {
            if let (Some(line), Some(col)) = (e.line, e.col) {
                prop_assert!(
                    position_is_plausible(&src, line, col),
                    "reported position {line}:{col} is implausible for a {} line / {} byte source",
                    src.lines().count(),
                    src.len()
                );
            }
        }
    }

    #[test]
    fn mutated_example_parse_error_positions_lie_within_the_source(
        fixture_idx in 0..FIXTURES.len(),
        muts in proptest::collection::vec(mutation_strategy(), 0..12),
    ) {
        let mut lines: Vec<String> = FIXTURES[fixture_idx].lines().map(String::from).collect();
        for m in &muts {
            apply_mutation(&mut lines, m);
        }
        let src = lines.join("\n");
        if let Err(e) = parse_bounded(&src, ParseLimits::strict()) {
            if let (Some(line), Some(col)) = (e.line, e.col) {
                prop_assert!(
                    position_is_plausible(&src, line, col),
                    "reported position {line}:{col} is implausible for a {} line / {} byte source",
                    src.lines().count(),
                    src.len()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// (e) Deeply nested input is rejected, not stack-overflowed.
// ---------------------------------------------------------------------------

/// `n` levels of parenthesised nesting around a numeral: `(((...1...)))`.
fn nested_parens(n: usize) -> String {
    format!("let x = {}1{}", "(".repeat(n), ")".repeat(n))
}

/// `n` levels of `match` nesting, each wrapping the previous as its single
/// arm's body: `match x { A => match x { A => ... 1 ... } }`.
fn nested_match(n: usize) -> String {
    let mut s = String::from("show ");
    for _ in 0..n {
        s.push_str("match x { A => ");
    }
    s.push('1');
    for _ in 0..n {
        s.push_str(" }");
    }
    s
}

proptest! {
    /// Comfortably past `ParseLimits::strict().max_nesting_depth` (128) in
    /// every case explored: the parser must refuse (`Err`), not overflow the
    /// stack — a refusal is the *only* acceptable outcome here, so this
    /// checks `Err` directly rather than merely "did not panic".
    #[test]
    fn deep_paren_nesting_is_always_rejected(n in 200usize..3000) {
        let src = nested_parens(n);
        prop_assert!(
            parse_bounded(&src, ParseLimits::strict()).is_err(),
            "n={n} levels of paren nesting must be refused under strict limits"
        );
    }

    /// Same shape, for `match` nesting — a separate recursive-descent path
    /// (`parse_expr` re-entered for both the scrutinee and each arm body)
    /// from the unary-prefix operators `parse_limits.rs` already pins by
    /// hand, so it is exercised here by generation instead.
    #[test]
    fn deep_match_nesting_is_always_rejected(n in 200usize..3000) {
        let src = nested_match(n);
        prop_assert!(
            parse_bounded(&src, ParseLimits::strict()).is_err(),
            "n={n} levels of match nesting must be refused under strict limits"
        );
    }
}
