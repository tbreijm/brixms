//! `brix fmt` v0 — canonical, idempotent AST pretty-printer.
//!
//! Appendix D: "`brix fmt` output is the canonical form." v0 establishes the
//! canonical *shape* (two-space indent, one clause/field per line, normalized
//! spacing around operators and punctuation) and, crucially, is **idempotent**
//! and **parse-stable**: `fmt(parse(fmt(parse(src)))) == fmt(parse(src))`.
//! The corpus test `fmt_idempotent` proves this over every fixture.
//!
//! It formats from the AST, so it is intentionally lossy about incidental
//! source layout (blank lines, comment placement) — comments are dropped in
//! v0 (a comment-preserving formatter is a follow-up; noted in the report).
//! Everything semantically load-bearing round-trips.

use crate::ast::*;

fn vis_prefix(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Private => "",
        Visibility::Public(None) => "pub ",
        Visibility::Public(Some(RelVis::Read)) => "pub read ",
        Visibility::Public(Some(RelVis::Write)) => "pub write ",
        Visibility::Public(Some(RelVis::Derive)) => "pub derive ",
    }
}

/// Format a parsed file to canonical text.
pub fn format_file(file: &File) -> String {
    let mut f = Formatter::default();
    f.file(file);
    f.finish()
}

#[derive(Default)]
struct Formatter {
    out: String,
    indent: usize,
}

impl Formatter {
    fn finish(mut self) -> String {
        // Exactly one trailing newline.
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out.push('\n');
        self.out
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn blank(&mut self) {
        if !self.out.ends_with("\n\n") && !self.out.is_empty() {
            self.out.push('\n');
        }
    }

    /// Emit a captured verbatim run (see `ast::Decl::Error` and friends) on
    /// its own line(s). Never a comment: the lexer drops comments on
    /// reparse, which would break `fmt` idempotence for exactly the tokens
    /// this exists to preserve. An empty capture (the zero-token error case)
    /// emits nothing — dropping is itself a fixpoint.
    fn verbatim_lines(&mut self, raw: &str) {
        if raw.is_empty() {
            return;
        }
        for line in raw.lines() {
            self.line(line);
        }
    }

    fn file(&mut self, file: &File) {
        if let Some(p) = &file.package {
            self.line(&format!(
                "package {} @ {}",
                path_str(&p.name),
                p.version.text
            ));
        }
        if let Some(m) = &file.module {
            self.line(&format!("module {}", m.name.text));
        }
        if file.package.is_some() || file.module.is_some() {
            self.blank();
        }
        for u in &file.uses {
            if u.items.is_empty() {
                self.line(&format!("use {}", path_str(&u.path)));
            } else {
                let items = u
                    .items
                    .iter()
                    .map(|i| i.text.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(&format!("use {}.{{{}}}", path_str(&u.path), items));
            }
        }
        if !file.uses.is_empty() {
            self.blank();
        }
        for (i, d) in file.decls.iter().enumerate() {
            if i > 0 {
                self.blank();
            }
            self.decl(d);
        }
    }

    fn decl(&mut self, d: &Decl) {
        match d {
            Decl::Entity(e) => self.entity(e),
            Decl::Rel(r) => self.rel(r),
            Decl::Derive(d) => self.derive(d),
            Decl::Constraint(c) => self.constraint(c),
            Decl::Query(q) => self.query(q),
            Decl::Protocol(p) => self.protocol(p),
            Decl::Driver(d) => self.driver(d),
            Decl::Scenario(s) => self.scenario(s),
            Decl::Fn(f) => self.fn_decl(f),
            Decl::Type(t) => self.line(&format!(
                "{}type {}{} = {}",
                vis_prefix(t.vis),
                t.name.text,
                generics_str(&t.generics),
                type_str(&t.value)
            )),
            Decl::Measure(m) => self.line(&format!("{}measure {}", vis_prefix(m.vis), m.name.text)),
            Decl::Unit(u) => self.line(&format!(
                "{}unit {}: {} = {}",
                vis_prefix(u.vis),
                u.name.text,
                u.measure.text,
                expr_str(&u.value)
            )),
            Decl::Enum(e) => self.enum_decl(e),
            Decl::Record(r) => self.record(r),
            Decl::DataRecipe(r) => self.data_recipe(r),
            Decl::Feature(f) => self.feature(f),
            Decl::FeatureSet(f) => {
                let v = f
                    .version
                    .as_ref()
                    .map(|e| format!(" @ {}", expr_str(e)))
                    .unwrap_or_default();
                self.header_loose(&format!("{}feature set {}{}", vis_prefix(f.vis), f.name.text, v), &f.items);
            }
            Decl::Dataset(d) => self.header_loose(&format!("{}dataset {}", vis_prefix(d.vis), d.name.text), &d.items),
            Decl::StatModel(s) => {
                self.header_loose(&format!("{}statistical model {}", vis_prefix(s.vis), s.name.text), &s.items)
            }
            Decl::MlWorkflow(m) => {
                self.header_loose(&format!("{}ml workflow {}", vis_prefix(m.vis), m.name.text), &m.items)
            }
            Decl::Experiment(e) => {
                let kw = match e.kind {
                    ExperimentKind::Experiment => "experiment",
                    ExperimentKind::Tuning => "tuning",
                };
                self.header_loose(&format!("{}{kw} {}", vis_prefix(e.vis), e.name.text), &e.items);
            }
            Decl::Visualization(v) => {
                self.header_loose(&format!("{}visualization {}", vis_prefix(v.vis), v.name.text), &v.items)
            }
            Decl::Let(l) => {
                let ty =
                    l.ty.as_ref()
                        .map(|t| format!(": {}", type_str(t)))
                        .unwrap_or_default();
                self.line(&format!("{}let {}{ty} = {}", vis_prefix(l.vis), l.name.text, expr_str(&l.value)));
            }
            Decl::Extension(x) => self.extension(x),
            Decl::Error(_, raw) => self.verbatim_lines(raw),
        }
    }

    fn entity(&mut self, e: &EntityDecl) {
        let vis = vis_prefix(e.vis);
        if e.fields.is_empty() {
            self.line(&format!("{vis}entity {} {{}}", e.name.text));
            return;
        }
        self.line(&format!("{vis}entity {} {{", e.name.text));
        self.indent += 1;
        for field in &e.fields {
            self.line(&field_str(field));
        }
        self.indent -= 1;
        self.line("}");
    }

    fn rel(&mut self, r: &RelDecl) {
        let vis = vis_prefix(r.vis);
        let kw = match r.kind {
            RelKind::Ground => "rel".to_string(),
            RelKind::State => "state rel".to_string(),
            RelKind::Event => "event rel".to_string(),
            RelKind::Open => "open rel".to_string(),
        };
        let mods = if r.mods.is_empty() {
            String::new()
        } else {
            let m = r.mods.iter().map(relmod_str).collect::<Vec<_>>().join(" ");
            format!(" {m}")
        };
        if r.roles.is_empty() {
            self.line(&format!("{vis}{kw} {} {{}}{mods}", r.name.text));
            return;
        }
        self.line(&format!("{vis}{kw} {} {{", r.name.text));
        self.indent += 1;
        for role in &r.roles {
            self.line(&field_str(role));
        }
        self.indent -= 1;
        self.line(&format!("}}{mods}"));
    }

    fn derive(&mut self, d: &DeriveDecl) {
        self.line(&format!(
            "{}derive {}: {} from {{",
            vis_prefix(d.vis),
            d.name.text,
            head_str(&d.head)
        ));
        self.indent += 1;
        self.clauses(&d.body.clauses);
        self.indent -= 1;
        self.line("}");
    }

    fn clauses(&mut self, clauses: &[Clause]) {
        for c in clauses {
            self.clause(c);
        }
    }

    fn clause(&mut self, c: &Clause) {
        match c {
            Clause::Edge(e) => self.line(&edge_str(e)),
            Clause::History(e) => self.line(&format!("history {}", edge_str(e))),
            Clause::Entity(e) => self.line(&entity_clause_str(e)),
            Clause::Let(l) => self.line(&format!(
                "let {} = {}",
                expr_str(&l.pattern),
                expr_str(&l.value)
            )),
            Clause::When(e) => self.line(&format!("when {}", expr_str(e))),
            Clause::Any(cases) => {
                self.line("any {");
                self.indent += 1;
                for case in cases {
                    self.line("case {");
                    self.indent += 1;
                    self.clauses(&case.clauses);
                    self.indent -= 1;
                    self.line("}");
                }
                self.indent -= 1;
                self.line("}");
            }
            Clause::Exists(b) => self.nested_block("exists", b),
            Clause::Without(b) => self.nested_block("without", b),
            Clause::Optional(b) => self.nested_block("optional", b),
            Clause::Cross(b) => self.nested_block("cross", b),
            Clause::Path(p) => self.line(&format!(
                "path {} from {} to {}",
                path_expr_str(&p.expr),
                p.from.text,
                p.to.text
            )),
            Clause::Error(_, raw) => self.verbatim_lines(raw),
        }
    }

    fn nested_block(&mut self, kw: &str, b: &Block) {
        if b.clauses.is_empty() {
            self.line(&format!("{kw} {{}}"));
            return;
        }
        self.line(&format!("{kw} {{"));
        self.indent += 1;
        self.clauses(&b.clauses);
        self.indent -= 1;
        self.line("}");
    }

    fn constraint(&mut self, c: &ConstraintDecl) {
        let kind = match c.kind {
            ConstraintKind::Advisory => "advisory",
            ConstraintKind::Strict => "strict",
            ConstraintKind::Audit => "audit",
        };
        self.line(&format!("{}constraint {} {} {{", vis_prefix(c.vis), c.name.text, kind));
        self.indent += 1;
        self.clauses(&c.body.clauses);
        self.indent -= 1;
        self.line("}");
    }

    fn query(&mut self, q: &QueryDecl) {
        self.line(&format!(
            "{}query {}({}) -> {} =",
            vis_prefix(q.vis),
            q.name.text,
            params_str(&q.params),
            type_str(&q.ret)
        ));
        self.indent += 1;
        self.line("from {");
        self.indent += 1;
        self.clauses(&q.from.clauses);
        self.indent -= 1;
        self.line("}");
        self.line(&format!("yield {}", expr_str(&q.yield_)));
        if let Some(o) = &q.order {
            let by = o.by.iter().map(expr_str).collect::<Vec<_>>().join(", ");
            let limit = o
                .limit
                .as_ref()
                .map(|e| format!(" limit {}", expr_str(e)))
                .unwrap_or_default();
            self.line(&format!("order by {by}{limit}"));
        }
        self.indent -= 1;
    }

    fn protocol(&mut self, p: &ProtocolDecl) {
        self.line(&format!(
            "{}protocol {}{} {{",
            vis_prefix(p.vis),
            p.name.text,
            generics_str(&p.generics)
        ));
        self.indent += 1;
        // request
        if !p.request.roles.is_empty() || !p.request.key.is_empty() {
            let key = if p.request.key.is_empty() {
                String::new()
            } else {
                format!(
                    " key({})",
                    p.request
                        .key
                        .iter()
                        .map(|i| i.text.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            if p.request.roles.is_empty() {
                self.line(&format!("request {{}}{key}"));
            } else {
                self.line("request {");
                self.indent += 1;
                for r in &p.request.roles {
                    self.line(&field_str(r));
                }
                self.indent -= 1;
                self.line(&format!("}}{key}"));
            }
        }
        for m in &p.methods {
            let ret = m
                .ret
                .as_ref()
                .map(|t| format!(" -> {}", type_str(t)))
                .unwrap_or_default();
            self.line(&format!(
                "{}({}){}",
                m.name.text,
                params_str(&m.params),
                ret
            ));
        }
        for o in &p.outcomes {
            if o.roles.is_empty() {
                self.line(&format!("outcome {} {{}}", o.name.text));
            } else {
                self.line(&format!("outcome {} {{", o.name.text));
                self.indent += 1;
                for r in &o.roles {
                    self.line(&field_str(r));
                }
                self.indent -= 1;
                self.line("}");
            }
        }
        if let Some(pol) = &p.policy {
            self.header_loose("policy", pol);
        }
        self.indent -= 1;
        self.line("}");
    }

    fn driver(&mut self, d: &DriverDecl) {
        let needs = if d.needs.is_empty() {
            String::new()
        } else {
            let n = d.needs.iter().map(cap_str).collect::<Vec<_>>().join(", ");
            format!(" needs {n}")
        };
        self.line(&format!(
            "{}driver {} for {}{} {{",
            vis_prefix(d.vis), d.name.text, d.for_protocol.text, needs
        ));
        self.indent += 1;
        self.line(&format!(
            "on request({}, {}) {{",
            d.req_param.text, d.cancel_param.text
        ));
        self.indent += 1;
        self.stmts(&d.body.stmts);
        self.indent -= 1;
        self.line("}");
        self.indent -= 1;
        self.line("}");
    }

    fn scenario(&mut self, s: &ScenarioDecl) {
        self.line(&format!("{}scenario {} {{", vis_prefix(s.vis), s.name.text));
        self.indent += 1;
        match &s.seed {
            SeedDecl::Nat(n, _) => self.line(&format!("seed {n}")),
            SeedDecl::Each(e, _) => self.line(&format!("seed each {}", expr_str(e))),
        }
        for b in &s.binds {
            let args = if b.args.is_empty() {
                String::new()
            } else {
                format!("({})", args_str(&b.args))
            };
            let to =
                b.to.as_ref()
                    .map(|e| format!(" to {}", expr_str(e)))
                    .unwrap_or_default();
            self.line(&format!("bind {}{}{}", path_str(&b.protocol), args, to));
        }
        if let Some(setup) = &s.setup {
            self.line("setup {");
            self.indent += 1;
            self.tx_stmts(&setup.stmts);
            self.indent -= 1;
            self.line("}");
        }
        for step in &s.steps {
            self.line(&format!(
                "step every {} for {} {{",
                expr_str(&step.every),
                expr_str(&step.for_)
            ));
            self.indent += 1;
            self.tx_stmts(&step.body.stmts);
            self.indent -= 1;
            self.line("}");
        }
        for at in &s.ats {
            self.line(&format!("at {} {{", expr_str(&at.at)));
            self.indent += 1;
            self.tx_stmts(&at.body.stmts);
            self.indent -= 1;
            self.line("}");
        }
        for a in &s.asserts {
            let mode = match a.mode {
                AssertMode::Always => "always",
                AssertMode::Eventually => "eventually",
                AssertMode::AtEnd => "at end",
            };
            self.line(&format!("assert {} {{ {} }}", mode, expr_str(&a.cond)));
        }
        self.indent -= 1;
        self.line("}");
    }

    fn tx_stmts(&mut self, stmts: &[TxStmt]) {
        for s in stmts {
            match s {
                TxStmt::Let { pattern, value } => {
                    self.line(&format!(
                        "let {} = {}",
                        expr_str(pattern),
                        tx_expr_str(value)
                    ));
                }
                TxStmt::Expr(e) => self.line(&tx_expr_str(e)),
                TxStmt::Error(_, raw) => self.verbatim_lines(raw),
            }
        }
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            match s {
                Stmt::Let { pattern, value, .. } => {
                    self.line(&format!("let {} = {}", expr_str(pattern), expr_str(value)));
                }
                Stmt::Expr(e) => {
                    // multi-line exprs (if/match/block) format specially
                    let text = expr_str(e);
                    for (i, l) in text.split('\n').enumerate() {
                        if i == 0 {
                            self.line(l);
                        } else {
                            // already-indented continuation handled by expr_str
                            self.out.push_str(l);
                            self.out.push('\n');
                        }
                    }
                }
                Stmt::Error(_, raw) => self.verbatim_lines(raw),
            }
        }
    }

    fn fn_decl(&mut self, f: &FnDecl) {
        let mut head = String::from(vis_prefix(f.vis));
        if f.partial {
            head.push_str("partial ");
        }
        if f.aggregate {
            head.push_str("aggregate ");
        }
        head.push_str(&format!(
            "fn {}{}({}) -> {}",
            f.name.text,
            generics_str(&f.generics),
            params_str(&f.params),
            type_str(&f.ret)
        ));
        if let Some(effs) = &f.effects {
            head.push_str(&format!(
                " !{{{}}}",
                effs.iter()
                    .map(|i| i.text.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        match &f.body {
            None => self.line(&head),
            Some(FnBody::Expr(e)) => self.line(&format!("{head} = {}", expr_str(e))),
            Some(FnBody::Block(b)) => {
                self.line(&format!("{head} {{"));
                self.indent += 1;
                self.stmts(&b.stmts);
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn enum_decl(&mut self, e: &EnumDecl) {
        self.line(&format!(
            "{}enum {}{} {{",
            vis_prefix(e.vis),
            e.name.text,
            generics_str(&e.generics)
        ));
        self.indent += 1;
        for v in &e.variants {
            self.line(&variant_str(v));
        }
        self.indent -= 1;
        self.line("}");
    }

    fn record(&mut self, r: &RecordDecl) {
        self.line(&format!(
            "{}record {}{} {{",
            vis_prefix(r.vis),
            r.name.text,
            generics_str(&r.generics)
        ));
        self.indent += 1;
        for f in &r.fields {
            self.line(&field_str(f));
        }
        self.indent -= 1;
        self.line("}");
    }

    fn data_recipe(&mut self, r: &DataRecipeDecl) {
        self.line(&format!("{}data recipe {} {{", vis_prefix(r.vis), r.name.text));
        self.indent += 1;
        for item in &r.items {
            match item {
                RecipeItem::Input(t) => self.line(&format!("input {}", type_str(t))),
                RecipeItem::Output(t) => self.line(&format!("output {}", type_str(t))),
                RecipeItem::Quarantine(e) => self.line(&format!("quarantine {}", expr_str(e))),
                RecipeItem::Step { name, rest } => {
                    let r = loose_item_str(rest);
                    if r.is_empty() {
                        self.line(&format!("step {}", name.text));
                    } else {
                        self.line(&format!("step {} {}", name.text, r));
                    }
                }
            }
        }
        self.indent -= 1;
        self.line("}");
    }

    fn feature(&mut self, f: &FeatureDecl) {
        let head = format!(
            "{}feature {}({}) -> {}",
            vis_prefix(f.vis),
            f.name.text,
            params_str(&f.params),
            type_str(&f.ret)
        );
        match &f.body {
            FeatureBody::Expr(e) => self.line(&format!("{head} = {}", expr_str(e))),
            FeatureBody::Items(items) => {
                self.line(&format!("{head} {{"));
                self.indent += 1;
                for it in items {
                    self.line(&feature_item_str(it));
                }
                self.indent -= 1;
                self.line("}");
            }
        }
    }

    fn header_loose(&mut self, header: &str, block: &LooseBlock) {
        if block.items.is_empty() {
            self.line(&format!("{header} {{}}"));
            return;
        }
        self.line(&format!("{header} {{"));
        self.indent += 1;
        self.loose_items(&block.items);
        self.indent -= 1;
        self.line("}");
    }

    fn loose_items(&mut self, items: &[LooseItem]) {
        for it in items {
            // An item that ends in a block prints the block expanded.
            if let Some(LoosePart::Block(_)) = it.parts.last() {
                let prefix_parts = &it.parts[..it.parts.len() - 1];
                let prefix = prefix_parts
                    .iter()
                    .map(loose_part_inline)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if let LoosePart::Block(b) = it.parts.last().unwrap() {
                    if prefix.is_empty() {
                        self.header_loose("", b);
                    } else {
                        self.header_loose(&prefix, b);
                    }
                }
            } else {
                let s = loose_item_str(it);
                if !s.is_empty() {
                    self.line(&s);
                }
            }
        }
    }

    fn extension(&mut self, x: &ExtensionDecl) {
        let mut head = x
            .keywords
            .iter()
            .map(|k| k.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(n) = &x.name {
            head.push(' ');
            head.push_str(&n.text);
        }
        if let Some(v) = &x.version {
            head.push_str(&format!(" @ {}", expr_str(v)));
        }
        match &x.body {
            Some(b) => self.header_loose(&head, b),
            None => self.line(&head),
        }
    }
}

// ---- inline string builders (no indentation state) --------------------

fn path_str(p: &Path) -> String {
    p.segments
        .iter()
        .map(|s| s.text.clone())
        .collect::<Vec<_>>()
        .join(".")
}

fn field_str(f: &FieldDecl) -> String {
    let key = if f.is_key { "key " } else { "" };
    format!("{key}{}: {}", f.name.text, type_str(&f.ty))
}

fn params_str(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", p.name.text, type_str(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn generics_str(g: &[GenericParam]) -> String {
    if g.is_empty() {
        return String::new();
    }
    let inner = g
        .iter()
        .map(|p| {
            if let Some(b) = &p.bound {
                format!("{}: {}", p.name.text, type_str(b))
            } else {
                p.name.text.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{inner}>")
}

fn relmod_str(m: &RelMod) -> String {
    let list = |ids: &[Ident]| {
        ids.iter()
            .map(|i| i.text.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match m {
        RelMod::Key(ids) => format!("key({})", list(ids)),
        RelMod::Unique(ids) => format!("unique({})", list(ids)),
        RelMod::Index(ids) => format!("index({})", list(ids)),
        RelMod::Partition(ids) => format!("partition({})", list(ids)),
        RelMod::Time(id) => format!("time({})", id.text),
    }
}

fn head_str(h: &Head) -> String {
    match h {
        Head::Tuple { path, args } => {
            if args.is_empty() {
                format!("{}()", path_str(path))
            } else {
                format!("{}({})", path_str(path), args_str(args))
            }
        }
        Head::Node {
            binder,
            ty,
            args,
            keyed_by,
        } => {
            let a = if args.is_empty() {
                " {}".to_string()
            } else {
                format!(" {{ {} }}", args_str(args))
            };
            let k = if keyed_by.is_empty() {
                String::new()
            } else {
                format!(
                    " keyed by ({})",
                    keyed_by
                        .iter()
                        .map(|i| i.text.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!("{}: {}{}{}", binder.text, ty.text, a, k)
        }
        Head::Mask { target, by } => format!("mask({}) by {}", target.text, by.text),
    }
}

fn edge_str(e: &EdgeClause) -> String {
    let alias = e
        .alias
        .as_ref()
        .map(|a| format!("{} @ ", a.text))
        .unwrap_or_default();
    if e.args.is_empty() {
        format!("{alias}{}()", path_str(&e.path))
    } else {
        format!("{alias}{}({})", path_str(&e.path), args_str(&e.args))
    }
}

fn entity_clause_str(e: &EntityClause) -> String {
    if e.fields.is_empty() {
        format!("{}: {} {{}}", e.binder.text, e.ty.text)
    } else {
        format!(
            "{}: {} {{ {} }}",
            e.binder.text,
            e.ty.text,
            args_str(&e.fields)
        )
    }
}

fn args_str(args: &[Arg]) -> String {
    args.iter().map(arg_str).collect::<Vec<_>>().join(", ")
}

fn arg_str(a: &Arg) -> String {
    match &a.name {
        Some(n) => {
            // punning: `name: name` collapses to `name`
            if let ExprKind::Ident(p) = &*a.value.kind {
                if p.segments.len() == 1 && p.segments[0].text == n.text {
                    return n.text.clone();
                }
            }
            format!("{}: {}", n.text, expr_str(&a.value))
        }
        None => expr_str(&a.value),
    }
}

fn cap_str(c: &CapRef) -> String {
    if c.args.is_empty() {
        c.name.text.clone()
    } else {
        format!("{}<{}>", c.name.text, type_args_str(&c.args))
    }
}

fn type_args_str(args: &[TypeArg]) -> String {
    args.iter()
        .map(|a| match a {
            TypeArg::Type(t) => type_str(t),
            TypeArg::Lit(e) => expr_str(e),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn type_str(t: &Type) -> String {
    match &t.kind {
        TypeKind::Named { path, args } => {
            if args.is_empty() {
                path_str(path)
            } else {
                format!("{}<{}>", path_str(path), type_args_str(args))
            }
        }
        TypeKind::Row { fields, rest } => {
            let mut inner = fields
                .iter()
                .map(|(n, ty)| format!("{}: {}", n.text, type_str(ty)))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(r) = rest {
                inner.push_str(&format!(" | {}", type_str(r)));
            }
            format!("{{ {inner} }}")
        }
        TypeKind::Div(a, b) => format!("{} / {}", type_str(a), type_str(b)),
    }
}

fn variant_str(v: &EnumVariant) -> String {
    match &v.payload {
        VariantPayload::Unit => v.name.text.clone(),
        VariantPayload::Tuple(tys) => format!(
            "{}({})",
            v.name.text,
            tys.iter().map(type_str).collect::<Vec<_>>().join(", ")
        ),
        VariantPayload::Struct(fields) => format!(
            "{} {{ {} }}",
            v.name.text,
            fields.iter().map(field_str).collect::<Vec<_>>().join("; ")
        ),
    }
}

fn feature_item_str(it: &FeatureItem) -> String {
    match it {
        FeatureItem::ObservationTime(e) => format!("observationTime {}", expr_str(e)),
        FeatureItem::Window(e) => format!("window {}", expr_str(e)),
        FeatureItem::Source(p) => format!("source {}", path_str(p)),
        FeatureItem::Leakage(i) => format!("leakage {}", i.text),
        FeatureItem::Missing(i) => format!("missing {}", i.text),
    }
}

fn loose_item_str(it: &LooseItem) -> String {
    it.parts
        .iter()
        .map(loose_part_inline)
        // An empty rendering (a zero-token captured `Error`, see
        // `ExprKind::Error`) must contribute nothing — not even the
        // separating space `join` would otherwise insert, which reparses to
        // a *different* (shorter) part list and breaks idempotence.
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn loose_part_inline(p: &LoosePart) -> String {
    match p {
        LoosePart::Expr(e) => expr_str(e),
        LoosePart::Pair { name, value } => format!("{}: {}", name.text, expr_str(value)),
        LoosePart::Assign { name, value } => format!("{} = {}", name.text, expr_str(value)),
        LoosePart::TypedAssign { name, ty, value } => {
            format!("{}: {} = {}", name.text, expr_str(ty), expr_str(value))
        }
        LoosePart::Query { name, ret, from } => {
            format!(
                "{} -> {} from {}",
                name.text,
                type_str(ret),
                block_clauses_inline(from)
            )
        }
        LoosePart::Block(b) => {
            let rendered = b
                .items
                .iter()
                .map(loose_item_str)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            if rendered.is_empty() {
                "{}".to_string()
            } else {
                format!("{{ {} }}", rendered.join("; "))
            }
        }
    }
}

fn path_expr_str(p: &PathExpr) -> String {
    match p {
        PathExpr::Step(s) => format!("{}({} -> {})", path_str(&s.path), s.from.text, s.to.text),
        PathExpr::Alt(alts) => alts
            .iter()
            .map(path_expr_str)
            .collect::<Vec<_>>()
            .join(" | "),
        PathExpr::Group(inner) => format!("( {} )", path_expr_str(inner)),
        PathExpr::Repeat(inner, rep) => {
            let r = match rep {
                Repeat::Plus => "+".to_string(),
                Repeat::Star => "*".to_string(),
                Repeat::Range(lo, Some(hi)) => format!("{{{lo}, {hi}}}"),
                Repeat::Range(lo, None) => format!("{{{lo}}}"),
            };
            format!("{}{}", path_expr_str(inner), r)
        }
    }
}

fn tx_expr_str(t: &TxExpr) -> String {
    match t {
        TxExpr::Ensure { ty, args, .. } => brace_call("ensure", &ty.text, args),
        TxExpr::Fresh { ty, args, .. } => brace_call("fresh", &ty.text, args),
        TxExpr::AssertStruct { ty, args, .. } => brace_call("assert", &ty.text, args),
        TxExpr::AssertTuple { path, args, .. } => {
            format!("assert {}({})", path_str(path), args_str(args))
        }
        TxExpr::Set { path, args, .. } => format!("set {}({})", path_str(path), args_str(args)),
        TxExpr::Retract { expr, .. } => format!("retract {}", expr_str(expr)),
        TxExpr::Supersede { new, old, .. } => {
            format!("supersede {} over {}", expr_str(new), expr_str(old))
        }
    }
}

fn brace_call(kw: &str, ty: &str, args: &[Arg]) -> String {
    if args.is_empty() {
        format!("{kw} {ty} {{}}")
    } else {
        format!("{kw} {ty} {{ {} }}", args_str(args))
    }
}

/// Format an expression to a single line (v0). Multi-line constructs
/// (`if`/`match`) are rendered inline too, which is unambiguous and keeps
/// the formatter idempotent without a line-width engine (a wrapping pass is
/// a v1 concern).
fn expr_str(e: &Expr) -> String {
    match &*e.kind {
        ExprKind::Int(v) => v.to_string(),
        ExprKind::Float(v) => format_float(*v),
        ExprKind::Str(s) => format!("\"{}\"", escape(s)),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Measured { value, unit } => format!("{} {}", expr_str(value), unit.text),
        ExprKind::Ident(p) => path_str(p),
        ExprKind::Unary { op, expr } => {
            let o = match op {
                UnOp::Neg => "-",
                UnOp::Not => "!",
            };
            format!("{o}{}", expr_str(expr))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            format!("{} {} {}", expr_str(lhs), op.as_str(), expr_str(rhs))
        }
        ExprKind::Range { lo, hi } => {
            let l = lo.as_ref().map(expr_str).unwrap_or_default();
            let h = hi.as_ref().map(expr_str).unwrap_or_default();
            format!("{l}..{h}")
        }
        ExprKind::Call { callee, args } => {
            if let ExprKind::Ident(p) = &*callee.kind {
                if p.segments.len() == 1 && p.segments[0].text == "[]" {
                    return format!(
                        "[{}]",
                        args.iter()
                            .map(|a| expr_str(&a.value))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            format!("{}({})", expr_str(callee), args_str(args))
        }
        ExprKind::StructLit { path, fields } => {
            let p = path.as_ref().map(path_str).unwrap_or_default();
            let sp = if p.is_empty() {
                String::new()
            } else {
                format!("{p} ")
            };
            if fields.is_empty() {
                format!("{sp}{{}}")
            } else {
                format!("{sp}{{ {} }}", args_str(fields))
            }
        }
        ExprKind::Field { base, name } => {
            if matches!(&*base.kind, ExprKind::Ident(p) if p.segments.is_empty()) {
                format!(".{}", name.text)
            } else {
                format!("{}.{}", expr_str(base), name.text)
            }
        }
        ExprKind::Try(e) => format!("{}?", expr_str(e)),
        ExprKind::If { cond, then, else_ } => {
            let t = match then {
                IfBody::Then(e) => format!("then {}", expr_str(e)),
                IfBody::Block(b) => format!("{{ {} }}", block_inline(&b.stmts)),
            };
            let e = else_
                .as_ref()
                .map(|e| format!(" else {}", expr_str(e)))
                .unwrap_or_default();
            format!("if {} {}{}", expr_str(cond), t, e)
        }
        ExprKind::Match { scrutinee, arms } => {
            let a = arms
                .iter()
                .map(|arm| format!("{} => {}", expr_str(&arm.pattern), expr_str(&arm.body)))
                .collect::<Vec<_>>()
                .join("; ");
            format!("match {} {{ {} }}", expr_str(scrutinee), a)
        }
        ExprKind::Closure { params, body } => {
            let p = params
                .iter()
                .map(|i| i.text.clone())
                .collect::<Vec<_>>()
                .join(", ");
            format!("|{}| {}", p, expr_str(body))
        }
        ExprKind::Succeed { path, args } => outcome_str("succeed", path, args),
        ExprKind::Fail { path, args } => outcome_str("fail", path, args),
        ExprKind::Paren(inner) => format!("({})", expr_str(inner)),
        ExprKind::Block(b) => format!("{{ {} }}", block_inline(&b.stmts)),
        ExprKind::Versioned { base, version } => format!("{}@{}", expr_str(base), version),
        ExprKind::AdapterScript { base, arms } => {
            let a = arms
                .iter()
                .map(|arm| {
                    let w = arm
                        .when
                        .as_ref()
                        .map(|e| format!("when {}", expr_str(e)))
                        .unwrap_or_else(|| "otherwise".to_string());
                    format!("{w} => {}", expr_str(&arm.body))
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{} {{ {a} }}", expr_str(base))
        }
        ExprKind::Generic { base, args } => {
            format!("{}<{}>", expr_str(base), type_args_str(args))
        }
        ExprKind::From { block, yield_ } => {
            let clauses = block
                .clauses
                .iter()
                .map(clause_inline_str)
                .collect::<Vec<_>>()
                .join("; ");
            let y = yield_
                .as_ref()
                .map(|e| format!(" yield {}", expr_str(e)))
                .unwrap_or_default();
            if clauses.is_empty() {
                format!("from {{}}{y}")
            } else {
                format!("from {{ {clauses} }}{y}")
            }
        }
        ExprKind::Ellipsis => "...".to_string(),
        // Never a comment (see module docs): the lexer drops comments on
        // reparse, which is exactly the bug this verbatim-capture design
        // fixes. An empty capture (the only case reachable from `atom`'s
        // zero-token error arm) is itself a fixpoint.
        ExprKind::Error(raw) => raw.clone(),
    }
}

/// Render one [`Clause`] on a single line — the inline counterpart of
/// [`Formatter::clause`], used inside expression-position `from { ... }`
/// comprehensions (see `ExprKind::From`) where the surrounding expression
/// must stay on one line (v0 has no line-wrapping engine).
fn clause_inline_str(c: &Clause) -> String {
    match c {
        Clause::Edge(e) => edge_str(e),
        Clause::History(e) => format!("history {}", edge_str(e)),
        Clause::Entity(e) => entity_clause_str(e),
        Clause::Let(l) => format!("let {} = {}", expr_str(&l.pattern), expr_str(&l.value)),
        Clause::When(e) => format!("when {}", expr_str(e)),
        Clause::Any(cases) => {
            let c = cases
                .iter()
                .map(|b| {
                    format!(
                        "case {{ {} }}",
                        b.clauses
                            .iter()
                            .map(clause_inline_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("any {{ {c} }}")
        }
        Clause::Exists(b) => format!("exists {}", block_clauses_inline(b)),
        Clause::Without(b) => format!("without {}", block_clauses_inline(b)),
        Clause::Optional(b) => format!("optional {}", block_clauses_inline(b)),
        Clause::Cross(b) => format!("cross {}", block_clauses_inline(b)),
        Clause::Path(p) => format!(
            "path {} from {} to {}",
            path_expr_str(&p.expr),
            p.from.text,
            p.to.text
        ),
        Clause::Error(_, raw) => raw.clone(),
    }
}

fn block_clauses_inline(b: &Block) -> String {
    if b.clauses.is_empty() {
        return "{}".to_string();
    }
    let inner = b
        .clauses
        .iter()
        .map(clause_inline_str)
        .collect::<Vec<_>>()
        .join("; ");
    format!("{{ {inner} }}")
}

fn outcome_str(kw: &str, path: &Option<Path>, args: &[Arg]) -> String {
    let p = path
        .as_ref()
        .map(|p| format!(" {}", path_str(p)))
        .unwrap_or_default();
    if args.is_empty() {
        format!("{kw}{p} {{}}")
    } else {
        format!("{kw}{p} {{ {} }}", args_str(args))
    }
}

fn block_inline(stmts: &[Stmt]) -> String {
    stmts
        .iter()
        .map(|s| match s {
            Stmt::Let { pattern, value, .. } => {
                format!("let {} = {}", expr_str(pattern), expr_str(value))
            }
            Stmt::Expr(e) => expr_str(e),
            Stmt::Error(_, raw) => raw.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_float(v: f64) -> String {
    if v == v.trunc() && v.is_finite() {
        format!("{v:.1}")
    } else {
        let s = format!("{v}");
        s
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}
