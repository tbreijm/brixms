//! Token grammar (Appendix D "Lexical" paragraph) via `logos`.
//!
//! Keywords are *not* distinguished at the token-kind level: every
//! identifier-shaped run of characters lexes as [`TokKind::Ident`] and the
//! parser matches on its text where the grammar names a literal keyword. This
//! keeps the lexer small and total (no "reserved word" table to keep in sync)
//! and lets the parser produce better diagnostics ("expected `derive`, found
//! identifier `derve`") than a hard keyword/ident split would.
//!
//! Trivia (comments, and significant newlines) are real tokens in the stream
//! rather than being skipped, because Appendix D makes newlines meaningful
//! ("Newlines terminate clauses inside `{}` blocks"). The parser's list
//! helpers decide, per grammatical position, whether a newline is a
//! separator or is insignificant (e.g. inside a parenthesized argument list).

use logos::Logos;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t\r\x0c]+")]
pub enum TokKind {
    // ---- trivia -------------------------------------------------------
    #[token("\n")]
    Newline,
    #[regex(r"//[^\n]*", allow_greedy = true)]
    LineComment,
    #[regex(r"/\*([^*]|\*[^/])*\*/")]
    BlockComment,

    // ---- literals -------------------------------------------------------
    /// `"..."` with `\`-escapes; contents are re-lexed by the parser to
    /// resolve escapes (keeps this regex simple and total).
    #[regex(r#""([^"\\\n]|\\.)*""#)]
    Str,
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9]+)?")]
    Float,
    #[regex(r"[0-9][0-9_]*")]
    Int,

    /// Identifiers and keywords alike (see module docs). Appendix D:
    /// "identifiers are Unicode XID normalized to NFC". We lex the ASCII
    /// XID subset here (`[A-Za-z_][A-Za-z0-9_]*`); full `\p{XID_*}` support
    /// and NFC folding are deferred to match brix-canon's own `write_ident`
    /// APP-G TODO (see this crate's report / DEPS.md discussion) rather than
    /// introducing a second, uncoordinated unicode-normalization dependency.
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,

    // ---- punctuation / operators ----------------------------------------
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,

    #[token(",")]
    Comma,
    #[token(";")]
    Semi,
    #[token("::")]
    ColonColon,
    #[token(":")]
    Colon,
    #[token("...")]
    DotDotDot,
    #[token("..")]
    DotDot,
    #[token(".")]
    Dot,
    #[token("@")]
    At,
    #[token("~")]
    Tilde,

    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("|>")]
    PipeGt,
    #[token("??")]
    QuestionQuestion,
    #[token("?")]
    Question,
    #[token("!")]
    Bang,

    #[token("==")]
    EqEq,
    #[token("!=")]
    Ne,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("=")]
    Eq,

    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
    #[token("|")]
    Pipe,
    #[token("&")]
    Amp,

    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,

    /// Anything the regexes above reject. Kept as a real token (rather than
    /// a `logos` bail) so the parser can emit one diagnostic per bad byte
    /// and resynchronize instead of aborting the whole file.
    Error,
}

use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub span: Span,
}

impl Token {
    pub fn text<'s>(&self, src: &'s str) -> &'s str {
        &src[self.span.as_range()]
    }
}

/// Lex `src` to completion. Never fails: unrecognized bytes become
/// [`TokKind::Error`] tokens (one token, one diagnostic downstream) so a
/// single bad character can't stop the parser from recovering over the rest
/// of the file.
pub fn lex(src: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut lexer = TokKind::lexer(src);
    while let Some(result) = lexer.next() {
        let kind = result.unwrap_or(TokKind::Error);
        let span = lexer.span();
        out.push(Token {
            kind,
            span: Span::new(span.start as u32, span.end as u32),
        });
    }
    out
}

/// Whether `kind` should be filtered from the token stream feeding
/// grammatical productions that don't care about trivia (i.e. everywhere
/// except newline-sensitive list separators).
pub fn is_trivia(kind: TokKind) -> bool {
    matches!(kind, TokKind::LineComment | TokKind::BlockComment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_flagship_keywords_as_ident() {
        let toks = lex("package derive rel state event mask");
        assert!(toks.iter().all(|t| t.kind == TokKind::Ident));
    }

    #[test]
    fn lexes_operators_longest_match() {
        let toks = lex("-> => |> == != <= >= ..");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TokKind::Arrow,
                TokKind::FatArrow,
                TokKind::PipeGt,
                TokKind::EqEq,
                TokKind::Ne,
                TokKind::Le,
                TokKind::Ge,
                TokKind::DotDot,
            ]
        );
    }

    #[test]
    fn newline_is_a_real_token() {
        let toks = lex("a\nb");
        assert_eq!(toks[0].kind, TokKind::Ident);
        assert_eq!(toks[1].kind, TokKind::Newline);
        assert_eq!(toks[2].kind, TokKind::Ident);
    }

    #[test]
    fn quantity_literal_is_number_then_ident_same_line() {
        // Appendix D lexical form `<number> <unit>` is realized at the parser
        // layer (see ast::Expr::Measured), not the lexer: the lexer just
        // needs Int/Float immediately followed by Ident with no Newline
        // between them.
        let toks = lex("3500 kg");
        assert_eq!(toks[0].kind, TokKind::Int);
        assert_eq!(toks[1].kind, TokKind::Ident);
    }

    #[test]
    fn unrecognized_byte_is_error_token_not_a_panic() {
        let toks = lex("a # b");
        assert_eq!(toks[0].kind, TokKind::Ident);
        assert_eq!(toks[1].kind, TokKind::Error);
        assert_eq!(toks[2].kind, TokKind::Ident);
    }
}
