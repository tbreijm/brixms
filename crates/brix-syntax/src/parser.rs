//! Recursive-descent parser for `.brix` (ADR-0010, L1).

use crate::ast::*;
use crate::lexer::{self, Token, TokenKind};
use crate::source_map::{self, IdentOccurrence, ItemSourceInfo, SourceMap, SourceSpan};

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
    let (module, _source_map) = parse_bounded_with_source_map(source, limits)?;
    Ok(module)
}

/// Parse under explicit resource bounds (ADR-0022 D6), also returning a sidecar
/// [`SourceMap`] recording where each top-level item — and each identifier
/// token inside it — came from.
///
/// Runs the *exact same* parse as [`parse_bounded`] (which is defined in terms
/// of this function and simply discards the map), so the two can never return
/// different [`Module`]s for the same input.
pub fn parse_bounded_with_source_map(
    source: &str,
    limits: crate::ParseLimits,
) -> Result<(Module, SourceMap), ParseError> {
    let tokens = lexer::lex_bounded(source, limits)?;
    let mut parser = Parser::new(tokens, limits);
    let module = parser.parse_module()?;
    Ok((
        module,
        SourceMap {
            items: parser.item_source,
        },
    ))
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    limits: crate::ParseLimits,
    /// Current recursive-descent depth. Incremented on entry to each
    /// recursive expression rule and decremented on exit, so the bound tracks
    /// live stack rather than total rule applications.
    depth: usize,
    /// Sidecar per-item source info, recorded as each top-level item is
    /// parsed (see [`Self::parse_module`]). Never consulted by parsing itself
    /// — purely an additional recording alongside it.
    item_source: Vec<ItemSourceInfo>,
}

impl Parser {
    fn new(tokens: Vec<Token>, limits: crate::ParseLimits) -> Self {
        Self {
            tokens,
            pos: 0,
            limits,
            depth: 0,
            item_source: Vec::new(),
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
            let start_pos = self.pos;
            let item = self.parse_item()?;
            self.record_item_source(&item, start_pos, self.pos);
            items.push(item);
        }
        Ok(Module { items })
    }

    /// Record the span and identifier-token occurrences of the item that was
    /// just parsed from token range `[start_pos, end_pos)`. Purely additive
    /// bookkeeping for [`SourceMap`] — never observed by the parse itself.
    fn record_item_source(&mut self, item: &Item, start_pos: usize, end_pos: usize) {
        if start_pos >= end_pos || end_pos > self.tokens.len() {
            return;
        }
        let start_tok = &self.tokens[start_pos];
        let end_tok = &self.tokens[end_pos - 1];
        let span = SourceSpan {
            start_line: start_tok.line,
            start_col: start_tok.col,
            end_line: end_tok.line,
            end_col: end_tok.col,
        };
        let mut idents = Vec::new();
        for tok in &self.tokens[start_pos..end_pos] {
            if let TokenKind::Ident(name) = &tok.kind {
                idents.push(IdentOccurrence {
                    name: name.clone(),
                    line: tok.line,
                    col: tok.col,
                });
            }
        }
        let (kind, name) = source_map::item_kind_name(item);
        self.item_source.push(ItemSourceInfo {
            kind,
            name,
            span,
            idents,
        });
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek() {
            TokenKind::Use => {
                self.advance();
                self.parse_use_path().map(Item::Use)
            }
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
                self.parse_rule_decl().map(Item::Rule)
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
            TokenKind::Propose => {
                self.advance();
                self.parse_propose_decl().map(Item::Propose)
            }
            TokenKind::Commit => {
                self.advance();
                self.parse_commit_decl().map(Item::Commit)
            }
            TokenKind::Input => {
                self.advance();
                self.parse_input_decl().map(Item::Input)
            }
            other => Err(self.error(format!("Unexpected token {:?} at top-level item", other))),
        }
    }

    /// `use brix.soc` — a dotted package path.
    fn parse_use_path(&mut self) -> Result<String, ParseError> {
        let mut path = self.expect_ident("package name")?.0;
        while self.check(&TokenKind::Dot) {
            self.advance();
            path.push('.');
            path.push_str(&self.expect_ident("package path segment")?.0);
        }
        Ok(path)
    }

    fn parse_config_decl(&mut self) -> Result<ConfigDecl, ParseError> {
        let name = self.expect_ident("config declaration name")?.0;
        // `config List<T, U> = …`. Absent for an ordinary config.
        let mut params = Vec::new();
        if self.check(&TokenKind::Lt) {
            self.advance();
            loop {
                params.push(self.expect_ident("type parameter")?.0);
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.consume(TokenKind::Gt, "type parameter list '>'")?;
        }
        self.consume(TokenKind::Equals, "config declaration '='")?;
        if self.check(&TokenKind::OpenBrace) {
            self.advance();
            let fields =
                self.parse_comma_separated(TokenKind::CloseBrace, |p| p.parse_field_decl())?;
            self.consume(TokenKind::CloseBrace, "config record '}'")?;
            Ok(ConfigDecl {
                name,
                params,
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
                params,
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
        } else if self.check(&TokenKind::OpenBrace) {
            // A named-field variant. Desugared to one positional parameter of
            // anonymous record type — see `ast::Variant`.
            self.advance();
            let fields =
                self.parse_comma_separated(TokenKind::CloseBrace, |p| p.parse_field_decl())?;
            self.consume(TokenKind::CloseBrace, "variant closing '}'")?;
            vec![Ty::Record(fields)]
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
            params_declared: true,
        })
    }

    /// `rule name[(deps...)] [: Ty] = body` (ADR-0038: the dependency list is
    /// optional — omitting it entirely means "infer from the body", distinct
    /// from an explicit empty `()`).
    fn parse_rule_decl(&mut self) -> Result<Callable, ParseError> {
        let name = self.expect_ident("rule declaration name")?.0;
        let (params, params_declared) = if self.check(&TokenKind::OpenParen) {
            self.advance();
            let params = self.parse_comma_separated(TokenKind::CloseParen, |p| p.parse_param())?;
            self.consume(TokenKind::CloseParen, "rule declaration ')'")?;
            (params, true)
        } else {
            (Vec::new(), false)
        };
        let ret = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_ty()?)
        } else {
            None
        };
        self.consume(TokenKind::Equals, "rule declaration '='")?;
        let body = self.parse_expr()?;
        Ok(Callable {
            name,
            params,
            ret,
            body,
            params_declared,
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

    /// `propose NAME[(DEPS...)] priority UINT when GUARD = VALUE`, or the
    /// `otherwise` fallback sugar `propose NAME[(DEPS...)] otherwise = VALUE`
    /// (ADR-0038). The dependency list is optional, as for `rule`; omitting
    /// it means dependencies are inferred from the guard and value.
    fn parse_propose_decl(&mut self) -> Result<ProposeDecl, ParseError> {
        let name = self.expect_ident("propose candidate name")?.0;
        let (deps, deps_declared) = if self.check(&TokenKind::OpenParen) {
            self.advance();
            let deps = self.parse_comma_separated(TokenKind::CloseParen, |p| {
                p.expect_ident("candidate dependency").map(|(id, _)| id)
            })?;
            self.consume(TokenKind::CloseParen, "propose candidate dependencies ')'")?;
            (deps, true)
        } else {
            (Vec::new(), false)
        };
        if self.check(&TokenKind::Otherwise) {
            self.advance();
            self.consume(
                TokenKind::Equals,
                "propose declaration '=' after 'otherwise'",
            )?;
            let value = self.parse_expr()?;
            return Ok(ProposeDecl {
                name,
                deps,
                priority: u64::MAX,
                guard: Expr::Bool(true),
                value,
                deps_declared,
                otherwise: true,
            });
        }
        self.consume(TokenKind::Priority, "propose declaration 'priority'")?;
        let priority = self.parse_priority()?;
        self.consume(TokenKind::When, "propose declaration 'when'")?;
        let guard = self.parse_expr()?;
        self.consume(
            TokenKind::Equals,
            "propose declaration '=' between guard and value",
        )?;
        let value = self.parse_expr()?;
        Ok(ProposeDecl {
            name,
            deps,
            priority,
            guard,
            value,
            deps_declared,
            otherwise: false,
        })
    }

    fn parse_priority(&mut self) -> Result<u64, ParseError> {
        let tok = self.current().clone();
        match &tok.kind {
            TokenKind::Num(s) => match s.parse::<u64>() {
                Ok(val) => {
                    self.advance();
                    Ok(val)
                }
                Err(_) => Err(ParseError::at(
                    format!(
                        "Expected nonnegative unsigned integer for priority, found '{}'",
                        s
                    ),
                    tok.line,
                    tok.col,
                )),
            },
            other => Err(ParseError::at(
                format!(
                    "Expected nonnegative unsigned integer for priority, found {:?}",
                    other
                ),
                tok.line,
                tok.col,
            )),
        }
    }

    fn parse_commit_decl(&mut self) -> Result<CommitDecl, ParseError> {
        let name = self.expect_ident("commit declaration name")?.0;
        self.consume(TokenKind::From, "commit declaration 'from'")?;
        let lparen_tok = self
            .consume(TokenKind::OpenParen, "commit candidate list '('")?
            .clone();
        let candidates = self.parse_comma_separated(TokenKind::CloseParen, |p| {
            p.expect_ident("candidate identifier in commit")
                .map(|(id, _)| id)
        })?;
        self.consume(TokenKind::CloseParen, "commit candidate list ')'")?;
        if candidates.is_empty() {
            return Err(ParseError::at(
                "commit candidate list cannot be empty",
                lparen_tok.line,
                lparen_tok.col,
            ));
        }
        Ok(CommitDecl { name, candidates })
    }

    fn parse_input_decl(&mut self) -> Result<InputDecl, ParseError> {
        let name = self.expect_ident("input declaration name")?.0;
        self.consume(TokenKind::Colon, "input declaration ':'")?;
        let ty = self.parse_ty()?;
        // `List<T> max N` (ADR-0037). `max` is a contextual identifier here,
        // exactly as `proving`/`exhaustive` are contextual after a `match`:
        // it is not reserved anywhere else in the grammar.
        let list_max = if matches!(&ty, Ty::App(name, _) if name == "List") {
            match self.peek().clone() {
                TokenKind::Ident(id) if id == "max" => {
                    self.advance();
                    let tok = self.current().clone();
                    match &tok.kind {
                        TokenKind::Num(s) if !s.contains('.') => match s.parse::<u64>() {
                            Ok(v) => {
                                self.advance();
                                Some(v)
                            }
                            Err(_) => {
                                return Err(ParseError::at(
                                    format!(
                                        "expected nonnegative unsigned integer for 'max', found '{s}'"
                                    ),
                                    tok.line,
                                    tok.col,
                                ));
                            }
                        },
                        other => {
                            return Err(ParseError::at(
                                format!(
                                    "expected nonnegative unsigned integer after 'max', found {other:?}"
                                ),
                                tok.line,
                                tok.col,
                            ));
                        }
                    }
                }
                other => {
                    return Err(self.error(format!(
                        "expected 'max' bound after 'List<...>' input type, found {other:?}"
                    )));
                }
            }
        } else {
            None
        };
        Ok(InputDecl { name, ty, list_max })
    }

    fn parse_ty(&mut self) -> Result<Ty, ParseError> {
        let name = self.expect_ident("type name")?.0;
        // `List<Int>` — a parameterized config at an instantiation. The `<`
        // is unambiguous here because a type position has no comparison.
        let base_ty = if self.check(&TokenKind::Lt) {
            self.advance();
            let mut args = Vec::new();
            loop {
                args.push(self.parse_ty()?);
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.consume(TokenKind::Gt, "type argument list '>'")?;
            Ty::App(name, args)
        } else {
            Ty::Named(name)
        };
        if self.check(&TokenKind::At) {
            self.advance();
            // Grade names are contextual: they are recognized here, in grade
            // position, and are ordinary identifiers everywhere else.
            let grade = match self.peek() {
                TokenKind::Ident(name) if name == "Derived" => {
                    self.advance();
                    Grade::Derived
                }
                TokenKind::Ident(name) if name == "Audited" => {
                    self.advance();
                    Grade::Audited
                }
                TokenKind::Ident(name) if name == "Proven" => {
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
        self.parse_expr_bp(0)
    }

    /// One call argument: either an ordinary expression, or a hygienic
    /// binder `ident => expr` (ADR-0037, ADR-0040). Lambdas are fold/filter/
    /// map syntax only — recognized here structurally, independent of the
    /// callee's name, and rejected at lowering wherever the callee is not one
    /// of the recognized builtin forms.
    fn parse_call_arg(&mut self) -> Result<Expr, ParseError> {
        if let TokenKind::Ident(name) = self.peek().clone() {
            if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::FatArrow) {
                self.advance(); // binder identifier
                self.advance(); // '=>'
                self.enter()?;
                let body = self.parse_expr_inner();
                self.leave();
                return Ok(Expr::Lambda {
                    param: name,
                    body: Box::new(body?),
                });
            }
        }
        self.parse_expr()
    }

    /// Precedence climbing for binary operators.
    ///
    /// Precedence levels (lowest to highest):
    /// 1. `then`, `and` (witness composition)
    /// 2. `||` (logical OR)
    /// 3. `&&` (logical AND)
    /// 4. `<`, `<=`, `>`, `>=`, `==`, `!=` (non-associative comparison)
    /// 5. `+`, `-` (additive)
    /// 6. `*`, `/` (multiplicative)
    ///
    /// Higher still, outside this ladder entirely, are the prefix operators
    /// (`!`, unary `-`, `prove`, `audit`) handled by [`Self::parse_expr_prefix`]
    /// before this function's loop ever runs, and postfix `.field` handled by
    /// [`Self::parse_expr_postfix`] before that. So `-a * b` is `(-a) * b`
    /// (unary minus binds tighter than every binary operator here), and `-a.field`
    /// is `-(a.field)` (postfix binds tighter than prefix).
    fn parse_expr_bp(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_expr_prefix()?;

        loop {
            let (left_bp, right_bp, op, is_cmp) = match self.peek() {
                TokenKind::Then => (1, 2, BinOp::Then, false),
                TokenKind::And => (1, 2, BinOp::And, false),
                TokenKind::PipePipe => (3, 4, BinOp::OrOr, false),
                TokenKind::AmpAmp => (5, 6, BinOp::AndAnd, false),
                TokenKind::Lt => (7, 8, BinOp::Lt, true),
                TokenKind::Le => (7, 8, BinOp::Le, true),
                TokenKind::Gt => (7, 8, BinOp::Gt, true),
                TokenKind::Ge => (7, 8, BinOp::Ge, true),
                TokenKind::EqEq => (7, 8, BinOp::Eq, true),
                TokenKind::Ne => (7, 8, BinOp::Ne, true),
                TokenKind::In => (7, 8, BinOp::In, true),
                TokenKind::Plus => (9, 10, BinOp::Add, false),
                TokenKind::Minus => (9, 10, BinOp::Sub, false),
                TokenKind::Star => (11, 12, BinOp::Mul, false),
                TokenKind::Slash => (11, 12, BinOp::Div, false),
                _ => break,
            };

            if left_bp < min_bp {
                break;
            }

            self.advance();

            let rhs = self.parse_expr_bp(right_bp)?;

            if is_cmp && self.peek_cmp() {
                return Err(self.error("comparison operators do not chain; parenthesise instead"));
            }

            lhs = Expr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }

        Ok(lhs)
    }

    fn peek_cmp(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::EqEq
                | TokenKind::Ne
                | TokenKind::In
        )
    }

    /// Operand of a self-recursive prefix operator (`!`, unary `-`, `prove`,
    /// `audit`).
    ///
    /// These arms recurse into [`Self::parse_expr_prefix`] directly rather than
    /// going back through [`Self::parse_expr`], so without this they descend
    /// **uncharged**: `!!!!…true` is one expression at depth 1 and one stack
    /// frame per `!`. That contradicts `ParseLimits::max_nesting_depth`, whose
    /// whole contract is that a deep input is "refused before descending, so
    /// the stack is never at risk" (ADR-0022 D6). Charging here restores it.
    ///
    /// Unary minus takes this path only for its general `0 - e` desugaring
    /// (`-a`, `-(e)`, …); the numeral-folding shortcut for `-<digits>` returns
    /// straight out of [`Self::parse_expr_prefix`] without recursing at all,
    /// so it needs no charge — see the `Minus` arm there.
    fn parse_prefix_operand(&mut self) -> Result<Expr, ParseError> {
        self.enter()?;
        let out = self.parse_expr_prefix();
        self.leave();
        out
    }

    fn parse_expr_prefix(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            TokenKind::Bang => {
                self.advance();
                let inner = self.parse_prefix_operand()?;
                Ok(Expr::Not(Box::new(inner)))
            }
            TokenKind::Minus => {
                self.advance();
                // A numeral immediately following `-` folds into a negative
                // numeric literal rather than desugaring to `0 - n`. This is
                // not merely cosmetic: `i64::MIN` (`-9223372036854775808`) has
                // no positive counterpart representable in `i64`, so `0 -
                // 9223372036854775808` can never be built (the positive
                // magnitude alone already overflows `i64::MAX` and would fall
                // through to a float literal). Folding the sign directly into
                // the literal text lets `s.parse::<i64>()` succeed on the
                // *negative* string, which is the only representable form.
                if let TokenKind::Num(n) = self.peek().clone() {
                    self.advance();
                    return Ok(Expr::Num(format!("-{n}")));
                }
                // Every other operand (`-a`, `-a.field`, `-f(x)`, `-(e)`, a
                // nested `--e`, …) desugars to `0 - e`, which is exactly
                // `BinOp::Sub` and therefore needs no new AST or `L3ExprV2`
                // variant, no new `encode_expr_v2` ordinal, and gets checked
                // (never wrapping) overflow for free from the existing
                // `Arith` evaluation path. The operand is parsed through
                // `parse_prefix_operand`, not `parse_expr`, so a chain of
                // unary minuses is charged against `max_nesting_depth` exactly
                // like `!`/`prove`/`audit` — see that function's doc comment.
                let inner = self.parse_prefix_operand()?;
                Ok(Expr::Bin {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Num("0".to_string())),
                    rhs: Box::new(inner),
                })
            }
            TokenKind::Prove => {
                self.advance();
                let inner = self.parse_prefix_operand()?;
                Ok(Expr::Prove(Box::new(inner)))
            }
            TokenKind::Audit => {
                self.advance();
                let inner = self.parse_prefix_operand()?;
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
            TokenKind::True => {
                self.advance();
                Ok(Expr::Bool(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::Bool(false))
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
            TokenKind::OpenBracket => {
                self.advance();
                let elems =
                    self.parse_comma_separated(TokenKind::CloseBracket, |p| p.parse_expr())?;
                self.consume(TokenKind::CloseBracket, "list literal ']'")?;
                Ok(Expr::ListLit(elems))
            }
            TokenKind::For => {
                self.advance();
                let mut generators = Vec::new();
                loop {
                    let binder = self.expect_ident("comprehension generator binder")?.0;
                    self.consume(TokenKind::In, "comprehension generator 'in'")?;
                    let source = self.parse_expr()?;
                    generators.push((binder, source));
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                        continue;
                    }
                    break;
                }
                let where_clause = if self.check(&TokenKind::Where) {
                    self.advance();
                    Some(Box::new(self.parse_expr()?))
                } else {
                    None
                };
                self.consume(TokenKind::Yield, "comprehension 'yield'")?;
                let yield_expr = Box::new(self.parse_expr()?);
                Ok(Expr::Comprehension {
                    generators,
                    where_clause,
                    yield_expr,
                })
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
                        let args = self
                            .parse_comma_separated(TokenKind::CloseParen, |p| p.parse_call_arg())?;
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

        // `true`/`false` are the two nullary constructors of `Bool`, so they
        // pattern-match through the ordinary constructor path. They are
        // keywords rather than identifiers, so they need naming here — but
        // they carry no special pattern kind, which is what lets a boolean
        // match be coverage-certified like any other sum.
        for (tok, name) in [(TokenKind::True, "true"), (TokenKind::False, "false")] {
            if self.check(&tok) {
                self.advance();
                return Ok(Pattern::Ctor {
                    name: name.to_string(),
                    args: vec![],
                });
            }
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
