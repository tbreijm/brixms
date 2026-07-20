//! Hand-written recursive-descent parser (Appendix D).
//!
//! Not a generator: every production is a method, so error recovery and
//! diagnostics are first-class. The parser is *total* — it always returns a
//! [`File`] plus a [`Diagnostics`] list, never `Err`. On an unexpected token
//! it emits a diagnostic and recovers by one of two strategies:
//!
//! * **declaration recovery** — skip tokens until the next token that can
//!   start a top-level `Decl`, then resume (`Decl::Error` marks the gap);
//! * **delimiter recovery** — inside a `{...}`/`(...)`, skip to the matching
//!   closer using a depth counter so a mistake in one clause can't eat the
//!   rest of the file.
//!
//! Newlines are significant only as clause/item separators inside blocks;
//! comments are trivia and are dropped from the working token stream (a
//! comment-preserving CST is a fmt-lane follow-up, noted in the report).

use crate::ast::*;
use crate::diag::{Diagnostic, Diagnostics};
use crate::lexer::{is_trivia, lex, TokKind, Token};
use crate::span::Span;

/// Parse a whole source file. Always returns a tree; inspect the returned
/// [`Diagnostics`] for errors.
pub const DEFAULT_MAX_PARSE_DEPTH: u16 = 512;

pub fn parse_file(src: &str) -> (File, Diagnostics) {
    parse_file_with_limit(src, DEFAULT_MAX_PARSE_DEPTH)
}

/// Parse with an explicit nesting budget. Long-lived callers such as an
/// editor/daemon can choose a tighter resource boundary for untrusted text
/// without changing ordinary grammar behavior.
pub fn parse_file_with_limit(src: &str, max_depth: u16) -> (File, Diagnostics) {
    let raw = lex(src);
    let tokens: Vec<Token> = raw.into_iter().filter(|t| !is_trivia(t.kind)).collect();
    let mut p = Parser::new(src, tokens, max_depth);
    let file = p.file();
    (file, p.diags)
}

struct Parser<'s> {
    src: &'s str,
    tokens: Vec<Token>,
    pos: usize,
    diags: Diagnostics,
    /// Guards against pathological non-advancing loops during recovery.
    fuel: u32,
    /// Active recursive grammar frames. This bounds the hand-written parser's
    /// use of the process stack on adversarial nesting.
    depth: u16,
    max_depth: u16,
    depth_reported: bool,
    /// Suppresses the `Ident { ... }` struct-literal heuristic in `postfix`
    /// while set. Scoped around parsing `if`/`match`'s condition/scrutinee:
    /// `if cond { ... }` must not read a bare-identifier `cond` immediately
    /// followed by `{` as `cond { ... }` (a struct literal), which would
    /// swallow the `then` block. Set/cleared immediately around the single
    /// `self.expr()` call for the condition (see `if_expr`/`match_expr`).
    no_struct_lit: bool,
}

/// Keywords that can begin a top-level declaration. Used as the resync set
/// for declaration-level error recovery.
const DECL_STARTS: &[&str] = &[
    "entity",
    "rel",
    "state",
    "event",
    "open",
    "derive",
    "constraint",
    "query",
    "protocol",
    "driver",
    "scenario",
    "fn",
    "partial",
    "aggregate",
    "type",
    "measure",
    "unit",
    "currency",
    "enum",
    "record",
    "data",
    "feature",
    "dataset",
    "statistical",
    "ml",
    "experiment",
    "tuning",
    "visualization",
    "trait",
    "impl",
    "phase",
    "use",
    "reimport",
    // extension keywords (outside Appendix D Decl):
    "logic",
    "system",
    "hybrid",
    "decision",
    "workflow",
    "brick",
    "export",
    "model",
    "correction",
    "interchange",
    "consistency",
    "factor",
    "ordered",
    "language",
    "transaction",
    "policy",
];

impl<'s> Parser<'s> {
    fn new(src: &'s str, tokens: Vec<Token>, max_depth: u16) -> Self {
        Parser {
            src,
            tokens,
            pos: 0,
            diags: Diagnostics::new(),
            fuel: 0,
            depth: 0,
            max_depth,
            depth_reported: false,
            no_struct_lit: false,
        }
    }

    fn enter_depth(&mut self, production: &'static str) -> bool {
        if self.depth >= self.max_depth {
            if !self.depth_reported {
                self.depth_reported = true;
                self.diags.push(Diagnostic::error(
                    "BRX-AST-0003",
                    self.cur_span(),
                    format!(
                        "maximum parser nesting depth ({}) exceeded while parsing {production}",
                        self.max_depth
                    ),
                ));
            }
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Parse `self.expr()` with the struct-literal heuristic suppressed
    /// (see `no_struct_lit`) — used for `if`/`match`'s condition/scrutinee.
    fn expr_no_struct_lit(&mut self) -> Expr {
        let prev = self.no_struct_lit;
        self.no_struct_lit = true;
        let e = self.expr();
        self.no_struct_lit = prev;
        e
    }

    // ---- token cursor -------------------------------------------------

    fn peek_kind(&self) -> Option<TokKind> {
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    /// Kind of the current token, skipping newlines (which are only
    /// meaningful to the separator logic, not to token classification).
    fn cur(&self) -> Option<TokKind> {
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if t.kind == TokKind::Newline {
                i += 1;
            } else {
                return Some(t.kind);
            }
        }
        None
    }

    fn cur_tok(&self) -> Option<Token> {
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if t.kind == TokKind::Newline {
                i += 1;
            } else {
                return Some(*t);
            }
        }
        None
    }

    /// Peek the nth non-newline token ahead (0 = current).
    fn nth(&self, n: usize) -> Option<TokKind> {
        let mut i = self.pos;
        let mut seen = 0;
        while let Some(t) = self.tokens.get(i) {
            if t.kind == TokKind::Newline {
                i += 1;
                continue;
            }
            if seen == n {
                return Some(t.kind);
            }
            seen += 1;
            i += 1;
        }
        None
    }

    fn text(&self, t: Token) -> &'s str {
        t.text(self.src)
    }

    fn cur_text(&self) -> &'s str {
        self.cur_tok().map(|t| self.text(t)).unwrap_or("")
    }

    /// Advance to the current (newline-skipped) token and return it,
    /// consuming any newlines in front of it too.
    fn bump(&mut self) -> Token {
        while let Some(t) = self.tokens.get(self.pos) {
            if t.kind == TokKind::Newline {
                self.pos += 1;
            } else {
                break;
            }
        }
        let t = self.tokens[self.pos.min(self.tokens.len().saturating_sub(1))];
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn at_eof(&self) -> bool {
        self.cur().is_none()
    }

    /// End-of-file span (for diagnostics at the very end of input).
    fn eof_span(&self) -> Span {
        let end = self.src.len() as u32;
        Span::new(end, end)
    }

    fn cur_span(&self) -> Span {
        self.cur_tok()
            .map(|t| t.span)
            .unwrap_or_else(|| self.eof_span())
    }

    // ---- matching helpers ---------------------------------------------

    fn at(&self, kind: TokKind) -> bool {
        self.cur() == Some(kind)
    }

    fn at_kw(&self, kw: &str) -> bool {
        self.cur() == Some(TokKind::Ident) && self.cur_text() == kw
    }

    fn eat(&mut self, kind: TokKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consume `kind` or emit an "expected" diagnostic (and do not advance).
    fn expect(&mut self, kind: TokKind, what: &str) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            self.error_here(format!("expected {what}"));
            None
        }
    }

    fn error_here(&mut self, msg: impl Into<String>) {
        let span = self.cur_span();
        let found = if self.at_eof() {
            "end of file".to_string()
        } else {
            format!("`{}`", self.cur_text())
        };
        self.diags.push(Diagnostic::error(
            "BRX-AST-0001",
            span,
            format!("{}, found {found}", msg.into()),
        ));
    }

    fn ident(&mut self, what: &str) -> Ident {
        if self.at(TokKind::Ident) {
            let t = self.bump();
            Ident {
                text: self.text(t).to_string(),
                span: t.span,
            }
        } else {
            self.error_here(format!("expected {what}"));
            Ident {
                text: String::new(),
                span: self.cur_span(),
            }
        }
    }

    // ---- file ---------------------------------------------------------

    fn file(&mut self) -> File {
        let start = self.cur_span();
        let package = if self.at_kw("package") {
            Some(self.package_decl())
        } else {
            None
        };
        let module = if self.at_kw("module") {
            Some(self.module_decl())
        } else {
            None
        };
        let mut uses = Vec::new();
        while self.at_kw("use") {
            uses.push(self.use_decl());
        }
        let mut reimports = Vec::new();
        while self.at_kw("reimport") {
            reimports.push(self.reimport_decl());
        }
        let mut decls = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            if let Some(d) = self.decl() {
                decls.push(d);
            }
            if self.pos == before {
                // No progress: recover to avoid an infinite loop. Capture
                // the exact source text of the one token skipped so `fmt`
                // can re-emit it verbatim (see ast::Decl::Error docs).
                let bad = self.cur_span();
                let t = self.bump();
                decls.push(Decl::Error(bad, self.text(t).to_string()));
            }
        }
        let end = self.eof_span();
        File {
            span: start.to(end),
            package,
            module,
            uses,
            reimports,
            decls,
        }
    }

    fn package_decl(&mut self) -> PackageDecl {
        let kw = self.bump(); // package
        let name = self.qual_ident();
        self.expect(TokKind::At, "`@`");
        let version = self.semver();
        PackageDecl {
            span: kw.span.to(version.span),
            name,
            version,
        }
    }

    fn semver(&mut self) -> SemVer {
        let (text, span) = self.semver_run();
        if text.is_empty() {
            self.error_here("expected version");
        }
        SemVer { span, text }
    }

    /// A verbatim, no-internal-whitespace run of Int/Ident/Dot/Minus/Plus
    /// tokens — the shared shape behind `@version` wherever it appears
    /// (package headers, `version_tag_opt`, and inline `Name@version` in
    /// loose-item position). Collected verbatim rather than parsed as a
    /// general expression because `1.0.0` would otherwise lex as the float
    /// `1.0` followed by a stray `.0` field access.
    fn semver_run(&mut self) -> (String, Span) {
        let start = self.cur_span();
        let mut end = start;
        let mut text = String::new();
        while matches!(
            self.cur(),
            Some(TokKind::Int)
                | Some(TokKind::Float)
                | Some(TokKind::Ident)
                | Some(TokKind::Dot)
                | Some(TokKind::Minus)
                | Some(TokKind::Plus)
        ) {
            // stop if there is a gap (newline) -> handled since cur skips
            // newlines; instead check adjacency by span.
            let t = self.cur_tok().unwrap();
            if !text.is_empty() && t.span.start != end.end {
                break;
            }
            self.bump();
            text.push_str(self.text(t));
            end = t.span;
        }
        (text, start.to(end))
    }

    fn module_decl(&mut self) -> ModuleDecl {
        let kw = self.bump();
        let name = self.ident("module name");
        ModuleDecl {
            span: kw.span.to(name.span),
            name,
        }
    }

    fn use_decl(&mut self) -> UseDecl {
        let kw = self.bump();
        let path = self.qual_ident();
        let mut items = Vec::new();
        let mut end = path.span;
        // `use a.b.{ x, y }`: `qual_ident` stops before the `.{`, leaving the
        // separating `.` unconsumed. Eat it so the brace import list is picked
        // up (Appendix D: `Use := "use" QualIdent ("." "{" Ident ... "}")?`).
        if self.at(TokKind::Dot) && self.nth(1) == Some(TokKind::LBrace) {
            self.bump();
        }
        if self.at(TokKind::LBrace) {
            let lb = self.bump();
            end = lb.span;
            if !self.at(TokKind::RBrace) {
                loop {
                    items.push(self.ident("imported item"));
                    if !self.eat(TokKind::Comma) {
                        break;
                    }
                    if self.at(TokKind::RBrace) {
                        break;
                    }
                }
            }
            if let Some(rb) = self.expect(TokKind::RBrace, "`}`") {
                end = rb.span;
            }
        }
        // `as Ident`: binds this use's local prefix to `alias` (design:
        // lets the same bare names from different modules coexist locally
        // instead of always claiming the path's own last segment / each
        // item's bare name).
        let alias = if self.at_kw("as") {
            self.bump();
            let id = self.ident("alias name");
            end = id.span;
            Some(id)
        } else {
            None
        };
        UseDecl {
            span: kw.span.to(end),
            path,
            items,
            alias,
        }
    }

    /// `reimport a.b` / `reimport a.b.{ x, y }` — package-entry-only
    /// re-export (Appendix D extension: `Reimport := "reimport" QualIdent
    /// ("." "{" Ident ("," Ident)* "}")?`). Shares `use`'s path/brace
    /// parsing shape; no `as` form (publishing outward has no local
    /// alias to bind).
    fn reimport_decl(&mut self) -> ReimportDecl {
        let kw = self.bump();
        let path = self.qual_ident();
        let mut items = Vec::new();
        let mut end = path.span;
        if self.at(TokKind::Dot) && self.nth(1) == Some(TokKind::LBrace) {
            self.bump();
        }
        if self.at(TokKind::LBrace) {
            let lb = self.bump();
            end = lb.span;
            if !self.at(TokKind::RBrace) {
                loop {
                    items.push(self.ident("reimported item"));
                    if !self.eat(TokKind::Comma) {
                        break;
                    }
                    if self.at(TokKind::RBrace) {
                        break;
                    }
                }
            }
            if let Some(rb) = self.expect(TokKind::RBrace, "`}`") {
                end = rb.span;
            }
        }
        ReimportDecl {
            span: kw.span.to(end),
            path,
            items,
        }
    }

    /// `a.b.c` — dotted identifiers. Stops before a `.{` (import list) so
    /// [`Self::use_decl`] can pick up the brace form.
    fn qual_ident(&mut self) -> Path {
        let first = self.ident("identifier");
        let mut segments = vec![first];
        let mut span = segments[0].span;
        while self.at(TokKind::Dot) && self.nth(1) == Some(TokKind::Ident) {
            self.bump();
            let seg = self.ident("identifier");
            span = span.to(seg.span);
            segments.push(seg);
        }
        Path { segments, span }
    }

    // ---- declarations -------------------------------------------------

    fn decl(&mut self) -> Option<Decl> {
        let kw = self.cur_text().to_string();
        match kw.as_str() {
            "entity" => Some(Decl::Entity(self.entity_decl())),
            "rel" | "state" | "event" | "open" => Some(self.rel_decl()),
            "derive" => Some(Decl::Derive(self.derive_decl())),
            "constraint" => Some(Decl::Constraint(self.constraint_decl())),
            "query" => Some(Decl::Query(self.query_decl())),
            "protocol" => Some(Decl::Protocol(self.protocol_decl())),
            "driver" => Some(Decl::Driver(self.driver_decl())),
            "scenario" => Some(Decl::Scenario(self.scenario_decl())),
            "fn" | "partial" | "aggregate" => Some(Decl::Fn(self.fn_decl())),
            "type" => Some(Decl::Type(self.type_decl())),
            "measure" => Some(Decl::Measure(self.measure_decl())),
            "unit" => Some(Decl::Unit(self.unit_decl())),
            "enum" => Some(Decl::Enum(self.enum_decl())),
            "record" => Some(Decl::Record(self.record_decl())),
            "feature" if self.nth(1) == Some(TokKind::Ident) && self.cur_text_at(1) == "set" => {
                Some(Decl::FeatureSet(self.feature_set_decl()))
            }
            "feature" => Some(Decl::Feature(self.feature_decl())),
            "data" if self.cur_text_at(1) == "recipe" => {
                Some(Decl::DataRecipe(self.data_recipe_decl()))
            }
            "dataset" => Some(Decl::Dataset(self.dataset_decl())),
            "statistical" if self.cur_text_at(1) == "model" => {
                Some(Decl::StatModel(self.stat_model_decl()))
            }
            "ml" if self.cur_text_at(1) == "workflow" => {
                Some(Decl::MlWorkflow(self.ml_workflow_decl()))
            }
            "experiment" | "tuning" => Some(Decl::Experiment(self.experiment_decl())),
            "visualization" => Some(Decl::Visualization(self.visualization_decl())),
            "let" => Some(Decl::Let(self.let_binding_decl())),
            "" => None,
            _ => Some(Decl::Extension(self.extension_decl())),
        }
    }

    fn cur_text_at(&self, n: usize) -> &'s str {
        // text of the nth non-newline token
        let mut i = self.pos;
        let mut seen = 0;
        while let Some(t) = self.tokens.get(i) {
            if t.kind == TokKind::Newline {
                i += 1;
                continue;
            }
            if seen == n {
                return self.text(*t);
            }
            seen += 1;
            i += 1;
        }
        ""
    }

    fn entity_decl(&mut self) -> EntityDecl {
        let kw = self.bump();
        let name = self.ident("entity name");
        let (fields, end) = self.field_block();
        EntityDecl {
            span: kw.span.to(end),
            name,
            fields,
        }
    }

    /// `{ FieldDecl (sep FieldDecl)* }` where a field is `key? Ident : Type`.
    /// Separator is a newline, `;`, or `,` (enum variant struct payloads —
    /// see `enum_decl` — use comma-separated fields on one line). Tolerates
    /// the spec's placeholder `{ ... }` (fixture 0003) by treating `...` as
    /// an empty body.
    fn field_block(&mut self) -> (Vec<FieldDecl>, Span) {
        let mut fields = Vec::new();
        let lb = match self.expect(TokKind::LBrace, "`{`") {
            Some(t) => t,
            None => return (fields, self.cur_span()),
        };
        let mut end = lb.span;
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            if self.at(TokKind::DotDotDot) {
                self.bump();
                continue;
            }
            let before = self.pos;
            let is_key = self.eat_kw("key");
            let name = self.ident("field name");
            self.expect(TokKind::Colon, "`:`");
            let ty = self.type_();
            fields.push(FieldDecl {
                span: name.span.to(ty.span),
                is_key,
                name,
                ty,
            });
            self.eat(TokKind::Comma);
            if self.pos == before {
                self.recover_in_braces();
                break;
            }
        }
        if let Some(rb) = self.expect(TokKind::RBrace, "`}`") {
            end = rb.span;
        }
        (fields, end)
    }

    fn rel_decl(&mut self) -> Decl {
        let start = self.cur_span();
        let kind = match self.cur_text() {
            "state" => {
                self.bump();
                RelKind::State
            }
            "event" => {
                self.bump();
                RelKind::Event
            }
            "open" => {
                self.bump();
                RelKind::Open
            }
            _ => RelKind::Ground,
        };
        // now expect `rel`
        if !self.eat_kw("rel") {
            self.error_here("expected `rel`");
        }
        let name = self.ident("relation name");
        let (roles, mut end) = self.field_block();
        let mut mods = Vec::new();
        loop {
            match self.cur_text() {
                "key" if self.nth(1) == Some(TokKind::LParen) => {
                    let (ids, e) = self.kw_ident_list("key");
                    mods.push(RelMod::Key(ids));
                    end = e;
                }
                "unique" => {
                    let (ids, e) = self.kw_ident_list("unique");
                    mods.push(RelMod::Unique(ids));
                    end = e;
                }
                "index" => {
                    let (ids, e) = self.kw_ident_list("index");
                    mods.push(RelMod::Index(ids));
                    end = e;
                }
                "partition" => {
                    let (ids, e) = self.kw_ident_list("partition");
                    mods.push(RelMod::Partition(ids));
                    end = e;
                }
                "time" => {
                    self.bump();
                    self.expect(TokKind::LParen, "`(`");
                    let id = self.ident("time role");
                    let rp = self.expect(TokKind::RParen, "`)`");
                    end = rp.map(|t| t.span).unwrap_or(id.span);
                    mods.push(RelMod::Time(id));
                }
                _ => break,
            }
        }
        Decl::Rel(RelDecl {
            span: start.to(end),
            kind,
            name,
            roles,
            mods,
        })
    }

    fn kw_ident_list(&mut self, kw: &str) -> (Vec<Ident>, Span) {
        self.eat_kw(kw);
        self.expect(TokKind::LParen, "`(`");
        let mut ids = Vec::new();
        if !self.at(TokKind::RParen) {
            loop {
                // Tolerate the spec's placeholder `key(...)` (fixture 0003)
                // the same way `field_block`/`block`/`tx_block` tolerate
                // `...` as an empty body: drop it rather than erroring on a
                // non-identifier token, which would otherwise leak `...`
                // and the following `)` out to the next declaration and
                // break fmt idempotence.
                if self.at(TokKind::DotDotDot) {
                    self.bump();
                } else {
                    ids.push(self.ident("identifier"));
                }
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        let end = self
            .expect(TokKind::RParen, "`)`")
            .map(|t| t.span)
            .unwrap_or_else(|| self.cur_span());
        (ids, end)
    }

    fn derive_decl(&mut self) -> DeriveDecl {
        let kw = self.bump();
        let name = self.ident("rule name");
        self.expect(TokKind::Colon, "`:`");
        let head = self.head();
        if !self.eat_kw("from") {
            self.error_here("expected `from`");
        }
        let body = self.block();
        DeriveDecl {
            span: kw.span.to(body.span),
            name,
            head,
            body,
        }
    }

    fn head(&mut self) -> Head {
        // mask ( Ident ) by Ident
        if self.at_kw("mask") {
            self.bump();
            self.expect(TokKind::LParen, "`(`");
            let target = self.ident("mask target");
            self.expect(TokKind::RParen, "`)`");
            if !self.eat_kw("by") {
                self.error_here("expected `by`");
            }
            let by = self.ident("mask reason");
            return Head::Mask { target, by };
        }
        // NodeHead: Ident : Ident { ArgList } keyed by (IdentList)
        // TupleHead: QualIdent ( ArgList )
        // Disambiguate: an Ident directly followed by `:` is a NodeHead.
        if self.at(TokKind::Ident) && self.nth(1) == Some(TokKind::Colon) {
            let binder = self.ident("binder");
            self.bump(); // :
            let ty = self.ident("type");
            let args = if self.at(TokKind::LBrace) {
                self.arg_braces()
            } else {
                Vec::new()
            };
            if self.eat_kw("keyed") {
                self.eat_kw("by");
                let (keyed_by, _) = self.paren_ident_list();
                return Head::Node {
                    binder,
                    ty,
                    args,
                    keyed_by,
                };
            }
            return Head::Node {
                binder,
                ty,
                args,
                keyed_by: Vec::new(),
            };
        }
        let path = self.qual_ident();
        let args = if self.at(TokKind::LParen) {
            self.arg_parens()
        } else {
            Vec::new()
        };
        Head::Tuple { path, args }
    }

    fn paren_ident_list(&mut self) -> (Vec<Ident>, Span) {
        self.expect(TokKind::LParen, "`(`");
        let mut ids = Vec::new();
        if !self.at(TokKind::RParen) {
            loop {
                // See `kw_ident_list`: tolerate a `...` placeholder entry.
                if self.at(TokKind::DotDotDot) {
                    self.bump();
                } else {
                    ids.push(self.ident("identifier"));
                }
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        let end = self
            .expect(TokKind::RParen, "`)`")
            .map(|t| t.span)
            .unwrap_or_else(|| self.cur_span());
        (ids, end)
    }

    // ---- pattern block / clauses --------------------------------------

    fn block(&mut self) -> Block {
        if !self.enter_depth("a pattern block") {
            return Block {
                span: self.cur_span(),
                clauses: Vec::new(),
            };
        }
        let mut clauses = Vec::new();
        let lb = match self.expect(TokKind::LBrace, "`{`") {
            Some(t) => t,
            None => {
                let block = Block {
                    span: self.cur_span(),
                    clauses,
                };
                self.leave_depth();
                return block;
            }
        };
        let mut end = lb.span;
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            if self.at(TokKind::DotDotDot) {
                self.bump();
                continue;
            }
            let before = self.pos;
            let clause = self.clause();
            clauses.push(clause);
            if self.pos == before {
                let bad = self.cur_span();
                let t = self.bump();
                clauses.push(Clause::Error(bad, self.text(t).to_string()));
            }
        }
        if let Some(rb) = self.expect(TokKind::RBrace, "`}`") {
            end = rb.span;
        }
        let block = Block {
            span: lb.span.to(end),
            clauses,
        };
        self.leave_depth();
        block
    }

    fn clause(&mut self) -> Clause {
        match self.cur_text() {
            "let" => Clause::Let(self.let_clause()),
            "when" => {
                let kw = self.bump();
                let e = self.expr();
                let _ = kw;
                Clause::When(e)
            }
            "any" => {
                self.bump();
                self.expect(TokKind::LBrace, "`{`");
                let mut cases = Vec::new();
                loop {
                    self.skip_separators();
                    if self.at(TokKind::RBrace) || self.at_eof() {
                        break;
                    }
                    if self.eat_kw("case") {
                        cases.push(self.block());
                    } else {
                        break;
                    }
                }
                self.expect(TokKind::RBrace, "`}`");
                Clause::Any(cases)
            }
            "exists" => {
                self.bump();
                Clause::Exists(self.block())
            }
            "without" => {
                self.bump();
                Clause::Without(self.block())
            }
            "optional" => {
                self.bump();
                Clause::Optional(self.block())
            }
            "cross" => {
                self.bump();
                Clause::Cross(self.block())
            }
            "history" => {
                self.bump();
                Clause::History(self.edge_clause())
            }
            "path" => Clause::Path(self.path_clause()),
            _ => {
                // EntityClause: Ident : Ident { ... }
                // EdgeClause:   (Ident @)? QualIdent ( ArgList )
                if self.at(TokKind::Ident) && self.nth(1) == Some(TokKind::Colon) {
                    return Clause::Entity(self.entity_clause());
                }
                Clause::Edge(self.edge_clause())
            }
        }
    }

    fn let_clause(&mut self) -> LetClause {
        let kw = self.bump();
        let pattern = self.expr();
        self.expect(TokKind::Eq, "`=`");
        let value = self.expr();
        LetClause {
            span: kw.span.to(value.span),
            pattern,
            value,
        }
    }

    fn entity_clause(&mut self) -> EntityClause {
        let binder = self.ident("binder");
        self.expect(TokKind::Colon, "`:`");
        let ty = self.ident("type");
        let fields = self.arg_braces();
        let end = fields.last().map(|a| a.span).unwrap_or(ty.span);
        EntityClause {
            span: binder.span.to(end),
            binder,
            ty,
            fields,
        }
    }

    fn edge_clause(&mut self) -> EdgeClause {
        let start = self.cur_span();
        let alias = if self.at(TokKind::Ident) && self.nth(1) == Some(TokKind::At) {
            let id = self.ident("alias");
            self.bump(); // @
            Some(id)
        } else {
            None
        };
        let path = self.qual_ident();
        let args = if self.at(TokKind::LParen) {
            self.arg_parens()
        } else {
            Vec::new()
        };
        EdgeClause {
            span: start.to(self.prev_span()),
            alias,
            path,
            args,
        }
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            return Span::empty(0);
        }
        // last consumed non-newline token
        let mut i = self.pos;
        while i > 0 {
            i -= 1;
            if self.tokens[i].kind != TokKind::Newline {
                return self.tokens[i].span;
            }
        }
        Span::empty(0)
    }

    fn path_clause(&mut self) -> PathClause {
        let kw = self.bump(); // path
        let expr = self.path_expr();
        if !self.eat_kw("from") {
            self.error_here("expected `from`");
        }
        let from = self.ident("source node");
        if !self.eat_kw("to") {
            self.error_here("expected `to`");
        }
        let to = self.ident("target node");
        PathClause {
            span: kw.span.to(to.span),
            expr,
            from,
            to,
        }
    }

    fn path_expr(&mut self) -> PathExpr {
        // alt := seq ( | seq )*
        let mut alts = vec![self.path_repeat()];
        while self.at(TokKind::Pipe) {
            self.bump();
            alts.push(self.path_repeat());
        }
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            PathExpr::Alt(alts)
        }
    }

    fn path_repeat(&mut self) -> PathExpr {
        let inner = if self.at(TokKind::LParen) {
            self.bump();
            let e = self.path_expr();
            self.expect(TokKind::RParen, "`)`");
            PathExpr::Group(Box::new(e))
        } else {
            PathExpr::Step(self.path_step())
        };
        if let Some(rep) = self.repeat_opt() {
            PathExpr::Repeat(Box::new(inner), rep)
        } else {
            inner
        }
    }

    fn repeat_opt(&mut self) -> Option<Repeat> {
        match self.cur() {
            Some(TokKind::Plus) => {
                self.bump();
                Some(Repeat::Plus)
            }
            Some(TokKind::Star) => {
                self.bump();
                Some(Repeat::Star)
            }
            Some(TokKind::LBrace) => {
                self.bump();
                let lo = self.nat();
                let hi = if self.eat(TokKind::Comma) {
                    Some(self.nat())
                } else {
                    None
                };
                self.expect(TokKind::RBrace, "`}`");
                Some(Repeat::Range(lo, hi))
            }
            _ => None,
        }
    }

    fn nat(&mut self) -> u64 {
        if self.at(TokKind::Int) {
            let t = self.bump();
            self.text(t).replace('_', "").parse().unwrap_or(0)
        } else {
            self.error_here("expected a number");
            0
        }
    }

    fn path_step(&mut self) -> PathStep {
        let path = self.qual_ident();
        self.expect(TokKind::LParen, "`(`");
        let from = self.ident("incidence role");
        self.expect(TokKind::Arrow, "`->`");
        let to = self.ident("incidence role");
        let end = self
            .expect(TokKind::RParen, "`)`")
            .map(|t| t.span)
            .unwrap_or(to.span);
        PathStep {
            span: path.span.to(end),
            path,
            from,
            to,
        }
    }

    // ---- constraints / queries ----------------------------------------

    fn constraint_decl(&mut self) -> ConstraintDecl {
        let kw = self.bump();
        let name = self.ident("constraint name");
        let kind = match self.cur_text() {
            "advisory" => {
                self.bump();
                ConstraintKind::Advisory
            }
            "strict" => {
                self.bump();
                ConstraintKind::Strict
            }
            "audit" => {
                self.bump();
                ConstraintKind::Audit
            }
            _ => {
                self.error_here("expected `advisory`, `strict`, or `audit`");
                ConstraintKind::Advisory
            }
        };
        let body = self.block();
        ConstraintDecl {
            span: kw.span.to(body.span),
            name,
            kind,
            body,
        }
    }

    fn query_decl(&mut self) -> QueryDecl {
        let kw = self.bump();
        let name = self.ident("query name");
        let params = self.param_list();
        self.expect(TokKind::Arrow, "`->`");
        let ret = self.type_();
        self.expect(TokKind::Eq, "`=`");
        if !self.eat_kw("from") {
            self.error_here("expected `from`");
        }
        let from = self.block();
        if !self.eat_kw("yield") {
            self.error_here("expected `yield`");
        }
        let yield_ = self.expr();
        let order = if self.at_kw("order") {
            Some(self.order_clause())
        } else {
            None
        };
        let end = order.as_ref().map(|o| o.span).unwrap_or(yield_.span);
        QueryDecl {
            span: kw.span.to(end),
            name,
            params,
            ret,
            from,
            yield_,
            order,
        }
    }

    fn order_clause(&mut self) -> OrderClause {
        let kw = self.bump(); // order
        self.eat_kw("by");
        let mut by = vec![self.expr()];
        while self.eat(TokKind::Comma) {
            by.push(self.expr());
        }
        let limit = if self.eat_kw("limit") {
            Some(self.expr())
        } else {
            None
        };
        let end = limit
            .as_ref()
            .map(|e| e.span)
            .or_else(|| by.last().map(|e| e.span))
            .unwrap_or(kw.span);
        OrderClause {
            span: kw.span.to(end),
            by,
            limit,
        }
    }

    fn param_list(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if !self.eat(TokKind::LParen) {
            return params;
        }
        if !self.at(TokKind::RParen) {
            loop {
                let name = self.ident("parameter name");
                self.expect(TokKind::Colon, "`:`");
                let ty = self.type_();
                params.push(Param {
                    span: name.span.to(ty.span),
                    name,
                    ty,
                });
                if !self.eat(TokKind::Comma) {
                    break;
                }
                if self.at(TokKind::RParen) {
                    break;
                }
            }
        }
        self.expect(TokKind::RParen, "`)`");
        params
    }

    // ---- protocol / driver / scenario ---------------------------------

    fn protocol_decl(&mut self) -> ProtocolDecl {
        let kw = self.bump();
        let name = self.ident("protocol name");
        let generics = self.generics_opt();
        let lb = self.expect(TokKind::LBrace, "`{`");
        let mut request = RequestDecl {
            span: name.span,
            roles: Vec::new(),
            key: Vec::new(),
        };
        let mut outcomes = Vec::new();
        let mut policy = None;
        let mut methods = Vec::new();
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            match self.cur_text() {
                "request" => {
                    let kwr = self.bump();
                    let (roles, _) = self.field_block();
                    let mut key = Vec::new();
                    if self.at_kw("key") {
                        let (ids, _) = self.kw_ident_list("key");
                        key = ids;
                    }
                    request = RequestDecl {
                        span: kwr.span.to(self.prev_span()),
                        roles,
                        key,
                    };
                }
                "outcome" => {
                    let kwo = self.bump();
                    let oname = self.ident("outcome name");
                    let (roles, end) = self.field_block();
                    outcomes.push(OutcomeDecl {
                        span: kwo.span.to(end),
                        name: oname,
                        roles,
                    });
                }
                "policy" => {
                    policy = Some(self.loose_block_kw());
                }
                "reconcile" | "compensate" | "retry" | "satisfies" => {
                    // Part 26.7 lifecycle clauses (not in Appendix D
                    // ProtocolDecl) — capture as a loose item so the tree
                    // stays faithful. See errata.
                    let _ = self.loose_item();
                }
                // Estimator-style method sig: Ident ( ... ) -> Type
                _ if self.at(TokKind::Ident) && self.nth(1) == Some(TokKind::LParen) => {
                    methods.push(self.fn_sig());
                }
                _ => {
                    self.error_here("expected `request`, `outcome`, or `policy`");
                    self.recover_in_braces();
                    break;
                }
            }
            if self.pos == before {
                self.recover_in_braces();
                break;
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .or(lb.map(|t| t.span))
            .unwrap_or(name.span);
        ProtocolDecl {
            span: kw.span.to(end),
            name,
            generics,
            request,
            outcomes,
            policy,
            methods,
        }
    }

    fn fn_sig(&mut self) -> FnSig {
        let name = self.ident("method name");
        let params = self.param_list();
        let ret = if self.eat(TokKind::Arrow) {
            Some(self.type_())
        } else {
            None
        };
        let end = ret.as_ref().map(|t| t.span).unwrap_or(name.span);
        FnSig {
            span: name.span.to(end),
            name,
            params,
            ret,
        }
    }

    fn driver_decl(&mut self) -> DriverDecl {
        let kw = self.bump();
        let name = self.ident("driver name");
        if !self.eat_kw("for") {
            self.error_here("expected `for`");
        }
        let for_protocol = self.ident("protocol name");
        let mut needs = Vec::new();
        if self.eat_kw("needs") {
            loop {
                needs.push(self.cap_ref());
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokKind::LBrace, "`{`");
        // on request ( Ident , Ident ) FnBlock
        self.eat_kw("on");
        self.eat_kw("request");
        self.expect(TokKind::LParen, "`(`");
        let req_param = self.ident("request param");
        self.expect(TokKind::Comma, "`,`");
        let cancel_param = self.ident("cancel param");
        self.expect(TokKind::RParen, "`)`");
        let body = self.fn_block();
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(body.span);
        DriverDecl {
            span: kw.span.to(end),
            name,
            for_protocol,
            needs,
            req_param,
            cancel_param,
            body,
        }
    }

    fn cap_ref(&mut self) -> CapRef {
        let name = self.ident("capability");
        let mut args = Vec::new();
        let mut end = name.span;
        if self.at(TokKind::Lt) {
            let (a, e) = self.type_args();
            args = a;
            end = e;
        }
        CapRef {
            span: name.span.to(end),
            name,
            args,
        }
    }

    fn scenario_decl(&mut self) -> ScenarioDecl {
        let kw = self.bump();
        let name = self.ident("scenario name");
        self.expect(TokKind::LBrace, "`{`");
        let mut seed: Option<SeedDecl> = None;
        let mut binds = Vec::new();
        let mut setup = None;
        let mut steps = Vec::new();
        let mut ats = Vec::new();
        let mut asserts = Vec::new();
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            match self.cur_text() {
                "seed" => {
                    if seed.is_some() {
                        self.error_here("duplicate `seed` declaration in scenario");
                    }
                    let s = self.bump();
                    seed = Some(if self.eat_kw("each") {
                        let e = self.expr();
                        SeedDecl::Each(e, s.span)
                    } else {
                        let n = self.nat();
                        SeedDecl::Nat(n, s.span)
                    });
                }
                "bind" => binds.push(self.bind_decl()),
                "setup" => {
                    self.bump();
                    setup = Some(self.tx_block());
                }
                "step" => steps.push(self.step_decl()),
                "at" => ats.push(self.at_decl()),
                "assert" => asserts.push(self.assert_decl()),
                _ => {
                    self.error_here("expected a scenario item");
                    self.recover_in_braces();
                    break;
                }
            }
            if self.pos == before {
                self.recover_in_braces();
                break;
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(name.span);
        let seed = match seed {
            Some(seed) => seed,
            None => {
                self.error_here("scenario requires an explicit `seed` declaration");
                SeedDecl::Nat(0, name.span)
            }
        };
        ScenarioDecl {
            span: kw.span.to(end),
            name,
            seed,
            binds,
            setup,
            steps,
            ats,
            asserts,
        }
    }

    fn bind_decl(&mut self) -> BindDecl {
        let kw = self.bump();
        let protocol = self.qual_ident();
        let args = if self.at(TokKind::LParen) {
            self.arg_parens()
        } else {
            Vec::new()
        };
        let to = if self.eat_kw("to") {
            Some(self.expr())
        } else {
            None
        };
        BindDecl {
            span: kw.span.to(self.prev_span()),
            protocol,
            args,
            to,
        }
    }

    fn step_decl(&mut self) -> StepDecl {
        let kw = self.bump();
        self.eat_kw("every");
        let every = self.expr();
        self.eat_kw("for");
        let for_ = self.expr();
        let body = self.tx_block();
        StepDecl {
            span: kw.span.to(body.span),
            every,
            for_,
            body,
        }
    }

    fn at_decl(&mut self) -> AtDecl {
        let kw = self.bump();
        let at = self.expr();
        let body = self.tx_block();
        AtDecl {
            span: kw.span.to(body.span),
            at,
            body,
        }
    }

    fn assert_decl(&mut self) -> AssertDecl {
        let kw = self.bump();
        let mode = match self.cur_text() {
            "always" => {
                self.bump();
                AssertMode::Always
            }
            "eventually" => {
                self.bump();
                AssertMode::Eventually
            }
            "at" => {
                self.bump();
                self.eat_kw("end");
                AssertMode::AtEnd
            }
            _ => {
                self.error_here("expected `always`, `eventually`, or `at end`");
                AssertMode::Always
            }
        };
        self.expect(TokKind::LBrace, "`{`");
        let cond = self.expr();
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(cond.span);
        AssertDecl {
            span: kw.span.to(end),
            mode,
            cond,
        }
    }

    // ---- transactions -------------------------------------------------

    fn tx_block(&mut self) -> TxBlock {
        let mut stmts = Vec::new();
        let lb = match self.expect(TokKind::LBrace, "`{`") {
            Some(t) => t,
            None => {
                return TxBlock {
                    span: self.cur_span(),
                    stmts,
                }
            }
        };
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            if self.at(TokKind::DotDotDot) {
                self.bump();
                continue;
            }
            let before = self.pos;
            let stmt = self.tx_stmt();
            stmts.push(stmt);
            if self.pos == before {
                let bad = self.cur_span();
                let t = self.bump();
                stmts.push(TxStmt::Error(bad, self.text(t).to_string()));
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(lb.span);
        TxBlock {
            span: lb.span.to(end),
            stmts,
        }
    }

    fn tx_stmt(&mut self) -> TxStmt {
        if self.at_kw("let") {
            self.bump();
            let pattern = self.expr();
            // optional `: Type` annotation on the binding
            if self.eat(TokKind::Colon) {
                let _ = self.type_();
            }
            self.expect(TokKind::Eq, "`=`");
            let value = self.tx_expr();
            TxStmt::Let { pattern, value }
        } else {
            TxStmt::Expr(self.tx_expr())
        }
    }

    fn tx_expr(&mut self) -> TxExpr {
        let start = self.cur_span();
        match self.cur_text() {
            "ensure" => {
                self.bump();
                let ty = self.ident("entity type");
                let args = self.arg_braces();
                TxExpr::Ensure {
                    ty,
                    args,
                    span: start.to(self.prev_span()),
                }
            }
            "fresh" => {
                self.bump();
                let ty = self.ident("entity type");
                let args = self.arg_braces();
                TxExpr::Fresh {
                    ty,
                    args,
                    span: start.to(self.prev_span()),
                }
            }
            "assert" => {
                self.bump();
                // QualIdent ( ArgList )  |  Ident { ArgList }
                let path = self.qual_ident();
                if self.at(TokKind::LBrace) {
                    let args = self.arg_braces();
                    let ty = path.segments.last().cloned().unwrap_or(Ident {
                        text: String::new(),
                        span: path.span,
                    });
                    TxExpr::AssertStruct {
                        ty,
                        args,
                        span: start.to(self.prev_span()),
                    }
                } else {
                    let args = if self.at(TokKind::LParen) {
                        self.arg_parens()
                    } else {
                        Vec::new()
                    };
                    TxExpr::AssertTuple {
                        path,
                        args,
                        span: start.to(self.prev_span()),
                    }
                }
            }
            "set" => {
                self.bump();
                let path = self.qual_ident();
                let args = if self.at(TokKind::LParen) {
                    self.arg_parens()
                } else {
                    Vec::new()
                };
                TxExpr::Set {
                    path,
                    args,
                    span: start.to(self.prev_span()),
                }
            }
            "retract" => {
                self.bump();
                let expr = self.expr();
                TxExpr::Retract {
                    span: start.to(expr.span),
                    expr,
                }
            }
            "supersede" => {
                self.bump();
                let new = self.expr();
                self.eat_kw("over");
                let old = self.expr();
                TxExpr::Supersede {
                    span: start.to(old.span),
                    new,
                    old,
                }
            }
            _ => {
                // Unknown tx form: parse as a bare expression wrapped in a
                // Retract-less structure is wrong; emit an error and consume
                // one expression so we make progress.
                self.error_here("expected a transaction statement");
                let expr = self.expr();
                TxExpr::Retract {
                    span: start.to(expr.span),
                    expr,
                }
            }
        }
    }

    // ---- fn / type / enum / record ------------------------------------

    fn fn_decl(&mut self) -> FnDecl {
        let start = self.cur_span();
        let partial = self.eat_kw("partial");
        let aggregate = self.eat_kw("aggregate");
        self.eat_kw("fn");
        let name = self.ident("function name");
        let generics = self.generics_opt();
        let params = self.param_list();
        self.expect(TokKind::Arrow, "`->`");
        let ret = self.type_();
        let effects = if self.at(TokKind::Bang) {
            Some(self.effect_row())
        } else {
            None
        };
        let body = if self.eat(TokKind::Eq) {
            Some(FnBody::Expr(self.expr()))
        } else if self.at(TokKind::LBrace) {
            Some(FnBody::Block(self.fn_block()))
        } else {
            None
        };
        let end = match &body {
            Some(FnBody::Expr(e)) => e.span,
            Some(FnBody::Block(b)) => b.span,
            None => ret.span,
        };
        FnDecl {
            span: start.to(end),
            partial,
            aggregate,
            name,
            generics,
            params,
            ret,
            effects,
            body,
        }
    }

    fn effect_row(&mut self) -> Vec<Ident> {
        self.eat(TokKind::Bang);
        self.expect(TokKind::LBrace, "`{`");
        let mut effs = Vec::new();
        if !self.at(TokKind::RBrace) {
            loop {
                effs.push(self.ident("effect"));
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokKind::RBrace, "`}`");
        effs
    }

    fn fn_block(&mut self) -> FnBlock {
        let mut stmts = Vec::new();
        let lb = match self.expect(TokKind::LBrace, "`{`") {
            Some(t) => t,
            None => {
                return FnBlock {
                    span: self.cur_span(),
                    stmts,
                }
            }
        };
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            if self.at_kw("let") {
                let kw = self.bump();
                let pattern = self.expr();
                if self.eat(TokKind::Colon) {
                    let _ = self.type_();
                }
                self.expect(TokKind::Eq, "`=`");
                let value = self.expr();
                stmts.push(Stmt::Let {
                    span: kw.span.to(value.span),
                    pattern,
                    value,
                });
            } else {
                let e = self.expr();
                stmts.push(Stmt::Expr(e));
            }
            if self.pos == before {
                let bad = self.cur_span();
                let t = self.bump();
                stmts.push(Stmt::Error(bad, self.text(t).to_string()));
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(lb.span);
        FnBlock {
            span: lb.span.to(end),
            stmts,
        }
    }

    fn type_decl(&mut self) -> TypeDecl {
        let kw = self.bump();
        let name = self.ident("type name");
        let generics = self.generics_opt();
        self.expect(TokKind::Eq, "`=`");
        let value = self.type_();
        TypeDecl {
            span: kw.span.to(value.span),
            name,
            generics,
            value,
        }
    }

    fn measure_decl(&mut self) -> MeasureDecl {
        let kw = self.bump();
        let name = self.ident("measure name");
        MeasureDecl {
            span: kw.span.to(name.span),
            name,
        }
    }

    fn unit_decl(&mut self) -> UnitDecl {
        let kw = self.bump();
        let name = self.ident("unit name");
        self.expect(TokKind::Colon, "`:`");
        let measure = self.ident("measure");
        self.expect(TokKind::Eq, "`=`");
        let value = self.expr();
        UnitDecl {
            span: kw.span.to(value.span),
            name,
            measure,
            value,
        }
    }

    fn enum_decl(&mut self) -> EnumDecl {
        let kw = self.bump();
        let name = self.ident("enum name");
        let generics = self.generics_opt();
        let mut variants = Vec::new();
        self.expect(TokKind::LBrace, "`{`");
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            let vname = self.ident("variant name");
            let payload = if self.at(TokKind::LBrace) {
                let (fields, _) = self.field_block();
                VariantPayload::Struct(fields)
            } else if self.at(TokKind::LParen) {
                self.bump();
                let mut tys = Vec::new();
                if !self.at(TokKind::RParen) {
                    loop {
                        tys.push(self.type_());
                        if !self.eat(TokKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(TokKind::RParen, "`)`");
                VariantPayload::Tuple(tys)
            } else {
                VariantPayload::Unit
            };
            variants.push(EnumVariant {
                span: vname.span.to(self.prev_span()),
                name: vname,
                payload,
            });
            if self.pos == before {
                self.recover_in_braces();
                break;
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(name.span);
        EnumDecl {
            span: kw.span.to(end),
            name,
            generics,
            variants,
        }
    }

    fn record_decl(&mut self) -> RecordDecl {
        let kw = self.bump();
        let name = self.ident("record name");
        let generics = self.generics_opt();
        let (fields, end) = self.field_block();
        RecordDecl {
            span: kw.span.to(end),
            name,
            generics,
            fields,
        }
    }

    // ---- data-science decls (mostly LooseBlock) -----------------------

    fn data_recipe_decl(&mut self) -> DataRecipeDecl {
        let kw = self.bump(); // data
        self.eat_kw("recipe");
        let name = self.ident("recipe name");
        let mut items = Vec::new();
        self.expect(TokKind::LBrace, "`{`");
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            match self.cur_text() {
                "input" => {
                    self.bump();
                    items.push(RecipeItem::Input(self.type_()));
                }
                "output" => {
                    self.bump();
                    items.push(RecipeItem::Output(self.type_()));
                }
                "quarantine" => {
                    self.bump();
                    items.push(RecipeItem::Quarantine(self.expr()));
                }
                "step" => {
                    self.bump();
                    let sname = self.ident("step name");
                    let rest = self.loose_item();
                    items.push(RecipeItem::Step { name: sname, rest });
                }
                _ => {
                    self.error_here("expected a recipe item");
                    self.recover_in_braces();
                    break;
                }
            }
            if self.pos == before {
                self.recover_in_braces();
                break;
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(name.span);
        DataRecipeDecl {
            span: kw.span.to(end),
            name,
            items,
        }
    }

    fn feature_decl(&mut self) -> FeatureDecl {
        let kw = self.bump();
        let name = self.ident("feature name");
        let params = self.param_list();
        self.expect(TokKind::Arrow, "`->`");
        let ret = self.type_();
        let body = if self.eat(TokKind::Eq) {
            FeatureBody::Expr(self.expr())
        } else {
            FeatureBody::Items(self.feature_items())
        };
        let end = match &body {
            FeatureBody::Expr(e) => e.span,
            FeatureBody::Items(_) => self.prev_span(),
        };
        FeatureDecl {
            span: kw.span.to(end),
            name,
            params,
            ret,
            body,
        }
    }

    fn feature_items(&mut self) -> Vec<FeatureItem> {
        let mut items = Vec::new();
        self.expect(TokKind::LBrace, "`{`");
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            match self.cur_text() {
                "observationTime" => {
                    self.bump();
                    items.push(FeatureItem::ObservationTime(self.expr()));
                }
                "window" => {
                    self.bump();
                    items.push(FeatureItem::Window(self.expr()));
                }
                "source" => {
                    self.bump();
                    items.push(FeatureItem::Source(self.qual_ident()));
                }
                "leakage" => {
                    self.bump();
                    items.push(FeatureItem::Leakage(self.ident("leakage mode")));
                }
                "missing" => {
                    self.bump();
                    items.push(FeatureItem::Missing(self.ident("missing mode")));
                }
                _ => {
                    self.error_here("expected a feature item");
                    self.recover_in_braces();
                    break;
                }
            }
            if self.pos == before {
                self.recover_in_braces();
                break;
            }
        }
        self.expect(TokKind::RBrace, "`}`");
        items
    }

    fn feature_set_decl(&mut self) -> FeatureSetDecl {
        let kw = self.bump(); // feature
        self.eat_kw("set");
        let name = self.ident("feature set name");
        let version = self.version_tag_opt();
        let items = self.loose_block();
        FeatureSetDecl {
            span: kw.span.to(items.span),
            name,
            version,
            items,
        }
    }

    fn dataset_decl(&mut self) -> DatasetDecl {
        let kw = self.bump();
        let name = self.ident("dataset name");
        let items = self.loose_block();
        DatasetDecl {
            span: kw.span.to(items.span),
            name,
            items,
        }
    }

    fn stat_model_decl(&mut self) -> StatModelDecl {
        let kw = self.bump(); // statistical
        self.eat_kw("model");
        let name = self.ident("model name");
        let items = self.loose_block();
        StatModelDecl {
            span: kw.span.to(items.span),
            name,
            items,
        }
    }

    fn ml_workflow_decl(&mut self) -> MlWorkflowDecl {
        let kw = self.bump(); // ml
        self.eat_kw("workflow");
        let name = self.ident("workflow name");
        let items = self.loose_block();
        MlWorkflowDecl {
            span: kw.span.to(items.span),
            name,
            items,
        }
    }

    fn experiment_decl(&mut self) -> ExperimentDecl {
        let kind = if self.cur_text() == "tuning" {
            ExperimentKind::Tuning
        } else {
            ExperimentKind::Experiment
        };
        let kw = self.bump();
        let name = self.ident("experiment name");
        let items = self.loose_block();
        ExperimentDecl {
            span: kw.span.to(items.span),
            kind,
            name,
            items,
        }
    }

    fn visualization_decl(&mut self) -> VisualizationDecl {
        let kw = self.bump();
        let name = self.ident("visualization name");
        let items = self.loose_block();
        VisualizationDecl {
            span: kw.span.to(items.span),
            name,
            items,
        }
    }

    /// Top-level `let name (: Type)? = expr` (see `ast::Decl::Let` docs —
    /// not in Appendix D's `Decl` alternation, but used by the spec's own
    /// prose at program scope).
    fn let_binding_decl(&mut self) -> LetBindingDecl {
        let kw = self.bump(); // let
        let name = self.ident("binding name");
        let ty = if self.eat(TokKind::Colon) {
            Some(self.type_())
        } else {
            None
        };
        if !self.eat(TokKind::Eq) {
            self.error_here("expected `=`");
        }
        let value = self.expr();
        LetBindingDecl {
            span: kw.span.to(value.span),
            name,
            ty,
            value,
        }
    }

    fn version_tag_opt(&mut self) -> Option<Expr> {
        if !self.at(TokKind::At) {
            return None;
        }
        self.bump();
        // A version tag is a semver-shaped run (`1.0.0`, `3`), not a general
        // expression — parsing it with `expr()` would read `1.0.0` as the
        // float `1.0` followed by a stray `.0` field access. Collect the
        // adjacent run verbatim (see `semver_run`) and carry it as an
        // ident-path expr so the formatter re-emits it unchanged.
        let (text, span) = self.semver_run();
        if text.is_empty() {
            self.error_here("expected version");
        }
        Some(Expr {
            span,
            kind: Box::new(ExprKind::Ident(Path {
                segments: vec![Ident { text, span }],
                span,
            })),
        })
    }

    // ---- extension decls (outside Appendix D) -------------------------

    fn extension_decl(&mut self) -> ExtensionDecl {
        let start = self.cur_span();
        // Collect leading keyword idents until we hit a name+`{` or a name
        // followed by an item. Heuristic: keywords are idents; the *name* is
        // the last ident before `{`, `@`, or EOL/`(`.
        let mut keywords = Vec::new();
        // Always take the first ident as a keyword.
        keywords.push(self.ident("keyword"));
        // Multi-word heads: `system dynamics`, `hybrid simulation`,
        // `export api`, `model contract`, `correction policy`,
        // `ordered factor`, `language task`, `decision threshold`.
        let two_word = matches!(
            keywords[0].text.as_str(),
            "system" | "hybrid" | "export" | "model" | "correction" | "ordered" | "language"
        ) || (keywords[0].text == "decision" && self.cur_text() == "threshold");
        if two_word && self.at(TokKind::Ident) {
            keywords.push(self.ident("keyword"));
        }
        let name = if self.at(TokKind::Ident) {
            Some(self.ident("name"))
        } else {
            None
        };
        let version = self.version_tag_opt();
        // Optional trailing `for X` / other tokens before body — skip until
        // `{`.
        let body = if self.at(TokKind::LBrace) {
            Some(self.loose_block())
        } else {
            // header-only extension (e.g. `transaction X isolation ...`) —
            // skip to a `{` on the same construct if present, else stop.
            self.skip_to_body_or_decl();
            if self.at(TokKind::LBrace) {
                Some(self.loose_block())
            } else {
                None
            }
        };
        let end = body.as_ref().map(|b| b.span).unwrap_or(start);
        ExtensionDecl {
            span: start.to(end),
            keywords,
            name,
            version,
            body,
        }
    }

    /// Skip tokens up to (but not consuming) the next `{` or the next
    /// declaration-start keyword, whichever is first. Bounded.
    fn skip_to_body_or_decl(&mut self) {
        let mut guard = 0;
        while !self.at_eof() {
            if self.at(TokKind::LBrace) {
                return;
            }
            if self.at(TokKind::Ident) && DECL_STARTS.contains(&self.cur_text()) {
                return;
            }
            self.bump();
            guard += 1;
            if guard > 200 {
                return;
            }
        }
    }

    // ---- loose blocks / items -----------------------------------------

    fn loose_block_kw(&mut self) -> LooseBlock {
        // consume the keyword (e.g. `policy`) then a brace block
        self.bump();
        self.loose_block()
    }

    /// `{ LooseItem* }` — a structurally-parsed item list (see ast docs).
    fn loose_block(&mut self) -> LooseBlock {
        if !self.enter_depth("a loose block") {
            return LooseBlock {
                span: self.cur_span(),
                items: Vec::new(),
            };
        }
        let mut items = Vec::new();
        let lb = match self.expect(TokKind::LBrace, "`{`") {
            Some(t) => t,
            None => {
                let block = LooseBlock {
                    span: self.cur_span(),
                    items,
                };
                self.leave_depth();
                return block;
            }
        };
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            let item = self.loose_item();
            items.push(item);
            if self.pos == before {
                self.bump();
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(lb.span);
        let block = LooseBlock {
            span: lb.span.to(end),
            items,
        };
        self.leave_depth();
        block
    }

    /// A single loose item: a sequence of expression/atom "parts" and nested
    /// brace blocks, terminated by a newline or `;` at brace depth 0. This is
    /// the fallback for all the item nonterminals Appendix D leaves
    /// undefined. It reads tokens as a flat run of expressions so that
    /// idempotent formatting is still possible.
    fn loose_item(&mut self) -> LooseItem {
        let start = self.cur_span();
        let mut parts = Vec::new();
        let start_line = self.pos;
        let _ = start_line;
        loop {
            let before = self.pos;
            match self.cur() {
                None => break,
                Some(TokKind::RBrace) => break,
                Some(TokKind::Semi) => {
                    self.bump();
                    break;
                }
                Some(TokKind::LBrace) => {
                    parts.push(LoosePart::Block(self.loose_block()));
                    // A block usually ends an item.
                    if self.newline_ahead() {
                        break;
                    }
                }
                // `name: value` — the common shape of the item nonterminals
                // Appendix D leaves undefined (`PolicyItem`, `DatasetItem`,
                // ...). Recognized directly rather than falling through to
                // an expression-level error on the `:` (a bare `Ident`
                // can't start an expression that continues into one). Also
                // covers the typed-binding shape `name: type = value` (e.g.
                // `system dynamics`'s `stock X: Quantity<N> = 100 units`):
                // if a bare `=` immediately follows the colon-value, what we
                // parsed was actually the type and a value follows.
                Some(TokKind::Ident) if self.nth(1) == Some(TokKind::Colon) => {
                    let name = self.ident("field name");
                    self.bump(); // :
                    let value = self.expr_bp(0, true);
                    if self.at(TokKind::Eq) {
                        self.bump();
                        let value2 = self.expr_bp(0, true);
                        parts.push(LoosePart::TypedAssign {
                            name,
                            ty: value,
                            value: value2,
                        });
                    } else {
                        parts.push(LoosePart::Pair { name, value });
                    }
                    if self.newline_ahead() || self.at(TokKind::RBrace) {
                        break;
                    }
                }
                // `name = value` — the untyped counterpart of the above
                // (e.g. `auxiliary utilization = a.value / b.value`).
                Some(TokKind::Ident) if self.nth(1) == Some(TokKind::Eq) => {
                    let name = self.ident("field name");
                    self.bump(); // =
                    let value = self.expr_bp(0, true);
                    parts.push(LoosePart::Assign { name, value });
                    if self.newline_ahead() || self.at(TokKind::RBrace) {
                        break;
                    }
                }
                // `name -> Type from { ... }` — an inline query-shaped item
                // (e.g. a `policy`'s `candidates -> Rel<{ vehicle: Vehicle
                // }> from { AssignmentCandidate(order, vehicle) }`),
                // mirroring `QueryDecl`'s own `-> Type from { ... }`.
                Some(TokKind::Ident) if self.nth(1) == Some(TokKind::Arrow) => {
                    let name = self.ident("field name");
                    self.bump(); // ->
                    let ret = self.type_();
                    if !self.eat_kw("from") {
                        self.error_here("expected `from`");
                    }
                    let from = self.block();
                    parts.push(LoosePart::Query { name, ret, from });
                    if self.newline_ahead() || self.at(TokKind::RBrace) {
                        break;
                    }
                }
                _ => {
                    // Stop the item at a newline boundary.
                    let e = self.loose_atom();
                    parts.push(LoosePart::Expr(e));
                    if self.newline_ahead() || self.at(TokKind::RBrace) {
                        break;
                    }
                }
            }
            if self.pos == before {
                // No token consumed — e.g. a placeholder `...` or any token
                // `loose_atom` can't start an expression from. Force progress
                // so the loose item is total (mirrors `field_block` /
                // `loose_block`), otherwise the parser loops forever.
                self.bump();
            }
        }
        LooseItem {
            span: start.to(self.prev_span()),
            parts,
        }
    }

    /// Whether the next raw token (before newline-skipping) is a newline —
    /// i.e. the current item/line ends here.
    fn newline_ahead(&self) -> bool {
        self.tokens
            .get(self.pos)
            .map(|t| t.kind == TokKind::Newline)
            .unwrap_or(true)
    }

    /// One "atom" inside a loose item: an expression that does NOT cross a
    /// newline. We reuse the general expression grammar but cap it so a
    /// missing operator doesn't merge two logical lines.
    fn loose_atom(&mut self) -> Expr {
        self.expr_bp(0, true)
    }

    // ---- generics / type args -----------------------------------------

    fn generics_opt(&mut self) -> Vec<GenericParam> {
        if !self.at(TokKind::Lt) {
            return Vec::new();
        }
        self.bump();
        let mut params = Vec::new();
        if !self.at(TokKind::Gt) {
            loop {
                let name = self.ident("type parameter");
                let bound = if self.eat(TokKind::Colon) {
                    Some(self.type_())
                } else {
                    None
                };
                params.push(GenericParam {
                    span: name
                        .span
                        .to(bound.as_ref().map(|t| t.span).unwrap_or(name.span)),
                    name,
                    bound,
                });
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokKind::Gt, "`>`");
        params
    }

    fn type_args(&mut self) -> (Vec<TypeArg>, Span) {
        self.expect(TokKind::Lt, "`<`");
        let mut args = Vec::new();
        if !self.at(TokKind::Gt) {
            loop {
                if matches!(
                    self.cur(),
                    Some(TokKind::Str) | Some(TokKind::Int) | Some(TokKind::Float)
                ) {
                    args.push(TypeArg::Lit(self.atom()));
                } else {
                    args.push(TypeArg::Type(self.type_()));
                }
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        let end = self
            .expect(TokKind::Gt, "`>`")
            .map(|t| t.span)
            .unwrap_or_else(|| self.cur_span());
        (args, end)
    }

    // ---- types --------------------------------------------------------

    fn type_(&mut self) -> Type {
        if !self.enter_depth("a type") {
            let span = self.cur_span();
            return Type {
                span,
                kind: TypeKind::Named {
                    path: Path {
                        segments: Vec::new(),
                        span,
                    },
                    args: Vec::new(),
                },
            };
        }
        let mut lhs = self.type_atom();
        // compound unit `T / U`
        while self.at(TokKind::Slash) {
            self.bump();
            let rhs = self.type_atom();
            lhs = Type {
                span: lhs.span.to(rhs.span),
                kind: TypeKind::Div(Box::new(lhs), Box::new(rhs)),
            };
        }
        self.leave_depth();
        lhs
    }

    fn type_atom(&mut self) -> Type {
        if self.at(TokKind::LBrace) {
            return self.row_type();
        }
        let path = self.qual_ident();
        let mut span = path.span;
        let mut args = Vec::new();
        if self.at(TokKind::Lt) {
            let (a, e) = self.type_args();
            args = a;
            span = span.to(e);
        }
        Type {
            span,
            kind: TypeKind::Named { path, args },
        }
    }

    fn row_type(&mut self) -> Type {
        let lb = self.bump(); // {
        let mut fields = Vec::new();
        let mut rest = None;
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            if self.at(TokKind::Pipe) {
                self.bump();
                rest = Some(Box::new(self.type_()));
                break;
            }
            let name = self.ident("field name");
            self.expect(TokKind::Colon, "`:`");
            let ty = self.type_();
            fields.push((name, ty));
            if !self.eat(TokKind::Comma) && !self.at(TokKind::Newline) {
                // allow newline-separated rows
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(lb.span);
        Type {
            span: lb.span.to(end),
            kind: TypeKind::Row { fields, rest },
        }
    }

    // ---- expressions (Pratt) ------------------------------------------

    fn expr(&mut self) -> Expr {
        self.expr_bp(0, false)
    }

    /// Precedence-climbing expression parser. `line_bounded` stops binary
    /// operators from crossing a newline (used inside loose items so two
    /// lines don't accidentally merge).
    fn expr_bp(&mut self, min_bp: u8, line_bounded: bool) -> Expr {
        if !self.enter_depth("an expression") {
            return Expr {
                span: self.cur_span(),
                kind: Box::new(ExprKind::Error(String::new())),
            };
        }
        let mut lhs = self.unary(line_bounded);
        loop {
            if line_bounded && self.newline_ahead() {
                break;
            }
            let op = match self.peek_binop() {
                Some(op) => op,
                None => break,
            };
            let (l_bp, r_bp) = binop_bp(op);
            if l_bp < min_bp {
                break;
            }
            let op_span = self.cur_span();
            self.consume_binop(op);
            // Range with open upper bound: `2..` handled in unary/postfix.
            let rhs = self.expr_bp(r_bp, line_bounded);
            let span = lhs.span.to(rhs.span);
            let kind = if op == PseudoOp::Range {
                ExprKind::Range {
                    lo: Some(lhs),
                    hi: Some(rhs),
                }
            } else if op == PseudoOp::Pipe {
                ExprKind::Binary {
                    op: BinOp::Pipe,
                    lhs,
                    rhs,
                }
            } else {
                ExprKind::Binary {
                    op: op.to_binop(),
                    lhs,
                    rhs,
                }
            };
            let _ = op_span;
            lhs = Expr {
                span,
                kind: Box::new(kind),
            };
        }
        self.leave_depth();
        lhs
    }

    fn unary(&mut self, line_bounded: bool) -> Expr {
        if !self.enter_depth("a unary expression") {
            return Expr {
                span: self.cur_span(),
                kind: Box::new(ExprKind::Error(String::new())),
            };
        }
        let expr = match self.cur() {
            Some(TokKind::Minus) => {
                let t = self.bump();
                let e = self.unary(line_bounded);
                Expr {
                    span: t.span.to(e.span),
                    kind: Box::new(ExprKind::Unary {
                        op: UnOp::Neg,
                        expr: e,
                    }),
                }
            }
            Some(TokKind::Bang) => {
                let t = self.bump();
                let e = self.unary(line_bounded);
                Expr {
                    span: t.span.to(e.span),
                    kind: Box::new(ExprKind::Unary {
                        op: UnOp::Not,
                        expr: e,
                    }),
                }
            }
            _ => self.postfix(line_bounded),
        };
        self.leave_depth();
        expr
    }

    fn postfix(&mut self, line_bounded: bool) -> Expr {
        let mut e = self.atom();
        loop {
            match self.cur() {
                Some(TokKind::Question) => {
                    let t = self.bump();
                    e = Expr {
                        span: e.span.to(t.span),
                        kind: Box::new(ExprKind::Try(e)),
                    };
                }
                Some(TokKind::Dot) => {
                    self.bump();
                    let name = self.ident("field or method");
                    // method call?
                    if self.at(TokKind::LParen) {
                        let args = self.arg_parens();
                        let end = self.prev_span();
                        let callee = Expr {
                            span: e.span.to(name.span),
                            kind: Box::new(ExprKind::Field { base: e, name }),
                        };
                        e = Expr {
                            span: callee.span.to(end),
                            kind: Box::new(ExprKind::Call { callee, args }),
                        };
                    } else {
                        e = Expr {
                            span: e.span.to(name.span),
                            kind: Box::new(ExprKind::Field { base: e, name }),
                        };
                    }
                }
                Some(TokKind::LParen) => {
                    let args = self.arg_parens();
                    let end = self.prev_span();
                    e = Expr {
                        span: e.span.to(end),
                        kind: Box::new(ExprKind::Call { callee: e, args }),
                    };
                }
                // `Base@version` — an inline version tag on a bare name in
                // loose-item position (e.g. `schema LogisticsProjection@3`;
                // Appendix D's own `@version` suffix, just not restricted
                // here to package/module headers).
                Some(TokKind::At) => {
                    self.bump();
                    let (version, vend) = self.semver_run();
                    e = Expr {
                        span: e.span.to(vend),
                        kind: Box::new(ExprKind::Versioned { base: e, version }),
                    };
                }
                // `Base<Args>` generic instantiation — only when a
                // read-only lookahead confirms `<...>` closes as a plausible
                // type-argument list (see `generic_args_end`), so a real
                // `a < b` comparison chain (no matching `>`, or one that
                // encloses an operator/keyword no type-arg list could
                // contain) is left for the binary-operator loop below.
                Some(TokKind::Lt) if self.generic_args_end().is_some() => {
                    let (args, end) = self.type_args();
                    e = Expr {
                        span: e.span.to(end),
                        kind: Box::new(ExprKind::Generic { base: e, args }),
                    };
                }
                // `sim.script { when cond => outcome ... otherwise =>
                // outcome }` adapter-script mini-language (Part VI §2) —
                // only when the brace body's first token is literally
                // `when`/`otherwise`, so it can't misfire on a real
                // `field: value` struct literal.
                Some(TokKind::LBrace)
                    if matches!(&*e.kind, ExprKind::Ident(_) | ExprKind::Field { .. })
                        && !self.newline_ahead_before_brace()
                        && !self.no_struct_lit
                        && self.nth(1) == Some(TokKind::Ident)
                        && matches!(self.cur_text_at(1), "when" | "otherwise") =>
                {
                    let arms = self.adapter_script_arms();
                    let end = self.prev_span();
                    e = Expr {
                        span: e.span.to(end),
                        kind: Box::new(ExprKind::AdapterScript { base: e, arms }),
                    };
                }
                // struct literal `Path { ... }` — only when the `{` is on the
                // same line as the path (heuristic to avoid swallowing block
                // bodies) and the expression so far is a bare identifier path.
                Some(TokKind::LBrace)
                    if matches!(&*e.kind, ExprKind::Ident(_))
                        && !self.newline_ahead_before_brace()
                        && !self.no_struct_lit =>
                {
                    let path = if let ExprKind::Ident(p) = &*e.kind {
                        Some(p.clone())
                    } else {
                        None
                    };
                    let fields = self.arg_braces();
                    let end = self.prev_span();
                    e = Expr {
                        span: e.span.to(end),
                        kind: Box::new(ExprKind::StructLit { path, fields }),
                    };
                }
                // quantity/money literal: number immediately followed by a
                // unit identifier (Appendix D `<number> <unit>`), same line.
                // The unit must be adjacent — a newline between the number and
                // the next ident means the ident starts a new clause (e.g. a
                // guard ending in `0.90` followed by a `without` clause), not a
                // unit.
                Some(TokKind::Ident)
                    if matches!(&*e.kind, ExprKind::Int(_) | ExprKind::Float(_))
                        && !self.is_keyword_boundary()
                        && !self.newline_ahead() =>
                {
                    let unit = self.ident("unit");
                    e = Expr {
                        span: e.span.to(unit.span),
                        kind: Box::new(ExprKind::Measured {
                            value: Box::new(e),
                            unit,
                        }),
                    };
                }
                _ => break,
            }
            if line_bounded && self.newline_ahead() {
                break;
            }
        }
        e
    }

    /// Guard so that `X\n{ ... }` (a block on the next line) is not misread
    /// as a struct literal.
    fn newline_ahead_before_brace(&self) -> bool {
        self.tokens
            .get(self.pos)
            .map(|t| t.kind == TokKind::Newline)
            .unwrap_or(false)
    }

    /// After a numeric literal, some idents are grammar keywords (e.g.
    /// `for`, `to`, `then`, `else`) rather than units. Don't treat those as
    /// units.
    fn is_keyword_boundary(&self) -> bool {
        matches!(
            self.cur_text(),
            "for"
                | "to"
                | "then"
                | "else"
                | "over"
                | "by"
                | "from"
                | "yield"
                | "into"
                | "descending"
                | "ascending"
                | "and"
                | "or"
                | "in"
                | "when"
                | "otherwise"
        )
    }

    /// Read-only lookahead from the current `<` token: does it plausibly
    /// open a generic-argument list (`Ident<...>`, closed by a matching
    /// `>`) rather than start a `<`/`>` comparison chain? Returns the token
    /// index just past the matching `>` on success. Never mutates parser
    /// state (no diagnostics, no `pos` change) so a failed guess is free to
    /// fall back to ordinary binary-operator parsing.
    ///
    /// The allowed interior — identifiers, literals, `.`/`,`/`:`/`|`/`/`,
    /// nested `<...>` and `{...}` (for row-type args like `Frame<{ f: T
    /// }>`), and newlines (type args can span lines, e.g. a multi-field row
    /// type) — is exactly what `type_args`/`type_`/`row_type` themselves
    /// accept. Anything else (an operator, `=`, a control keyword) aborts:
    /// no real type-argument list contains those, so what looked like `<`
    /// is almost certainly "less than".
    fn generic_args_end(&self) -> Option<usize> {
        let mut i = self.pos;
        while self.tokens.get(i).map(|t| t.kind) == Some(TokKind::Newline) {
            i += 1;
        }
        debug_assert_eq!(self.tokens.get(i).map(|t| t.kind), Some(TokKind::Lt));
        let mut angle_depth: i32 = 0;
        let mut brace_depth: i32 = 0;
        let mut saw_any = false;
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > 500 {
                return None;
            }
            let tok = self.tokens.get(i)?;
            match tok.kind {
                TokKind::Lt => {
                    angle_depth += 1;
                    i += 1;
                }
                TokKind::Gt => {
                    angle_depth -= 1;
                    i += 1;
                    if angle_depth < 0 {
                        return None;
                    }
                    if angle_depth == 0 && brace_depth == 0 {
                        return if saw_any { Some(i) } else { None };
                    }
                }
                TokKind::LBrace => {
                    brace_depth += 1;
                    saw_any = true;
                    i += 1;
                }
                TokKind::RBrace => {
                    if brace_depth == 0 {
                        return None;
                    }
                    brace_depth -= 1;
                    i += 1;
                }
                TokKind::Ident => {
                    // Control/logical keywords can't appear in a type; their
                    // presence means this is a real comparison/boolean
                    // expression, e.g. `a < b and c > d`.
                    if is_expr_keyword(tok.text(self.src)) {
                        return None;
                    }
                    saw_any = true;
                    i += 1;
                }
                TokKind::Str
                | TokKind::Int
                | TokKind::Float
                | TokKind::Comma
                | TokKind::Dot
                | TokKind::Colon
                | TokKind::Pipe
                | TokKind::Slash
                | TokKind::Semi
                | TokKind::Newline => {
                    saw_any = true;
                    i += 1;
                }
                _ => return None,
            }
        }
    }

    fn atom(&mut self) -> Expr {
        let span = self.cur_span();
        match self.cur() {
            Some(TokKind::Int) => {
                let t = self.bump();
                let v: i128 = self.text(t).replace('_', "").parse().unwrap_or(0);
                Expr {
                    span,
                    kind: Box::new(ExprKind::Int(v)),
                }
            }
            Some(TokKind::Float) => {
                let t = self.bump();
                let v: f64 = self.text(t).replace('_', "").parse().unwrap_or(0.0);
                Expr {
                    span,
                    kind: Box::new(ExprKind::Float(v)),
                }
            }
            Some(TokKind::Str) => {
                let t = self.bump();
                let raw = self.text(t);
                let inner = &raw[1..raw.len().saturating_sub(1)];
                Expr {
                    span,
                    kind: Box::new(ExprKind::Str(unescape(inner))),
                }
            }
            Some(TokKind::LParen) => {
                self.bump();
                let e = self.expr();
                let end = self
                    .expect(TokKind::RParen, "`)`")
                    .map(|t| t.span)
                    .unwrap_or(e.span);
                Expr {
                    span: span.to(end),
                    kind: Box::new(ExprKind::Paren(e)),
                }
            }
            Some(TokKind::LBracket) => self.list_literal(),
            // closure `|a, b| expr`
            Some(TokKind::Pipe) => self.closure(),
            // anonymous struct/record literal `{ field: expr, ... }` in bare
            // expression position (e.g. `yield { order: o, ... }`). The
            // named form (`Path { ... }`) is handled in `postfix` once an
            // `Ident` atom is seen; this is the path-less form.
            Some(TokKind::LBrace) => {
                let fields = self.arg_braces();
                let end = self.prev_span();
                Expr {
                    span: span.to(end),
                    kind: Box::new(ExprKind::StructLit { path: None, fields }),
                }
            }
            // `...` placeholder — Appendix D's own prose uses it where a
            // grammar production is expected (spec block/arg lists, loose
            // item bodies). Not a real production (see errata); accepted
            // here so it round-trips instead of erroring, with a warning
            // (not an error) since it's a deliberate placeholder, not a
            // malformed program.
            Some(TokKind::DotDotDot) => {
                let t = self.bump();
                self.diags.push(Diagnostic::warning(
                    "BRX-AST-0002",
                    t.span,
                    "placeholder `...` is spec prose, not Appendix D grammar",
                ));
                Expr {
                    span: t.span,
                    kind: Box::new(ExprKind::Ellipsis),
                }
            }
            // leading-dot field selector used in Frame pipelines: `.client`,
            // `.risk descending`. Parse as a field access off an implicit
            // row placeholder (represented as an Ident "" path).
            Some(TokKind::Dot) => {
                self.bump();
                let name = self.ident("field");
                Expr {
                    span: span.to(name.span),
                    kind: Box::new(ExprKind::Field {
                        base: Expr {
                            span: Span::empty(span.start),
                            kind: Box::new(ExprKind::Ident(Path {
                                segments: Vec::new(),
                                span: Span::empty(span.start),
                            })),
                        },
                        name,
                    }),
                }
            }
            Some(TokKind::DotDot) => {
                // open-lower range `..hi`
                self.bump();
                let hi = self.unary(false);
                Expr {
                    span: span.to(hi.span),
                    kind: Box::new(ExprKind::Range {
                        lo: None,
                        hi: Some(hi),
                    }),
                }
            }
            Some(TokKind::Ident) => self.ident_expr(),
            _ => {
                self.error_here("expected an expression");
                // No token consumed here (see module docs on totality), so
                // the captured verbatim text is empty — itself a fixpoint,
                // per the fmt-idempotence design (see ast.rs).
                Expr {
                    span,
                    kind: Box::new(ExprKind::Error(String::new())),
                }
            }
        }
    }

    fn ident_expr(&mut self) -> Expr {
        match self.cur_text() {
            "true" => {
                let t = self.bump();
                return Expr {
                    span: t.span,
                    kind: Box::new(ExprKind::Bool(true)),
                };
            }
            "false" => {
                let t = self.bump();
                return Expr {
                    span: t.span,
                    kind: Box::new(ExprKind::Bool(false)),
                };
            }
            "if" => return self.if_expr(),
            "match" => return self.match_expr(),
            "succeed" => return self.succeed_fail_expr(true),
            "fail" => return self.succeed_fail_expr(false),
            // Only a comprehension when directly followed by `{` — bare
            // `from` is also an ordinary word in loose item bodies (e.g.
            // `alternatives from CandidateAssignment`, `step ... from x`)
            // that must keep parsing as a plain identifier.
            "from" if self.nth(1) == Some(TokKind::LBrace) => return self.comprehension_expr(),
            // `frame from { ... }` (Appendix D §27.2: a materialized Frame
            // built from the same comprehension grammar). `frame` itself
            // carries no separate payload here — it's a spelling cue, so
            // just fold into the same `from` comprehension node.
            "frame"
                if self.nth(1) == Some(TokKind::Ident)
                    && self.cur_text_at(1) == "from"
                    && self.nth(2) == Some(TokKind::LBrace) =>
            {
                self.bump(); // frame
                return self.comprehension_expr();
            }
            _ => {}
        }
        let path = self.qual_ident();
        Expr {
            span: path.span,
            kind: Box::new(ExprKind::Ident(path)),
        }
    }

    fn if_expr(&mut self) -> Expr {
        let kw = self.bump();
        let cond = self.expr_no_struct_lit();
        let then = if self.eat_kw("then") {
            IfBody::Then(self.expr())
        } else {
            IfBody::Block(self.fn_block())
        };
        let else_ = if self.eat_kw("else") {
            if self.at(TokKind::LBrace) {
                Some(self.block_expr())
            } else {
                Some(self.expr())
            }
        } else {
            None
        };
        let end = else_.as_ref().map(|e| e.span).unwrap_or(match &then {
            IfBody::Then(e) => e.span,
            IfBody::Block(b) => b.span,
        });
        Expr {
            span: kw.span.to(end),
            kind: Box::new(ExprKind::If { cond, then, else_ }),
        }
    }

    fn block_expr(&mut self) -> Expr {
        let b = self.fn_block();
        Expr {
            span: b.span,
            kind: Box::new(ExprKind::Block(b)),
        }
    }

    fn match_expr(&mut self) -> Expr {
        let kw = self.bump();
        let scrutinee = self.expr_no_struct_lit();
        self.expect(TokKind::LBrace, "`{`");
        let mut arms = Vec::new();
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            let pattern = self.expr();
            self.expect(TokKind::FatArrow, "`=>`");
            let body = self.expr();
            arms.push(MatchArm {
                span: pattern.span.to(body.span),
                pattern,
                body,
            });
            if self.pos == before {
                self.bump();
            }
        }
        let end = self
            .expect(TokKind::RBrace, "`}`")
            .map(|t| t.span)
            .unwrap_or(scrutinee.span);
        Expr {
            span: kw.span.to(end),
            kind: Box::new(ExprKind::Match { scrutinee, arms }),
        }
    }

    /// `{ (when Expr | otherwise) '=>' Expr }*` — the body of an
    /// [`ExprKind::AdapterScript`]. The caller has already confirmed (via
    /// lookahead) that the brace body starts with `when`/`otherwise`.
    fn adapter_script_arms(&mut self) -> Vec<ScriptArm> {
        self.bump(); // {
        let mut arms = Vec::new();
        loop {
            self.skip_separators();
            if self.at(TokKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            let start = self.cur_span();
            let when = if self.eat_kw("otherwise") {
                None
            } else if self.eat_kw("when") {
                Some(self.expr())
            } else {
                self.error_here("expected `when` or `otherwise`");
                None
            };
            self.expect(TokKind::FatArrow, "`=>`");
            let body = self.expr();
            arms.push(ScriptArm {
                span: start.to(body.span),
                when,
                body,
            });
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(TokKind::RBrace, "`}`");
        arms
    }

    /// `from { PatternClause* } (yield Expr)?` used as an expression, e.g.
    /// `count(from { Move(vehicle: v) })` (Appendix D §4: relation
    /// comprehension). Reuses [`Self::block`] — the identical grammar
    /// `QueryDecl.from` already uses — so a comprehension nested inside a
    /// call argument gets the same clause vocabulary for free.
    fn comprehension_expr(&mut self) -> Expr {
        let kw = self.bump();
        let block = self.block();
        let yield_ = if self.eat_kw("yield") {
            Some(self.expr())
        } else {
            None
        };
        let end = yield_.as_ref().map(|e| e.span).unwrap_or(block.span);
        Expr {
            span: kw.span.to(end),
            kind: Box::new(ExprKind::From {
                block: Box::new(block),
                yield_,
            }),
        }
    }

    fn succeed_fail_expr(&mut self, is_succeed: bool) -> Expr {
        let kw = self.bump();
        // `succeed Outcome { ... }` | `succeed { ... }` | `fail Outcome {..}`
        let path = if self.at(TokKind::Ident) {
            Some(self.qual_ident())
        } else {
            None
        };
        let args = if self.at(TokKind::LBrace) {
            self.arg_braces()
        } else {
            Vec::new()
        };
        let end = self.prev_span();
        let kind = if is_succeed {
            ExprKind::Succeed { path, args }
        } else {
            ExprKind::Fail { path, args }
        };
        Expr {
            span: kw.span.to(end),
            kind: Box::new(kind),
        }
    }

    fn closure(&mut self) -> Expr {
        let start = self.bump(); // |
        let mut params = Vec::new();
        if !self.at(TokKind::Pipe) {
            loop {
                params.push(self.ident("closure param"));
                if !self.eat(TokKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokKind::Pipe, "`|`");
        let body = self.expr();
        Expr {
            span: start.span.to(body.span),
            kind: Box::new(ExprKind::Closure { params, body }),
        }
    }

    fn list_literal(&mut self) -> Expr {
        let lb = self.bump(); // [
        let mut items = Vec::new();
        if !self.at(TokKind::RBracket) {
            loop {
                items.push(Arg {
                    span: self.cur_span(),
                    name: None,
                    value: self.expr(),
                });
                if !self.eat(TokKind::Comma) {
                    break;
                }
                if self.at(TokKind::RBracket) {
                    break;
                }
            }
        }
        let end = self
            .expect(TokKind::RBracket, "`]`")
            .map(|t| t.span)
            .unwrap_or(lb.span);
        // Represent list literals as a call to a synthetic `[]` — simpler:
        // reuse StructLit with no path is wrong; use Call on Ident "[]".
        Expr {
            span: lb.span.to(end),
            kind: Box::new(ExprKind::Call {
                callee: Expr {
                    span: lb.span,
                    kind: Box::new(ExprKind::Ident(Path {
                        segments: vec![Ident {
                            text: "[]".to_string(),
                            span: lb.span,
                        }],
                        span: lb.span,
                    })),
                },
                args: items,
            }),
        }
    }

    // ---- argument lists -----------------------------------------------

    fn arg_parens(&mut self) -> Vec<Arg> {
        self.arg_list(TokKind::LParen, TokKind::RParen)
    }

    fn arg_braces(&mut self) -> Vec<Arg> {
        self.arg_list(TokKind::LBrace, TokKind::RBrace)
    }

    /// `open ( name : expr | expr | name )* close`, comma- or
    /// newline-separated. A bare `name` with no `:` is "punning"
    /// (Appendix D: `other` = `other: other`) and is recorded as a named arg
    /// with the value being the same identifier.
    fn arg_list(&mut self, open: TokKind, close: TokKind) -> Vec<Arg> {
        let mut args = Vec::new();
        if !self.eat(open) {
            return args;
        }
        loop {
            self.skip_separators();
            if self.at(close) || self.at_eof() {
                break;
            }
            let before = self.pos;
            let arg = self.arg(close);
            args.push(arg);
            self.skip_separators();
            if !self.eat(TokKind::Comma) {
                // allow newline as separator; loop re-checks close
                if self.at(close) || self.at_eof() {
                    break;
                }
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(
            close,
            if close == TokKind::RParen {
                "`)`"
            } else {
                "`}`"
            },
        );
        args
    }

    fn arg(&mut self, _close: TokKind) -> Arg {
        let start = self.cur_span();
        // named arg `name : value` — only if Ident followed by `:` and the
        // `:` is not part of a `::` path.
        if self.at(TokKind::Ident) && self.nth(1) == Some(TokKind::Colon) {
            let name = self.ident("field name");
            self.bump(); // :
            let value = self.expr();
            return Arg {
                span: start.to(value.span),
                name: Some(name),
                value,
            };
        }
        // punning: bare identifier that is NOT followed by a `(` or `.` (a
        // call/path) → `name: name`. But `Foo(...)` inside args is a nested
        // value, so only pun a lone ident before a comma/close/newline.
        if self.at(TokKind::Ident)
            && matches!(
                self.nth(1),
                Some(TokKind::Comma)
                    | Some(TokKind::RParen)
                    | Some(TokKind::RBrace)
                    | Some(TokKind::Newline)
                    | None
            )
        {
            let name = self.ident("field name");
            return Arg {
                span: name.span,
                value: Expr {
                    span: name.span,
                    kind: Box::new(ExprKind::Ident(Path::single(name.clone()))),
                },
                name: Some(name),
            };
        }
        let value = self.expr();
        Arg {
            span: value.span,
            name: None,
            value,
        }
    }

    // ---- operator classification --------------------------------------

    fn peek_binop(&self) -> Option<PseudoOp> {
        match self.cur() {
            Some(TokKind::PipeGt) => Some(PseudoOp::Pipe),
            Some(TokKind::PipePipe) => Some(PseudoOp::Bin(BinOp::Or)),
            Some(TokKind::AmpAmp) => Some(PseudoOp::Bin(BinOp::And)),
            Some(TokKind::EqEq) => Some(PseudoOp::Bin(BinOp::Eq)),
            Some(TokKind::Ne) => Some(PseudoOp::Bin(BinOp::Ne)),
            Some(TokKind::Lt) => Some(PseudoOp::Bin(BinOp::Lt)),
            Some(TokKind::Le) => Some(PseudoOp::Bin(BinOp::Le)),
            Some(TokKind::Gt) => Some(PseudoOp::Bin(BinOp::Gt)),
            Some(TokKind::Ge) => Some(PseudoOp::Bin(BinOp::Ge)),
            Some(TokKind::Plus) => Some(PseudoOp::Bin(BinOp::Add)),
            Some(TokKind::Minus) => Some(PseudoOp::Bin(BinOp::Sub)),
            Some(TokKind::Star) => Some(PseudoOp::Bin(BinOp::Mul)),
            Some(TokKind::Slash) => Some(PseudoOp::Bin(BinOp::Div)),
            Some(TokKind::Tilde) => Some(PseudoOp::Bin(BinOp::Tilde)),
            // Formula interaction terms (Appendix D §27.5, `Distance ~ a +
            // b + a:b`): `:` as a binary operator only ever reaches here
            // mid-expression, since every `name: value` position (args,
            // fields, loose-item pairs) intercepts a *leading* `Ident :` one
            // token earlier, before calling into the expression grammar.
            Some(TokKind::Colon) => Some(PseudoOp::Bin(BinOp::Colon)),
            Some(TokKind::DotDot) => Some(PseudoOp::Range),
            Some(TokKind::Ident) => match self.cur_text() {
                "and" => Some(PseudoOp::Bin(BinOp::And)),
                "or" => Some(PseudoOp::Bin(BinOp::Or)),
                "in" => Some(PseudoOp::Bin(BinOp::In)),
                _ => None,
            },
            _ => None,
        }
    }

    fn consume_binop(&mut self, _op: PseudoOp) {
        self.bump();
    }

    // ---- recovery -----------------------------------------------------

    fn skip_separators(&mut self) {
        while matches!(
            self.peek_kind(),
            Some(TokKind::Newline) | Some(TokKind::Semi)
        ) {
            self.pos += 1;
        }
    }

    /// Skip to the matching `}` for the currently open brace nesting,
    /// leaving the cursor just before that `}`.
    fn recover_in_braces(&mut self) {
        let mut depth = 0i32;
        while let Some(k) = self.peek_kind() {
            match k {
                TokKind::LBrace => {
                    depth += 1;
                    self.pos += 1;
                }
                TokKind::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    self.pos += 1;
                }
                _ => self.pos += 1,
            }
            self.fuel += 1;
            if self.fuel > 5_000_000 {
                return;
            }
        }
    }
}

/// A binary-or-pseudo operator recognized at a given cursor position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PseudoOp {
    Bin(BinOp),
    Pipe,
    Range,
}

impl PseudoOp {
    fn to_binop(self) -> BinOp {
        match self {
            PseudoOp::Bin(b) => b,
            PseudoOp::Pipe => BinOp::Pipe,
            PseudoOp::Range => BinOp::Sub, // unreachable in practice
        }
    }
}

/// Binding powers (left, right). Higher binds tighter. Mirrors the v4
/// precedence table referenced by Appendix D (logical < comparison <
/// additive < multiplicative), with `|>` lowest and `~` (formula) very low.
fn binop_bp(op: PseudoOp) -> (u8, u8) {
    match op {
        PseudoOp::Pipe => (1, 2),
        PseudoOp::Bin(BinOp::Tilde) => (3, 4),
        PseudoOp::Bin(BinOp::Or) => (5, 6),
        PseudoOp::Bin(BinOp::And) => (7, 8),
        PseudoOp::Bin(BinOp::In) => (9, 10),
        PseudoOp::Bin(BinOp::Eq)
        | PseudoOp::Bin(BinOp::Ne)
        | PseudoOp::Bin(BinOp::Lt)
        | PseudoOp::Bin(BinOp::Le)
        | PseudoOp::Bin(BinOp::Gt)
        | PseudoOp::Bin(BinOp::Ge) => (11, 12),
        PseudoOp::Range => (13, 14),
        PseudoOp::Bin(BinOp::Add) | PseudoOp::Bin(BinOp::Sub) => (15, 16),
        PseudoOp::Bin(BinOp::Mul) | PseudoOp::Bin(BinOp::Div) => (17, 18),
        PseudoOp::Bin(BinOp::Colon) => (19, 20),
        PseudoOp::Bin(_) => (11, 12),
    }
}

/// Keywords that only make sense inside an expression/clause, never inside
/// a type or generic-argument list. Used by `generic_args_end` to bail out
/// of a `<...>` lookahead that turns out to be a real comparison chain.
fn is_expr_keyword(text: &str) -> bool {
    matches!(
        text,
        "and"
            | "or"
            | "in"
            | "when"
            | "then"
            | "else"
            | "for"
            | "to"
            | "over"
            | "by"
            | "from"
            | "yield"
            | "into"
            | "descending"
            | "ascending"
            | "otherwise"
            | "true"
            | "false"
            | "if"
            | "match"
            | "let"
    )
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('0') => out.push('\0'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_limit_is_a_single_recoverable_diagnostic() {
        let source = "package t @ 1.0.0\n\
derive D: Output(value: value) from { Input(value: (((((1))))) }\n";
        let (_file, diagnostics) = parse_file_with_limit(source, 4);
        let depth_errors: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "BRX-AST-0003")
            .collect();
        assert_eq!(depth_errors.len(), 1, "{diagnostics:?}");
    }

    #[test]
    fn ordinary_nesting_stays_below_the_default_budget() {
        let source = "package t @ 1.0.0\n\
derive D: Output(value: value) from { Input(value: (((((1))))) }\n";
        let (_file, diagnostics) = parse_file(source);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "BRX-AST-0003"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn use_as_parses_both_the_prefix_and_selective_forms() {
        let source = "use brix.math.order as Ord\n\
use brix.math.order.{min, max} as O2\n\
use brix.math.{clamp}\n";
        let (file, diagnostics) = parse_file(source);
        assert!(!diagnostics.has_errors(), "{diagnostics:?}");
        assert_eq!(file.uses.len(), 3);

        assert!(file.uses[0].items.is_empty());
        assert_eq!(
            file.uses[0].alias.as_ref().map(|a| a.text.as_str()),
            Some("Ord")
        );

        assert_eq!(
            file.uses[1]
                .items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>(),
            vec!["min", "max"]
        );
        assert_eq!(
            file.uses[1].alias.as_ref().map(|a| a.text.as_str()),
            Some("O2")
        );

        assert!(file.uses[2].alias.is_none());
    }

    #[test]
    fn reimport_parses_the_bare_and_brace_forms() {
        let source = "package brix.math @ 0.1.0\n\
module Math\n\
reimport order\n\
reimport sign.{abs, neg}\n";
        let (file, diagnostics) = parse_file(source);
        assert!(!diagnostics.has_errors(), "{diagnostics:?}");
        assert_eq!(file.reimports.len(), 2);
        assert!(file.reimports[0].items.is_empty());
        assert_eq!(
            file.reimports[1]
                .items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>(),
            vec!["abs", "neg"]
        );
    }

    #[test]
    fn use_as_and_reimport_round_trip_through_fmt() {
        let source = "use brix.math.order.{min, max} as Ord\n\
use brix.math as Flat\n\
\n\
reimport order\n\
reimport sign.{abs, neg}\n";
        let (file, diagnostics) = parse_file(source);
        assert!(!diagnostics.has_errors(), "{diagnostics:?}");
        let once = crate::fmt::format_file(&file);
        let (file2, diagnostics2) = parse_file(&once);
        assert!(!diagnostics2.has_errors(), "{diagnostics2:?}");
        let twice = crate::fmt::format_file(&file2);
        assert_eq!(once, twice, "fmt of use-as/reimport must be idempotent");
        assert!(once.contains("use brix.math.order.{min, max} as Ord"));
        assert!(once.contains("use brix.math as Flat"));
        assert!(once.contains("reimport order"));
        assert!(once.contains("reimport sign.{abs, neg}"));
    }
}
