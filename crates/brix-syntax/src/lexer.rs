//! Hand-written lexer for `.brix` (ADR-0010, L1).

use crate::parser::ParseError;

/// The kind of token recognized in `.brix` source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    // Keywords
    Config,
    Regime,
    Gen,
    Rule,
    Fn,
    Let,
    Show,
    Witness,
    Match,
    Prove,
    Why,
    Audit,
    Then,
    And,
    Derived,
    Audited,
    Proven,
    True,
    False,

    // Identifiers & Literals
    Ident(String),
    Num(String),
    Str(String),

    // Symbols & Operators
    OpenBrace,  // {
    CloseBrace, // }
    OpenParen,  // (
    CloseParen, // )
    Colon,      // :
    Equals,     // =
    Pipe,       // |
    Comma,      // ,
    Dot,        // .
    At,         // @
    FatArrow,   // =>
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Lt,         // <
    Le,         // <=
    Gt,         // >
    Ge,         // >=
    EqEq,       // ==
    Ne,         // !=
    Underscore, // _

    Eof,
}

/// A token with source position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}

/// Lex a `.brix` source string into a vector of tokens.
pub fn lex(source: &str) -> Result<Vec<Token>, ParseError> {
    lex_bounded(source, crate::ParseLimits::generous())
}

/// Tokenize under explicit resource bounds (ADR-0022 D6).
///
/// The token bound is charged **as tokens are produced**, not by measuring a
/// finished vector: a limit that inspects a structure already built has not
/// prevented the allocation it was meant to prevent.
pub fn lex_bounded(source: &str, limits: crate::ParseLimits) -> Result<Vec<Token>, ParseError> {
    // Checked before UTF-8 handling and before any per-token allocation.
    if source.len() > limits.max_source_bytes {
        return Err(ParseError::limit(crate::LimitExceeded::SourceBytes {
            limit: limits.max_source_bytes,
            found: source.len(),
        }));
    }
    let mut tokens = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut line = 1;
    let mut col = 1;

    while i < len {
        let ch = chars[i];

        // Line comments: // ...
        if ch == '/' && i + 1 < len && chars[i + 1] == '/' {
            i += 2;
            col += 2;
            while i < len && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }

        // Whitespace
        if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
            continue;
        }

        let start_line = line;
        let start_col = col;

        // Number literals: digits, optionally with decimal point
        if ch.is_ascii_digit() {
            let mut num_str = String::new();
            while i < len && chars[i].is_ascii_digit() {
                num_str.push(chars[i]);
                i += 1;
                col += 1;
            }
            if i + 1 < len && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                num_str.push('.');
                i += 1;
                col += 1;
                while i < len && chars[i].is_ascii_digit() {
                    num_str.push(chars[i]);
                    i += 1;
                    col += 1;
                }
            }
            charge_token(&tokens, limits)?;
            tokens.push(Token {
                kind: TokenKind::Num(num_str),
                line: start_line,
                col: start_col,
            });
            continue;
        }

        // String literals: "..."
        if ch == '"' {
            i += 1;
            col += 1;
            let mut str_val = String::new();
            let mut terminated = false;

            while i < len {
                let c = chars[i];
                if c == '"' {
                    i += 1;
                    col += 1;
                    terminated = true;
                    break;
                } else if c == '\\' {
                    i += 1;
                    col += 1;
                    if i < len {
                        let esc = chars[i];
                        match esc {
                            'n' => str_val.push('\n'),
                            'r' => str_val.push('\r'),
                            't' => str_val.push('\t'),
                            '"' => str_val.push('"'),
                            '\\' => str_val.push('\\'),
                            other => str_val.push(other),
                        }
                        i += 1;
                        col += 1;
                    } else {
                        return Err(ParseError::at(
                            "Unterminated escape sequence in string literal",
                            start_line,
                            start_col,
                        ));
                    }
                } else if c == '\n' {
                    str_val.push('\n');
                    i += 1;
                    line += 1;
                    col = 1;
                } else {
                    str_val.push(c);
                    i += 1;
                    col += 1;
                }
            }

            if !terminated {
                return Err(ParseError::at(
                    "Unterminated string literal",
                    start_line,
                    start_col,
                ));
            }

            charge_token(&tokens, limits)?;

            tokens.push(Token {
                kind: TokenKind::Str(str_val),
                line: start_line,
                col: start_col,
            });
            continue;
        }

        // Identifiers and Keywords
        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut ident = String::new();
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
                col += 1;
            }

            let kind = if ident == "_" {
                TokenKind::Underscore
            } else {
                match ident.as_str() {
                    "config" => TokenKind::Config,
                    "regime" => TokenKind::Regime,
                    "gen" => TokenKind::Gen,
                    "rule" => TokenKind::Rule,
                    "fn" => TokenKind::Fn,
                    "let" => TokenKind::Let,
                    "show" => TokenKind::Show,
                    "witness" => TokenKind::Witness,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "match" => TokenKind::Match,
                    "prove" => TokenKind::Prove,
                    "why" => TokenKind::Why,
                    "audit" => TokenKind::Audit,
                    "then" => TokenKind::Then,
                    "and" => TokenKind::And,
                    "Derived" => TokenKind::Derived,
                    "Audited" => TokenKind::Audited,
                    "Proven" => TokenKind::Proven,
                    _ => TokenKind::Ident(ident),
                }
            };

            charge_token(&tokens, limits)?;

            tokens.push(Token {
                kind,
                line: start_line,
                col: start_col,
            });
            continue;
        }

        // Operators & Symbols
        let kind = match ch {
            '{' => {
                i += 1;
                col += 1;
                TokenKind::OpenBrace
            }
            '}' => {
                i += 1;
                col += 1;
                TokenKind::CloseBrace
            }
            '(' => {
                i += 1;
                col += 1;
                TokenKind::OpenParen
            }
            ')' => {
                i += 1;
                col += 1;
                TokenKind::CloseParen
            }
            ':' => {
                i += 1;
                col += 1;
                TokenKind::Colon
            }
            '=' => {
                if i + 1 < len && chars[i + 1] == '>' {
                    i += 2;
                    col += 2;
                    TokenKind::FatArrow
                } else if i + 1 < len && chars[i + 1] == '=' {
                    i += 2;
                    col += 2;
                    TokenKind::EqEq
                } else {
                    i += 1;
                    col += 1;
                    TokenKind::Equals
                }
            }
            '|' => {
                i += 1;
                col += 1;
                TokenKind::Pipe
            }
            ',' => {
                i += 1;
                col += 1;
                TokenKind::Comma
            }
            '.' => {
                i += 1;
                col += 1;
                TokenKind::Dot
            }
            '@' => {
                i += 1;
                col += 1;
                TokenKind::At
            }
            '<' => {
                i += 1;
                col += 1;
                if chars.get(i) == Some(&'=') {
                    i += 1;
                    col += 1;
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                i += 1;
                col += 1;
                if chars.get(i) == Some(&'=') {
                    i += 1;
                    col += 1;
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '!' => {
                i += 1;
                col += 1;
                if chars.get(i) == Some(&'=') {
                    i += 1;
                    col += 1;
                    TokenKind::Ne
                } else {
                    return Err(ParseError::at(
                        "Unexpected character '!' (did you mean '!=' ?)".to_string(),
                        start_line,
                        start_col,
                    ));
                }
            }
            '+' => {
                i += 1;
                col += 1;
                TokenKind::Plus
            }
            '-' => {
                i += 1;
                col += 1;
                TokenKind::Minus
            }
            '*' => {
                i += 1;
                col += 1;
                TokenKind::Star
            }
            '/' => {
                i += 1;
                col += 1;
                TokenKind::Slash
            }
            _ => {
                return Err(ParseError::at(
                    format!("Unexpected character '{}'", ch),
                    start_line,
                    start_col,
                ));
            }
        };

        charge_token(&tokens, limits)?;

        tokens.push(Token {
            kind,
            line: start_line,
            col: start_col,
        });
    }

    charge_token(&tokens, limits)?;

    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
        col,
    });

    Ok(tokens)
}

/// Refuse before the push that would exceed the bound.
fn charge_token(tokens: &[Token], limits: crate::ParseLimits) -> Result<(), ParseError> {
    if tokens.len() >= limits.max_tokens {
        return Err(ParseError::limit(crate::LimitExceeded::Tokens {
            limit: limits.max_tokens,
        }));
    }
    Ok(())
}
