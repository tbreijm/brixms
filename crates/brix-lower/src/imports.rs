//! Import resolution: `use brix.soc`.
//!
//! **Deliberately filesystem-agnostic.** Resolution takes a loader rather than
//! reading files itself, so `brix-lower` stays a pure lowering crate and this
//! is testable without a directory layout. The CLI supplies the loader that
//! knows where packages live.
//!
//! **What an import brings in.** Declarations only — `config` and `fn`. A
//! library's `let` and `witness` bindings are its own worked examples, not its
//! exports, and re-checking them at every import site would make an import's
//! cost grow with the library's example count while establishing nothing new
//! about the importer.

use std::collections::{BTreeMap, BTreeSet};

use brix_syntax::ast::{self, Item};

/// Why an import could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The loader had no source for this package.
    NotFound(String),
    /// The imported source did not parse. Carries the package and the parse
    /// error, so the diagnostic names *which* import is broken rather than
    /// reporting a syntax error against the importing file.
    Parse { package: String, error: String },
    /// A package imports itself, directly or through others. Carries the
    /// chain, closing on its first element.
    Cycle(Vec<String>),
    /// Two visible declarations share a name.
    ///
    /// Refused rather than shadowed: with no qualified-name syntax there is no
    /// way for a program to say which one it meant, so silently picking one
    /// would make the program's meaning depend on import order.
    Conflict {
        name: String,
        first: String,
        second: String,
    },
}

/// Resolve every `use` in `module`, returning a module whose imported
/// declarations are in scope.
///
/// Imports are transitive: importing a package imports what it imports, since
/// its declarations may refer to them.
pub fn resolve_imports(
    module: &ast::Module,
    load: &dyn Fn(&str) -> Option<String>,
) -> Result<ast::Module, ImportError> {
    // `origin` records which package each visible name came from, so a
    // conflict can name both sides instead of just failing.
    let mut origin: BTreeMap<String, String> = BTreeMap::new();
    let mut imported: Vec<Item> = Vec::new();
    let mut loaded: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = Vec::new();

    for item in &module.items {
        if let Item::Use(path) = item {
            resolve_one(
                path,
                load,
                &mut origin,
                &mut imported,
                &mut loaded,
                &mut stack,
            )?;
        }
    }

    // The importing module's own declarations are checked against the imported
    // ones too — a local name colliding with an imported one is the same
    // ambiguity, in the other direction.
    for item in &module.items {
        if let Some(name) = declared_name(item) {
            if let Some(from) = origin.get(&name) {
                return Err(ImportError::Conflict {
                    name,
                    first: from.clone(),
                    second: "<this module>".to_string(),
                });
            }
        }
    }

    // Imported declarations come first so the importing module's items keep
    // their relative order — which matters, because declaration order is
    // semantic for `let` and for L3 rule eligibility.
    let mut items = imported;
    items.extend(
        module
            .items
            .iter()
            .filter(|i| !matches!(i, Item::Use(_)))
            .cloned(),
    );
    Ok(ast::Module { items })
}

fn resolve_one(
    path: &str,
    load: &dyn Fn(&str) -> Option<String>,
    origin: &mut BTreeMap<String, String>,
    imported: &mut Vec<Item>,
    loaded: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), ImportError> {
    if loaded.contains(path) {
        return Ok(()); // already in scope; importing twice is not an error
    }
    if stack.iter().any(|p| p == path) {
        let mut cycle = stack.clone();
        cycle.push(path.to_string());
        return Err(ImportError::Cycle(cycle));
    }

    let Some(source) = load(path) else {
        return Err(ImportError::NotFound(path.to_string()));
    };
    let parsed = brix_syntax::parse(&source).map_err(|e| ImportError::Parse {
        package: path.to_string(),
        error: e.to_string(),
    })?;

    stack.push(path.to_string());
    for item in &parsed.items {
        if let Item::Use(inner) = item {
            resolve_one(inner, load, origin, imported, loaded, stack)?;
        }
    }
    stack.pop();

    for item in &parsed.items {
        // Declarations only. A library's bindings are its examples.
        if !matches!(item, Item::Config(_) | Item::Fn(_)) {
            continue;
        }
        let Some(name) = declared_name(item) else {
            continue;
        };
        if let Some(first) = origin.get(&name) {
            return Err(ImportError::Conflict {
                name,
                first: first.clone(),
                second: path.to_string(),
            });
        }
        origin.insert(name, path.to_string());
        imported.push(item.clone());
    }

    loaded.insert(path.to_string());
    Ok(())
}

/// The name a top-level item declares, where it declares one.
fn declared_name(item: &Item) -> Option<String> {
    match item {
        Item::Config(c) => Some(c.name.clone()),
        Item::Fn(c) => Some(c.name.clone()),
        Item::Rule(c) => Some(c.name.clone()),
        Item::Regime(r) => Some(r.name.clone()),
        Item::Let(l) => Some(l.name.clone()),
        Item::Witness { name, .. } => Some(name.clone()),
        Item::Show(_) | Item::Use(_) => None,
    }
}
