//! Recursive-descent parser for `.brix` (ADR-0010, L1).

use crate::ast::*;
use crate::lexer::{self, Token, TokenKind};

/// A parse error with a human-readable message and optional source location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            col: None,
        }
    }

    pub fn at(message: impl Into<String>, line: usize, col: usize) -> Self {
        Self {
            message: message.into(),
            line: Some(line),
            col: Some(col),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let (Some(line), Some(col)) = (self.line, self.col) {
            write!(f, "Parse error at {}:{}: {}", line, col, self.message)
        } else {
            write!(f, "Parse error: {}", self.message)
        }
    }
}

impl ParseError {
    /// A resource refusal (ADR-0022 D6/D8). Distinguished in the message so a
    /// caller can tell "this source is malformed" from "this verifier declined
    /// to spend the resources", which are different facts.
    pub fn limit(exceeded: crate::LimitExceeded) -> Self {
        ParseError::new(format!("resource limit exceeded: {exceeded}"))
    }
}

impl std::error::Error for ParseError {}

/// Parse a `.brix` source string into a [`Module`].
///
/// Unbounded, for ordinary in-process callers that already control their own
/// input. A verifier re-deriving a manifest from *supplied* source must use
/// [`parse_bounded`] instead (ADR-0022 D6) — there the source is
/// attacker-controlled and the frontend is inside the trusted closure.
pub fn parse(source: &str) -> Result<Module, ParseError> {
    parse_bounded(source, crate::ParseLimits::generous())
}

/// Parse under explicit resource bounds (ADR-0022 D6).
///
/// Every bound is enforced *before* the work it governs: source length before
/// tokenization, token count as tokens are produced, and nesting depth before
/// each recursive descent. A refusal is a typed error and never a partial
/// module; there is no permissive retry.
pub fn parse_bounded(source: &str, limits: crate::ParseLimits) -> Result<Module, ParseError> {
    let tokens = lexer::lex_bounded(source, limits)?;
    let mut parser = Parser::new(tokens, limits);
    parser.parse_module()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    limits: crate::ParseLimits,
    /// Current recursive-descent depth. Incremented on entry to each
    /// recursive expression rule and decremented on exit, so the bound tracks
    /// live stack rather than total rule applications.
    depth: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>, limits: crate::ParseLimits) -> Self {
        Self {
            tokens,
            pos: 0,
            limits,
            depth: 0,
        }
    }

    /// Charge one level of nesting, refusing **before** the recursive call so
    /// a deep input is rejected rather than overflowing the stack.
    fn enter(&mut self) -> Result<(), ParseError> {
        if self.depth >= self.limits.max_nesting_depth {
            return Err(ParseError::limit(crate::LimitExceeded::NestingDepth {
                limit: self.limits.max_nesting_depth,
            }));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn current(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            self.tokens.last().expect("tokens is never empty")
        }
    }

    fn peek(&self) -> &TokenKind {
        &self.current().kind
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.pos += 1;
        }
        &self.tokens[self.pos - 1]
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn error(&self, msg: impl Into<String>) -> ParseError {
        let tok = self.current();
        ParseError::at(msg, tok.line, tok.col)
    }

    fn consume(&mut self, expected: TokenKind, context: &str) -> Result<&Token, ParseError> {
        if self.check(&expected) {
            Ok(self.advance())
        } else {
            Err(self.error(format!(
                "Expected {:?}, found {:?} in {}",
                expected,
                self.peek(),
                context
            )))
        }
    }

    fn expect_ident(&mut self, context: &str) -> Result<(String, Token), ParseError> {
        match self.peek().clone() {
            TokenKind::Ident(s) => {
                let tok = self.advance().clone();
                Ok((s, tok))
            }
            other => Err(self.error(format!(
                "Expected identifier, found {:?} in {}",
                other, context
            ))),
        }
    }

    fn is_record_literal_ahead(&self) -> bool {
        if let TokenKind::Ident(id) = self.peek() {
            if !id.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                return false;
            }
            if self.pos + 1 < self.tokens.len()
                && self.tokens[self.pos + 1].kind == TokenKind::OpenBrace
                && self.pos + 2 < self.tokens.len()
            {
                match &self.tokens[self.pos + 2].kind {
                    TokenKind::CloseBrace => return true,
                    TokenKind::Ident(_) if self.pos + 3 < self.tokens.len() => {
                        return self.tokens[self.pos + 3].kind == TokenKind::Colon;
                    }
                    _ => {}
                }
            }
        }
        false
    }

    fn parse_comma_separated<T>(
        &mut self,
        end_kind: TokenKind,
        mut parse_elem: impl FnMut(&mut Self) -> Result<T, ParseError>,
    ) -> Result<Vec<T>, ParseError> {
        let mut list = Vec::new();
        if !self.check(&end_kind) && !self.is_at_end() {
            loop {
                list.push(parse_elem(self)?);
                if self.check(&TokenKind::Comma) {
                    self.advance();
                    if self.check(&end_kind) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        Ok(list)
    }

    fn parse_module(&mut self) -> Result<Module, ParseError> {
        let mut items = Vec::new();
        while !self.is_at_end() {
            items.push(self.parse_item()?);
        }
        Ok(Module { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek() {
            TokenKind::Config => {
                self.advance();
                self.parse_config_decl().map(Item::Config)
            }
            TokenKind::Regime => {
                self.advance();
                self.parse_regime_decl().map(Item::Regime)
            }
            TokenKind::Rule => {
                self.advance();
                self.parse_callable().map(Item::Rule)
            }
            TokenKind::Fn => {
                self.advance();
                self.parse_callable().map(Item::Fn)
            }
            TokenKind::Let => {
                self.advance();
                self.parse_let_decl().map(Item::Let)
            }
            TokenKind::Show => {
                self.advance();
                self.parse_expr().map(Item::Show)
            }
            TokenKind::Witness => {
                self.advance();
                let name = self.expect_ident("witness declaration")?.0;
                self.consume(TokenKind::Equals, "witness declaration")?;
                let value = self.parse_expr()?;
                Ok(Item::Witness { name, value })
            }
            other => Err(self.error(format!("Unexpected token {:?} at top-level item", other))),
        }
    }

    fn parse_config_decl(&mut self) -> Result<ConfigDecl, ParseError> {
        let name = self.expect_ident("config declaration name")?.0;
        self.consume(TokenKind::Equals, "config declaration '='")?;
        if self.check(&TokenKind::OpenBrace) {
            self.advance();
            let fields =
                self.parse_comma_separated(TokenKind::CloseBrace, |p| p.parse_field_decl())?;
            self.consume(TokenKind::CloseBrace, "config record '}'")?;
            Ok(ConfigDecl {
                name,
                body: ConfigBody::Record(fields),
            })
        } else {
            let mut variants = Vec::new();
            loop {
                variants.push(self.parse_variant()?);
                if self.check(&TokenKind::Pipe) {
                    self.advance();
                } else {
                    break;
                }
            }
            Ok(ConfigDecl {
                name,
                body: ConfigBody::Sum(variants),
            })
        }
    }

    fn parse_field_decl(&mut self) -> Result<FieldDecl, ParseError> {
        let name = self.expect_ident("field name")?.0;
        self.consume(TokenKind::Colon, "field ':'")?;
        let ty = self.parse_ty()?;
        Ok(FieldDecl { name, ty })
    }

    fn parse_variant(&mut self) -> Result<Variant, ParseError> {
        let name = self.expect_ident("variant name")?.0;
        let params = if self.check(&TokenKind::OpenParen) {
            self.advance();
            let params = self.parse_comma_separated(TokenKind::CloseParen, |p| p.parse_ty())?;
            self.consume(TokenKind::CloseParen, "variant closing ')'")?;
            params
        } else {
            Vec::new()
        };
        Ok(Variant { name, params })
    }

    fn parse_regime_decl(&mut self) -> Result<RegimeDecl, ParseError> {
        let name = self.expect_ident("regime name")?.0;
        self.consume(TokenKind::OpenBrace, "regime '{'")?;
        let mut gens = Vec::new();
        while !self.check(&TokenKind::CloseBrace) && !self.is_at_end() {
            self.consume(TokenKind::Gen, "regime 'gen'")?;
            gens.push(self.parse_callable()?);
        }
        self.consume(TokenKind::CloseBrace, "regime '}'")?;
        Ok(RegimeDecl { name, gens })
    }

    fn parse_callable(&mut self) -> Result<Callable, ParseError> {
        let name = self.expect_ident("callable name")?.0;
        self.consume(TokenKind::OpenParen, "callable '('")?;
        let params = self.parse_comma_separated(TokenKind::CloseParen, |p| p.parse_param())?;
        self.consume(TokenKind::CloseParen, "callable ')'")?;
        let ret = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_ty()?)
        } else {
            None
        };
        self.consume(TokenKind::Equals, "callable '='")?;
        let body = self.parse_expr()?;
        Ok(Callable {
            name,
            params,
            ret,
            body,
        })
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let name = self.expect_ident("parameter name")?.0;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_ty()?)
        } else {
            None
        };
        Ok(Param { name, ty })
    }

    fn parse_let_decl(&mut self) -> Result<LetDecl, ParseError> {
        let name = self.expect_ident("let variable name")?.0;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_ty()?)
        } else {
            None
        };
        self.consume(TokenKind::Equals, "let declaration '='")?;
        let value = self.parse_expr()?;
        Ok(LetDecl { name, ty, value })
    }

    fn parse_ty(&mut self) -> Result<Ty, ParseError> {
        let name = self.expect_ident("type name")?.0;
        let base_ty = Ty::Named(name);
        if self.check(&TokenKind::At) {
            self.advance();
            let grade = match self.peek() {
                TokenKind::Derived => {
                    self.advance();
                    Grade::Derived
                }
                TokenKind::Audited => {
                    self.advance();
                    Grade::Audited
                }
                TokenKind::Proven => {
                    self.advance();
                    Grade::Proven
                }
                other => {
                    return Err(self.error(format!(
                        "Expected grade after '@' (Derived, Audited, Proven), found {:?}",
                        other
                    )));
                }
            };
            Ok(Ty::Graded(Box::new(base_ty), grade))
        } else {
            Ok(base_ty)
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.enter()?;
        let out = self.parse_expr_inner();
        self.leave();
        out
    }

    fn parse_expr_inner(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_bin1()
    }

    fn parse_expr_bin1(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_expr_bin2()?;
        while let Some(op) = self.match_bin1() {
            let rhs = self.parse_expr_bin2()?;
            lhs = Expr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn match_bin1(&mut self) -> Option<BinOp> {
        if self.check(&TokenKind::Then) {
            self.advance();
            Some(BinOp::Then)
        } else if self.check(&TokenKind::And) {
            self.advance();
            Some(BinOp::And)
        } else {
            None
        }
    }

    fn parse_expr_bin2(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_expr_bin3()?;
        while let Some(op) = self.match_bin2() {
            let rhs = self.parse_expr_bin3()?;
            lhs = Expr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn match_bin2(&mut self) -> Option<BinOp> {
        if self.check(&TokenKind::Plus) {
            self.advance();
            Some(BinOp::Add)
        } else if self.check(&TokenKind::Minus) {
            self.advance();
            Some(BinOp::Sub)
        } else {
            None
        }
    }

    fn parse_expr_bin3(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_expr_prefix()?;
        while let Some(op) = self.match_bin3() {
            let rhs = self.parse_expr_prefix()?;
            lhs = Expr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn match_bin3(&mut self) -> Option<BinOp> {
        if self.check(&TokenKind::Star) {
            self.advance();
            Some(BinOp::Mul)
        } else if self.check(&TokenKind::Slash) {
            self.advance();
            Some(BinOp::Div)
        } else {
            None
        }
    }

    fn parse_expr_prefix(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            TokenKind::Prove => {
                self.advance();
                let inner = self.parse_expr_prefix()?;
                Ok(Expr::Prove(Box::new(inner)))
            }
            TokenKind::Audit => {
                self.advance();
                let inner = self.parse_expr_prefix()?;
                Ok(Expr::Audit(Box::new(inner)))
            }
            TokenKind::Why => {
                self.advance();
                self.consume(TokenKind::OpenParen, "why argument '('")?;
                let inner = self.parse_expr()?;
                self.consume(TokenKind::CloseParen, "why argument ')'")?;
                Ok(Expr::Why(Box::new(inner)))
            }
            _ => self.parse_expr_postfix(),
        }
    }

    fn parse_expr_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_expr_primary()?;
        loop {
            if self.check(&TokenKind::Dot) {
                self.advance();
                let field_name = self.expect_ident("field access after '.'")?.0;
                expr = Expr::Field(Box::new(expr), field_name);
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_expr_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            TokenKind::Num(n) => {
                self.advance();
                Ok(Expr::Num(n))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str(s))
            }
            TokenKind::Match => {
                self.advance();
                let scrutinee = Box::new(self.parse_expr()?);
                self.consume(TokenKind::OpenBrace, "match body '{'")?;
                let mut arms = Vec::new();
                while !self.check(&TokenKind::CloseBrace) && !self.is_at_end() {
                    arms.push(self.parse_match_arm()?);
                }
                self.consume(TokenKind::CloseBrace, "match body '}'")?;
                let proving_exhaustive = self.parse_optional_proving_exhaustive()?;
                Ok(Expr::Match {
                    scrutinee,
                    arms,
                    proving_exhaustive,
                })
            }
            TokenKind::OpenParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.consume(TokenKind::CloseParen, "grouped expression ')'")?;
                Ok(expr)
            }
            TokenKind::Ident(id) => {
                if self.is_record_literal_ahead() {
                    self.advance(); // consume config name
                    self.advance(); // consume '{'
                    let fields = self.parse_comma_separated(TokenKind::CloseBrace, |p| {
                        let fname = p.expect_ident("record field name")?.0;
                        p.consume(TokenKind::Colon, "':' in record literal")?;
                        let fexpr = p.parse_expr()?;
                        Ok((fname, fexpr))
                    })?;
                    self.consume(TokenKind::CloseBrace, "record literal '}'")?;
                    Ok(Expr::Record { config: id, fields })
                } else {
                    self.advance();
                    if self.check(&TokenKind::OpenParen) {
                        self.advance();
                        let args =
                            self.parse_comma_separated(TokenKind::CloseParen, |p| p.parse_expr())?;
                        self.consume(TokenKind::CloseParen, "function call ')'")?;
                        Ok(Expr::Call { func: id, args })
                    } else {
                        Ok(Expr::Var(id))
                    }
                }
            }
            other => Err(self.error(format!("Unexpected token {:?} in expression", other))),
        }
    }

    /// Optionally consume a trailing `proving exhaustive` after a match
    /// block's closing `}`. `proving`/`exhaustive` are contextual — plain
    /// identifiers everywhere else — so this only looks for them in this
    /// specific post-match position and never reserves the words.
    fn parse_optional_proving_exhaustive(&mut self) -> Result<bool, ParseError> {
        let is_proving = matches!(self.peek(), TokenKind::Ident(id) if id == "proving");
        if !is_proving {
            return Ok(false);
        }
        self.advance();
        match self.peek() {
            TokenKind::Ident(id) if id == "exhaustive" => {
                self.advance();
                Ok(true)
            }
            other => Err(self.error(format!(
                "Expected 'exhaustive' after 'proving', found {:?}",
                other
            ))),
        }
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_pattern()?;
        self.consume(TokenKind::FatArrow, "'=>' in match arm")?;
        let body = self.parse_expr()?;
        Ok(MatchArm { pattern, body })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        if self.check(&TokenKind::Underscore) {
            self.advance();
            return Ok(Pattern::Wildcard);
        }

        let (name, _) = self.expect_ident("pattern")?;
        let is_capitalized = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());

        if self.check(&TokenKind::OpenParen) {
            self.advance();
            let args = self.parse_comma_separated(TokenKind::CloseParen, |p| p.parse_pattern())?;
            self.consume(TokenKind::CloseParen, "pattern ')'")?;
            Ok(Pattern::Ctor { name, args })
        } else if is_capitalized {
            Ok(Pattern::Ctor { name, args: vec![] })
        } else {
            Ok(Pattern::Var(name))
        }
    }
}
