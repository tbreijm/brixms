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

        tokens.push(Token {
            kind,
            line: start_line,
            col: start_col,
        });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
        col,
    });

    Ok(tokens)
}
