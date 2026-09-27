//! Doc-snippet gate: every fenced ` ```brix ` block in `README.md` and
//! `docs/brix-language.md` must actually check or lower, or be marked as an
//! intentional fragment.
//!
//! Sibling to `packaged_brix.rs`: that test guards a checked-in `.brix`
//! *file*; this one guards the `.brix` *prose* that documents the language,
//! which rots exactly the same way — a snippet that once matched the parser
//! silently stops matching it, and nobody notices until a reader copies it
//! into a fresh checkout and it fails.
//!
//! A fenced block is checked one of two ways, matching what `brix check`
//! itself does (`crates/brix-cli/src/commands/check.rs` decides the profile;
//! `crates/brix-cli/src/commands/mod.rs::prepare_finite_decision_module`
//! strips `show` items before finite-decision lowering):
//!
//! - if it contains any `propose`, `commit`, or `input` item, `show` items
//!   are stripped and `finite_decision::lower_finite_decision_plan` must
//!   succeed (this crate cannot depend on `brix-cli`, so that three-line
//!   `show`-stripping step is reproduced here rather than shared);
//! - otherwise every result of `check_module` must be `Ok`.
//!
//! A block that is intentionally a fragment — illustrating one call form, a
//! refusal, or a piece of syntax with no `commit` around it — opts out with
//! an HTML comment, `<!-- brix-snippet: fragment -->`, placed on the line
//! immediately before its opening fence (invisible when the Markdown
//! renders). A failure names the file and the fence's opening line number,
//! so it can be found without re-scanning the whole document.
//!
//! `docs/planning/` is deliberately not covered here — it documents proposed
//! syntax the parser does not accept yet, on purpose.

use brix_lower::check_module;
use brix_lower::finite_decision::{lower_finite_decision_plan, FINITE_DECISION_PROFILE};
use brix_syntax::ast::{Item, Module};
use brix_syntax::parse_bounded;

/// Run `f` on a thread with a large stack. See `packaged_brix.rs`'s module
/// doc for why: the kernel's proof-term checker (`brix_kernel::acceptance`)
/// can overflow a default 2 MiB test-thread stack at an expression depth far
/// below the parser's own nesting limit.
fn with_deep_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("checking a doc snippet must not panic")
}

const FRAGMENT_MARKER: &str = "<!-- brix-snippet: fragment -->";
const FENCE_OPEN: &str = "```brix";
const FENCE_CLOSE: &str = "```";

/// One ` ```brix ` fenced block found in a Markdown document.
struct Snippet {
    /// 1-based line number of the opening fence line, for failure messages.
    line: usize,
    source: String,
    is_fragment: bool,
}

/// Extract every ` ```brix ` fenced block from `markdown`, in document
/// order.
///
/// A fence is recognized by its trimmed line (so an indented fence, e.g.
/// inside a Markdown list item, still matches); a snippet is a fragment when
/// the line immediately preceding its opening fence trims to
/// [`FRAGMENT_MARKER`]. Source lines are passed through unindented-or-not as
/// written — the lexer treats whitespace as insignificant, so a list-nested,
/// indented snippet still parses.
fn extract_snippets(markdown: &str) -> Vec<Snippet> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut snippets = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == FENCE_OPEN {
            let is_fragment = i > 0 && lines[i - 1].trim() == FRAGMENT_MARKER;
            let line = i + 1; // 1-based, matching how editors report it
            let mut source = String::new();
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim() != FENCE_CLOSE {
                source.push_str(lines[j]);
                source.push('\n');
                j += 1;
            }
            assert!(
                j < lines.len(),
                "unterminated ```brix fence starting at line {line}"
            );
            snippets.push(Snippet {
                line,
                source,
                is_fragment,
            });
            i = j + 1;
        } else {
            i += 1;
        }
    }
    snippets
}

/// True if the module has any item that routes `brix check` to the
/// finite-decision lane (mirrors the check in
/// `crates/brix-cli/src/commands/check.rs`).
fn is_finite_decision_module(module: &Module) -> bool {
    module
        .items
        .iter()
        .any(|i| matches!(i, Item::Commit(_) | Item::Propose(_) | Item::Input(_)))
}

/// Remove surface `show` items before finite-decision lowering — exactly
/// what `crate::commands::prepare_finite_decision_module` does in
/// `brix-cli`, reproduced here since this crate is upstream of it.
fn strip_show_items(module: &mut Module) {
    module.items.retain(|i| !matches!(i, Item::Show(_)));
}

/// Check every non-fragment snippet in one Markdown file's text, panicking
/// with the file name and fence line number on the first failure.
fn check_markdown(file: &'static str, markdown: &'static str) {
    let snippets = extract_snippets(markdown);
    assert!(
        !snippets.is_empty(),
        "{file}: found no ```brix fenced blocks — did the extraction logic \
         break, or did the file stop documenting any Brix source?"
    );

    let mut checked = 0;
    for snippet in snippets {
        if snippet.is_fragment {
            continue;
        }
        checked += 1;
        let Snippet { line, source, .. } = snippet;
        with_deep_stack(move || {
            let module = match parse_bounded(&source, brix_syntax::ParseLimits::strict()) {
                Ok(m) => m,
                Err(err) => panic!(
                    "{file}:{line}: ```brix snippet failed to parse: {err}\n\
                     --- snippet ---\n{source}"
                ),
            };
            if is_finite_decision_module(&module) {
                let mut module = module;
                strip_show_items(&mut module);
                if let Err(err) = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE) {
                    panic!(
                        "{file}:{line}: finite-decision ```brix snippet failed to lower: \
                         {err}\n--- snippet ---\n{source}"
                    );
                }
            } else {
                for result in check_module(&module) {
                    if let Err((name, err)) = result {
                        panic!(
                            "{file}:{line}: ```brix snippet binding '{name}' failed to \
                             check: {err:?}\n--- snippet ---\n{source}"
                        );
                    }
                }
            }
        });
    }
    assert!(
        checked > 0,
        "{file}: every ```brix snippet is marked as a fragment — is that intentional?"
    );
}

/// Every runnable ```brix block in the top-level README must still check or
/// lower under the current parser and lowering rules.
#[test]
fn readme_brix_snippets_check() {
    check_markdown("README.md", include_str!("../../../README.md"));
}

/// Every runnable ```brix block in the language overview must still check or
/// lower under the current parser and lowering rules.
#[test]
fn brix_language_doc_snippets_check() {
    check_markdown(
        "docs/brix-language.md",
        include_str!("../../../docs/brix-language.md"),
    );
}
