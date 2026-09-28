//! A sidecar source map produced alongside the AST (ADR-0010, L1).
//!
//! The surface [`crate::ast`] deliberately carries no spans (see its module
//! doc): it is a semantic tree, and threading `Span` through every node would
//! spread a rendering concern into lowering, canonicalization, and every
//! `PartialEq`/`Eq` derive that feeds a program identity. Instead, the parser
//! optionally records, *alongside* the [`crate::ast::Module`] it already
//! builds, where each top-level item and each identifier token inside it came
//! from. This file is never consulted by lowering or canonicalization — only
//! by a diagnostic renderer deciding where to point a caret.
//!
//! [`crate::parser::parse_bounded`] and [`crate::parser::parse_bounded_with_source_map`]
//! run the exact same parse; the former simply discards the map, so the two
//! can never disagree about the [`crate::ast::Module`] they return.

use crate::ast::Item;

/// A source span, in 1-based line/column coordinates (matching
/// [`crate::lexer::Token`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// One identifier token's text and position, recorded in source order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentOccurrence {
    pub name: String,
    pub line: usize,
    pub col: usize,
}

/// Where one top-level item came from: its syntactic kind, its declared name
/// (where it has one), its overall span, and every identifier token lexed
/// while parsing it (in source order — the first is usually the declared name
/// itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSourceInfo {
    pub kind: &'static str,
    pub name: Option<String>,
    pub span: SourceSpan,
    pub idents: Vec<IdentOccurrence>,
}

impl ItemSourceInfo {
    /// The first identifier occurrence with this exact name, in source order.
    pub fn find_ident(&self, name: &str) -> Option<&IdentOccurrence> {
        self.idents.iter().find(|o| o.name == name)
    }

    /// The best available location for this item: the named identifier's
    /// position when `ident` is given and found, falling back to the item's
    /// own span start.
    pub fn location_for(&self, ident: Option<&str>) -> (usize, usize) {
        if let Some(id) = ident {
            if let Some(occ) = self.find_ident(id) {
                return (occ.line, occ.col);
            }
        }
        (self.span.start_line, self.span.start_col)
    }
}

/// A sidecar map from top-level item name to where it was written.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SourceMap {
    pub items: Vec<ItemSourceInfo>,
}

impl SourceMap {
    /// The item declaring `name`, preferring the **last** one written when a
    /// name is declared more than once — the occurrence a "duplicate ..."
    /// diagnostic is about.
    pub fn find_item(&self, name: &str) -> Option<&ItemSourceInfo> {
        self.items
            .iter()
            .rev()
            .find(|it| it.name.as_deref() == Some(name))
    }

    /// Resolve a `(item name, optional identifier within it)` subject —
    /// as returned by a lowering error's `location_subject` — to a concrete
    /// line/column, when the item is known to this map.
    pub fn resolve(&self, item_name: &str, ident: Option<&str>) -> Option<(usize, usize)> {
        self.find_item(item_name).map(|it| it.location_for(ident))
    }
}

/// The syntactic kind and declared name (if any) of a top-level item, used to
/// index [`SourceMap::items`].
pub(crate) fn item_kind_name(item: &Item) -> (&'static str, Option<String>) {
    match item {
        Item::Use(_) => ("use", None),
        Item::Config(c) => ("config", Some(c.name.clone())),
        Item::Regime(r) => ("regime", Some(r.name.clone())),
        Item::Rule(c) => ("rule", Some(c.name.clone())),
        Item::Let(l) => ("let", Some(l.name.clone())),
        Item::Fn(c) => ("fn", Some(c.name.clone())),
        Item::Show(_) => ("show", None),
        Item::Witness { name, .. } => ("witness", Some(name.clone())),
        Item::Propose(p) => ("propose", Some(p.name.clone())),
        Item::Commit(c) => ("commit", Some(c.name.clone())),
        Item::Input(i) => ("input", Some(i.name.clone())),
    }
}
