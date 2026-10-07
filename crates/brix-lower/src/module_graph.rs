//! Module graph, qualified symbol resolution, bounded loading, and interface-based invalidation (ADR-0046).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use brix_canon::{CanonWriter, Digest, Domain};
use brix_syntax::ast::{self, Item};
use brix_syntax::parse_bounded;
use brix_syntax::ParseLimits;

/// Limits enforced when loading and linking a module graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleLoaderLimits {
    /// Maximum import depth along any dependency chain.
    pub max_import_depth: usize,
    /// Maximum number of total modules in the graph.
    pub max_import_modules: usize,
    /// Maximum individual module size in bytes.
    pub max_module_source_bytes: usize,
    /// Maximum total cumulative source bytes across all loaded modules.
    pub max_total_source_bytes: usize,
}

impl Default for ModuleLoaderLimits {
    fn default() -> Self {
        Self {
            max_import_depth: 16,
            max_import_modules: 256,
            max_module_source_bytes: 1024 * 1024,    // 1 MiB
            max_total_source_bytes: 8 * 1024 * 1024, // 8 MiB
        }
    }
}

/// Errors during module loading, graph resolution, and linking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleLinkError {
    /// Package or module source was not found.
    NotFound(String),
    /// Module failed to parse.
    Parse { module: String, error: String },
    /// Import cycle detected.
    Cycle(Vec<String>),
    /// Import depth exceeded limit before descending.
    ImportDepthExceeded {
        limit: usize,
        found: usize,
        chain: Vec<String>,
    },
    /// Total module count exceeded limit before enqueuing.
    ModuleCountExceeded { limit: usize, found: usize },
    /// Individual module size exceeded limit before reading.
    ModuleBytesExceeded {
        module: String,
        limit: usize,
        found: usize,
    },
    /// Total cumulative source bytes exceeded limit.
    TotalBytesExceeded { limit: usize, found: usize },
    /// An unexported symbol was referenced from outside its module.
    NonExportedAccess { module: String, symbol: String },
    /// Unresolved symbol reference.
    UnresolvedSymbol { module: String, symbol: String },
    /// Conflicting top-level declaration in flat scope.
    Conflict {
        name: String,
        first_module: String,
        second_module: String,
    },
    /// Invalid schema contract.
    InvalidSchema {
        module: String,
        name: String,
        reason: String,
    },
    /// Invalid helper function contract.
    InvalidHelper {
        module: String,
        name: String,
        reason: String,
    },
}

impl fmt::Display for ModuleLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(pkg) => write!(f, "module or package '{pkg}' was not found"),
            Self::Parse { module, error } => {
                write!(f, "module '{module}' failed to parse: {error}")
            }
            Self::Cycle(chain) => write!(f, "import cycle detected: {}", chain.join(" -> ")),
            Self::ImportDepthExceeded {
                limit,
                found,
                chain,
            } => {
                write!(
                    f,
                    "import depth limit exceeded (limit: {limit}, found: {found}) along chain: {}",
                    chain.join(" -> ")
                )
            }
            Self::ModuleCountExceeded { limit, found } => {
                write!(
                    f,
                    "module count limit exceeded (limit: {limit}, found: {found})"
                )
            }
            Self::ModuleBytesExceeded {
                module,
                limit,
                found,
            } => {
                write!(
                    f,
                    "module '{module}' size exceeded limit (limit: {limit} bytes, found: {found} bytes)"
                )
            }
            Self::TotalBytesExceeded { limit, found } => {
                write!(
                    f,
                    "total module source bytes exceeded limit (limit: {limit} bytes, found: {found} bytes)"
                )
            }
            Self::NonExportedAccess { module, symbol } => {
                write!(f, "symbol '{symbol}' in module '{module}' is not exported")
            }
            Self::UnresolvedSymbol { module, symbol } => {
                write!(f, "unresolved symbol '{symbol}' in module '{module}'")
            }
            Self::Conflict {
                name,
                first_module,
                second_module,
            } => {
                write!(
                    f,
                    "symbol '{name}' defined in both '{first_module}' and '{second_module}'"
                )
            }
            Self::InvalidSchema {
                module,
                name,
                reason,
            } => {
                write!(
                    f,
                    "invalid schema contract '{name}' in module '{module}': {reason}"
                )
            }
            Self::InvalidHelper {
                module,
                name,
                reason,
            } => {
                write!(
                    f,
                    "invalid helper function contract '{name}' in module '{module}': {reason}"
                )
            }
        }
    }
}

impl std::error::Error for ModuleLinkError {}

/// A fully-qualified symbol name `module::name`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QualifiedName {
    pub module: String,
    pub name: String,
}

impl QualifiedName {
    pub fn new(module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            name: name.into(),
        }
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.module, self.name)
    }
}

/// The syntactic kind of an exported symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExportKind {
    Config,
    Fn,
    RelInput,
    RelDerived,
    Rule,
    Let,
}

/// One exported symbol declaration with signature for interface-based invalidation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExportedSymbol {
    pub name: String,
    pub kind: ExportKind,
    /// Canonical interface signature representation (parameter types and return types).
    pub signature: String,
}

/// Pinned manifest entry for a single loaded module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleManifestEntry {
    pub module_name: String,
    pub source_digest: Digest,
    pub interface_digest: Digest,
    pub source_bytes: usize,
    pub direct_imports: Vec<String>,
    pub exported_symbols: Vec<ExportedSymbol>,
}

/// Pinned immutable program graph manifest binding the complete source closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramGraphManifest {
    pub profile: String,
    pub root_module: String,
    pub modules: BTreeMap<String, ModuleManifestEntry>,
    pub transitive_manifest_digest: Digest,
}

/// In-memory representation of a single loaded module.
#[derive(Clone, Debug)]
pub struct LoadedModule {
    pub name: String,
    pub source: String,
    pub ast: ast::Module,
    pub source_digest: Digest,
    pub interface_digest: Digest,
    pub direct_imports: Vec<String>,
    pub exported_symbols: Vec<ExportedSymbol>,
}

/// Contract for loading module source and inspecting module metadata.
///
/// Implementors can supply `module_size` to probe source byte length before
/// allocating the source string into memory, guaranteeing that resource bounds
/// are charged strictly before allocation.
pub trait ModuleLoader {
    /// Probe the size in bytes of a module before allocating its source buffer.
    /// Returns `None` if size is not known in advance.
    fn module_size(&self, _module_name: &str) -> Option<usize> {
        None
    }

    /// Read the module source into an owned string.
    fn load_module(&self, module_name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> ModuleLoader for F {
    fn load_module(&self, module_name: &str) -> Option<String> {
        self(module_name)
    }
}

/// A sized module loader that probes size before loading, ensuring byte limits
/// are charged and enforced strictly before source allocation.
pub struct SizedLoader<S, L> {
    size_fn: S,
    load_fn: L,
}

impl<S, L> SizedLoader<S, L>
where
    S: Fn(&str) -> Option<usize>,
    L: Fn(&str) -> Option<String>,
{
    pub fn new(size_fn: S, load_fn: L) -> Self {
        Self { size_fn, load_fn }
    }
}

impl<S, L> ModuleLoader for SizedLoader<S, L>
where
    S: Fn(&str) -> Option<usize>,
    L: Fn(&str) -> Option<String>,
{
    fn module_size(&self, module_name: &str) -> Option<usize> {
        (self.size_fn)(module_name)
    }

    fn load_module(&self, module_name: &str) -> Option<String> {
        (self.load_fn)(module_name)
    }
}

/// A resolved in-memory module graph.
#[derive(Clone, Debug)]
pub struct ModuleGraph {
    pub root_module: String,
    pub modules: BTreeMap<String, LoadedModule>,
}

impl ModuleGraph {
    /// Load a complete module graph starting at `root_name` using `loader` under `limits`.
    pub fn load(
        root_name: &str,
        loader: &(impl ModuleLoader + ?Sized),
        limits: ModuleLoaderLimits,
    ) -> Result<Self, ModuleLinkError> {
        let mut modules: BTreeMap<String, LoadedModule> = BTreeMap::new();
        let mut total_bytes: usize = 0;
        let mut stack: Vec<String> = Vec::new();

        Self::load_recursive(
            root_name,
            loader,
            limits,
            &mut total_bytes,
            &mut stack,
            &mut modules,
        )?;

        Ok(Self {
            root_module: root_name.to_string(),
            modules,
        })
    }

    fn load_recursive(
        module_name: &str,
        loader: &(impl ModuleLoader + ?Sized),
        limits: ModuleLoaderLimits,
        total_bytes: &mut usize,
        stack: &mut Vec<String>,
        modules: &mut BTreeMap<String, LoadedModule>,
    ) -> Result<(), ModuleLinkError> {
        if modules.contains_key(module_name) {
            return Ok(());
        }

        // Check depth limit before descending
        if stack.len() >= limits.max_import_depth {
            let mut chain = stack.clone();
            chain.push(module_name.to_string());
            return Err(ModuleLinkError::ImportDepthExceeded {
                limit: limits.max_import_depth,
                found: stack.len() + 1,
                chain,
            });
        }

        // Check cycle
        if let Some(pos) = stack.iter().position(|s| s == module_name) {
            let mut cycle = stack[pos..].to_vec();
            cycle.push(module_name.to_string());
            return Err(ModuleLinkError::Cycle(cycle));
        }

        // Check total module count limit before enqueuing
        if modules.len().saturating_add(stack.len()) >= limits.max_import_modules {
            return Err(ModuleLinkError::ModuleCountExceeded {
                limit: limits.max_import_modules,
                found: modules.len().saturating_add(stack.len()).saturating_add(1),
            });
        }

        // Check module size and total bytes limit BEFORE allocation if known
        if let Some(size) = loader.module_size(module_name) {
            if size > limits.max_module_source_bytes {
                return Err(ModuleLinkError::ModuleBytesExceeded {
                    module: module_name.to_string(),
                    limit: limits.max_module_source_bytes,
                    found: size,
                });
            }
            if total_bytes.saturating_add(size) > limits.max_total_source_bytes {
                return Err(ModuleLinkError::TotalBytesExceeded {
                    limit: limits.max_total_source_bytes,
                    found: total_bytes.saturating_add(size),
                });
            }
        }

        let Some(source) = loader.load_module(module_name) else {
            return Err(ModuleLinkError::NotFound(module_name.to_string()));
        };

        // Metadata is only an early refusal optimization, never authority for
        // the bytes actually returned (a file may change between stat/read).
        {
            if source.len() > limits.max_module_source_bytes {
                return Err(ModuleLinkError::ModuleBytesExceeded {
                    module: module_name.to_string(),
                    limit: limits.max_module_source_bytes,
                    found: source.len(),
                });
            }
            if total_bytes.saturating_add(source.len()) > limits.max_total_source_bytes {
                return Err(ModuleLinkError::TotalBytesExceeded {
                    limit: limits.max_total_source_bytes,
                    found: total_bytes.saturating_add(source.len()),
                });
            }
        }
        *total_bytes += source.len();

        let parsed = parse_bounded(&source, ParseLimits::generous()).map_err(|e| {
            ModuleLinkError::Parse {
                module: module_name.to_string(),
                error: e.to_string(),
            }
        })?;

        let source_digest = Digest::of(Domain::Value, source.as_bytes());

        // Extract direct imports
        let mut direct_imports = Vec::new();
        for item in &parsed.items {
            if let Item::Use(path) = item {
                if !direct_imports.contains(path) {
                    direct_imports.push(path.clone());
                }
            }
        }

        // Extract exports and compute interface digest
        let mut exported_symbols = Vec::new();
        for item in &parsed.items {
            if let Item::Export(inner) = item {
                if let Some(sym) = extract_exported_symbol(inner) {
                    exported_symbols.push(sym);
                }
            }
        }
        exported_symbols.sort();
        let interface_digest = compute_interface_digest(&exported_symbols);

        stack.push(module_name.to_string());
        for dep in &direct_imports {
            Self::load_recursive(dep, loader, limits, total_bytes, stack, modules)?;
        }
        stack.pop();

        modules.insert(
            module_name.to_string(),
            LoadedModule {
                name: module_name.to_string(),
                source,
                ast: parsed,
                source_digest,
                interface_digest,
                direct_imports,
                exported_symbols,
            },
        );

        Ok(())
    }

    /// Produce an immutable pinned [`ProgramGraphManifest`].
    pub fn manifest(&self, profile: &str) -> ProgramGraphManifest {
        let mut manifest_entries = BTreeMap::new();
        let mut w = CanonWriter::new();
        w.write_ident(profile);
        w.write_ident(&self.root_module);

        for (name, module) in &self.modules {
            let entry = ModuleManifestEntry {
                module_name: name.clone(),
                source_digest: module.source_digest,
                interface_digest: module.interface_digest,
                source_bytes: module.source.len(),
                direct_imports: module.direct_imports.clone(),
                exported_symbols: module.exported_symbols.clone(),
            };

            w.write_ident(name);
            w.write_bytes(module.source_digest.as_bytes());
            w.write_bytes(module.interface_digest.as_bytes());
            w.write_uint(module.source.len() as u64);
            w.write_list(module.direct_imports.iter().map(|d| d.as_bytes().to_vec()));

            manifest_entries.insert(name.clone(), entry);
        }

        let transitive_manifest_digest = w.digest(Domain::Value);

        ProgramGraphManifest {
            profile: profile.to_string(),
            root_module: self.root_module.clone(),
            modules: manifest_entries,
            transitive_manifest_digest,
        }
    }

    /// Link the modules into a [`LinkedProgram`] with qualified symbols and dead-code reachability pruning.
    pub fn link(&self) -> Result<LinkedProgram, ModuleLinkError> {
        let root = self
            .modules
            .get(&self.root_module)
            .ok_or_else(|| ModuleLinkError::NotFound(self.root_module.clone()))?;

        // 1. Collect all declarations across all modules:
        //    Root module's local items are all included.
        //    Imported modules expose ONLY their exported items.
        let mut all_configs: BTreeMap<QualifiedName, ast::ConfigDecl> = BTreeMap::new();
        let mut all_functions: BTreeMap<QualifiedName, ast::Callable> = BTreeMap::new();
        let mut all_rel_inputs: BTreeMap<QualifiedName, ast::RelInputDecl> = BTreeMap::new();
        let mut all_rel_derived: BTreeMap<QualifiedName, ast::RelDerivedDecl> = BTreeMap::new();
        let mut all_rules: BTreeMap<QualifiedName, ast::Callable> = BTreeMap::new();
        let mut all_decides: BTreeMap<QualifiedName, ast::DecideDecl> = BTreeMap::new();
        let mut exported_symbols: BTreeSet<QualifiedName> = BTreeSet::new();
        let mut root_items: Vec<Item> = Vec::new();

        for (mod_name, module) in &self.modules {
            let is_root = mod_name == &self.root_module;
            let mut declared_names = BTreeSet::new();

            for item in &module.ast.items {
                let (decl, is_exported) = match item {
                    Item::Export(inner) => (inner.as_ref(), true),
                    other => (other, false),
                };

                if let Some(name) = declaration_name(decl) {
                    if !declared_names.insert(name.to_string()) {
                        return Err(ModuleLinkError::Conflict {
                            name: name.to_string(),
                            first_module: mod_name.clone(),
                            second_module: mod_name.clone(),
                        });
                    }
                }
                if is_root {
                    root_items.push(decl.clone());
                }

                let qname = match decl {
                    Item::Config(c) => {
                        let q = QualifiedName::new(mod_name, &c.name);
                        all_configs.insert(q.clone(), c.clone());
                        q
                    }
                    Item::Fn(f) => {
                        let q = QualifiedName::new(mod_name, &f.name);
                        all_functions.insert(q.clone(), f.clone());
                        q
                    }
                    Item::RelInput(r) => {
                        let q = QualifiedName::new(mod_name, &r.name);
                        all_rel_inputs.insert(q.clone(), r.clone());
                        q
                    }
                    Item::RelDerived(r) => {
                        let q = QualifiedName::new(mod_name, &r.name);
                        all_rel_derived.insert(q.clone(), r.clone());
                        q
                    }
                    Item::Rule(r) => {
                        let q = QualifiedName::new(mod_name, &r.name);
                        all_rules.insert(q.clone(), r.clone());
                        q
                    }
                    Item::Decide(d) => {
                        let q = QualifiedName::new(mod_name, &d.name);
                        all_decides.insert(q.clone(), d.clone());
                        q
                    }
                    _ => continue,
                };

                if is_exported {
                    exported_symbols.insert(qname);
                }
            }
        }

        // Validate every loaded declaration before reachability pruning. An
        // unused invalid declaration must not become silently admissible.
        let all = LinkedProgram {
            root_module: self.root_module.clone(),
            root_items: root_items.clone(),
            configs: all_configs.clone(),
            functions: all_functions.clone(),
            rel_inputs: all_rel_inputs.clone(),
            rel_derived: all_rel_derived.clone(),
            rules: all_rules.clone(),
            decides: all_decides.clone(),
        };
        all.validate_contracts()?;
        let known: BTreeSet<QualifiedName> = all_configs
            .keys()
            .chain(all_functions.keys())
            .chain(all_rel_inputs.keys())
            .chain(all_rel_derived.keys())
            .chain(all_rules.keys())
            .chain(all_decides.keys())
            .cloned()
            .collect();
        for (module_name, module) in &self.modules {
            for item in &module.ast.items {
                let item = match item {
                    Item::Export(inner) => inner.as_ref(),
                    item => item,
                };
                let mut deps = BTreeSet::new();
                trace_item_deps(item, module_name, &mut deps);
                for dep in deps {
                    if dep.module != *module_name {
                        if !module.direct_imports.contains(&dep.module) || !known.contains(&dep) {
                            return Err(ModuleLinkError::UnresolvedSymbol {
                                module: dep.module,
                                symbol: dep.name,
                            });
                        }
                        if !exported_symbols.contains(&dep) {
                            return Err(ModuleLinkError::NonExportedAccess {
                                module: dep.module,
                                symbol: dep.name,
                            });
                        }
                    }
                }
            }
        }

        // 2. Dead-code reachability pruning:
        //    Seeds: All items declared in the root module.
        //    Trace referenced symbols (functions, configs, relations) transitively.
        let mut reachable_symbols: BTreeSet<QualifiedName> = BTreeSet::new();

        // Initialize seeds from root module
        for item in &root.ast.items {
            let decl = match item {
                Item::Export(inner) => inner.as_ref(),
                other => other,
            };
            match decl {
                Item::Config(c) => {
                    let q = QualifiedName::new(&self.root_module, &c.name);
                    reachable_symbols.insert(q.clone());
                    trace_config_deps(c, &self.root_module, &mut reachable_symbols);
                }
                Item::Fn(f) => {
                    let q = QualifiedName::new(&self.root_module, &f.name);
                    reachable_symbols.insert(q.clone());
                    trace_expr_deps(&f.body, &self.root_module, &mut reachable_symbols);
                }
                Item::RelInput(r) => {
                    let q = QualifiedName::new(&self.root_module, &r.name);
                    reachable_symbols.insert(q);
                    trace_ty_deps(&r.ty, &self.root_module, &mut reachable_symbols);
                }
                Item::RelDerived(r) => {
                    let q = QualifiedName::new(&self.root_module, &r.name);
                    reachable_symbols.insert(q);
                    trace_rel_query_deps(&r.query, &self.root_module, &mut reachable_symbols);
                }
                Item::Rule(r) => {
                    let q = QualifiedName::new(&self.root_module, &r.name);
                    reachable_symbols.insert(q);
                    trace_expr_deps(&r.body, &self.root_module, &mut reachable_symbols);
                }
                Item::Let(l) => {
                    trace_expr_deps(&l.value, &self.root_module, &mut reachable_symbols);
                }
                Item::Show(e) => {
                    trace_expr_deps(e, &self.root_module, &mut reachable_symbols);
                }
                Item::Decide(d) => {
                    let q = QualifiedName::new(&self.root_module, &d.name);
                    reachable_symbols.insert(q);
                    trace_expr_deps(&d.list, &self.root_module, &mut reachable_symbols);
                    for p in &d.proposals {
                        trace_expr_deps(&p.guard, &self.root_module, &mut reachable_symbols);
                        trace_expr_deps(&p.value, &self.root_module, &mut reachable_symbols);
                    }
                }
                _ => {}
            }
        }

        // Expand reachability closure until fixpoint
        let mut changed = true;
        while changed {
            let prev_len = reachable_symbols.len();
            let current: Vec<QualifiedName> = reachable_symbols.iter().cloned().collect();
            for q in current {
                let mut deps = BTreeSet::new();
                if let Some(f) = all_functions.get(&q) {
                    trace_callable_deps(f, &q.module, &mut deps);
                }
                if let Some(input) = all_rel_inputs.get(&q) {
                    trace_ty_deps(&input.ty, &q.module, &mut deps);
                }
                if let Some(rule) = all_rules.get(&q) {
                    trace_callable_deps(rule, &q.module, &mut deps);
                }
                if let Some(decide) = all_decides.get(&q) {
                    trace_item_deps(&Item::Decide(decide.clone()), &q.module, &mut deps);
                }
                if let Some(r) = all_rel_derived.get(&q) {
                    trace_rel_query_deps(&r.query, &q.module, &mut deps);
                }
                if let Some(c) = all_configs.get(&q) {
                    trace_config_deps(c, &q.module, &mut deps);
                }

                for dep in deps {
                    // Check visibility: cross-module reference must be exported!
                    if dep.module != q.module
                        && dep.module != self.root_module
                        && !exported_symbols.contains(&dep)
                        && (all_functions.contains_key(&dep)
                            || all_configs.contains_key(&dep)
                            || all_rel_inputs.contains_key(&dep)
                            || all_rel_derived.contains_key(&dep))
                    {
                        return Err(ModuleLinkError::NonExportedAccess {
                            module: dep.module.clone(),
                            symbol: dep.name.clone(),
                        });
                    }
                    reachable_symbols.insert(dep);
                }
            }
            changed = reachable_symbols.len() > prev_len;
        }

        // 3. Filter reachable items for the active plan
        let active_configs: BTreeMap<QualifiedName, ast::ConfigDecl> = all_configs
            .into_iter()
            .filter(|(q, _)| reachable_symbols.contains(q))
            .collect();

        let active_functions: BTreeMap<QualifiedName, ast::Callable> = all_functions
            .into_iter()
            .filter(|(q, _)| reachable_symbols.contains(q))
            .collect();

        let active_rel_inputs: BTreeMap<QualifiedName, ast::RelInputDecl> = all_rel_inputs
            .into_iter()
            .filter(|(q, _)| reachable_symbols.contains(q))
            .collect();

        let active_rel_derived: BTreeMap<QualifiedName, ast::RelDerivedDecl> = all_rel_derived
            .into_iter()
            .filter(|(q, _)| reachable_symbols.contains(q))
            .collect();

        let linked = LinkedProgram {
            root_module: self.root_module.clone(),
            root_items,
            configs: active_configs,
            functions: active_functions,
            rel_inputs: active_rel_inputs,
            rel_derived: active_rel_derived,
            rules: all_rules
                .into_iter()
                .filter(|(q, _)| reachable_symbols.contains(q))
                .collect(),
            decides: all_decides
                .into_iter()
                .filter(|(q, _)| reachable_symbols.contains(q))
                .collect(),
        };

        linked.validate_contracts()?;
        Ok(linked)
    }
}

/// A linked, pruned program ready for operator DAG generation and validation.
#[derive(Clone, Debug)]
pub struct LinkedProgram {
    pub root_module: String,
    pub root_items: Vec<Item>,
    pub configs: BTreeMap<QualifiedName, ast::ConfigDecl>,
    pub functions: BTreeMap<QualifiedName, ast::Callable>,
    pub rel_inputs: BTreeMap<QualifiedName, ast::RelInputDecl>,
    pub rel_derived: BTreeMap<QualifiedName, ast::RelDerivedDecl>,
    pub rules: BTreeMap<QualifiedName, ast::Callable>,
    pub decides: BTreeMap<QualifiedName, ast::DecideDecl>,
}

impl LinkedProgram {
    /// Validate helper contracts and schema interfaces across all reachable declarations
    /// before graph activation (ADR-0046 P2).
    pub fn validate_contracts(&self) -> Result<(), ModuleLinkError> {
        // 1. Validate schemas: field names must be unique within a record schema
        for (qname, config) in &self.configs {
            match &config.body {
                ast::ConfigBody::Record(fields) => {
                    let mut seen_fields = BTreeSet::new();
                    for field in fields {
                        if !seen_fields.insert(&field.name) {
                            return Err(ModuleLinkError::InvalidSchema {
                                module: qname.module.clone(),
                                name: qname.name.clone(),
                                reason: format!("duplicate field '{}' in schema", field.name),
                            });
                        }
                    }
                }
                ast::ConfigBody::Sum(variants) => {
                    let mut seen_variants = BTreeSet::new();
                    for variant in variants {
                        if !seen_variants.insert(&variant.name) {
                            return Err(ModuleLinkError::InvalidSchema {
                                module: qname.module.clone(),
                                name: qname.name.clone(),
                                reason: format!("duplicate variant '{}' in enum", variant.name),
                            });
                        }
                    }
                }
            }
        }

        // 2. Validate helper functions: parameters must have distinct names
        for (qname, func) in &self.functions {
            let mut seen_params = BTreeSet::new();
            for param in &func.params {
                if !seen_params.insert(&param.name) {
                    return Err(ModuleLinkError::InvalidHelper {
                        module: qname.module.clone(),
                        name: qname.name.clone(),
                        reason: format!(
                            "duplicate parameter '{}' in function contract",
                            param.name
                        ),
                    });
                }
            }
        }

        // 3. Validate relational input schemas: key fields must not be empty and must exist in schema
        for (qname, rel_in) in &self.rel_inputs {
            if rel_in.key_fields.is_empty() {
                return Err(ModuleLinkError::InvalidSchema {
                    module: qname.module.clone(),
                    name: qname.name.clone(),
                    reason: "relational input schema must declare at least one key field"
                        .to_string(),
                });
            }
            if let ast::Ty::Record(fields) = &rel_in.ty {
                for key_field in &rel_in.key_fields {
                    if !fields.iter().any(|f| &f.name == key_field) {
                        return Err(ModuleLinkError::InvalidSchema {
                            module: qname.module.clone(),
                            name: qname.name.clone(),
                            reason: format!(
                                "key field '{key_field}' is missing from relational schema"
                            ),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

/// Compute the set of modules affected by changes between two manifests.
///
/// A module is affected if:
/// 1. Its source digest changed (or was added/removed).
/// 2. Or it directly or transitively imports a module whose *interface_digest* changed.
///
/// This reports *compilation interface* invalidation only. A helper body change
/// can preserve its signature while changing every dependent runtime result:
/// execution caches must separately track transitive implementation digests.
/// This set is not safe for deciding which world outputs to retain.
pub fn compute_affected_modules(
    old_manifest: &ProgramGraphManifest,
    new_manifest: &ProgramGraphManifest,
) -> BTreeSet<String> {
    let mut directly_changed_source = BTreeSet::new();
    let mut interface_changed = BTreeSet::new();

    // Check additions and modifications
    for (name, new_entry) in &new_manifest.modules {
        match old_manifest.modules.get(name) {
            None => {
                directly_changed_source.insert(name.clone());
                interface_changed.insert(name.clone());
            }
            Some(old_entry) => {
                if old_entry.source_digest != new_entry.source_digest {
                    directly_changed_source.insert(name.clone());
                }
                if old_entry.interface_digest != new_entry.interface_digest {
                    interface_changed.insert(name.clone());
                }
            }
        }
    }

    // Check deletions
    for name in old_manifest.modules.keys() {
        if !new_manifest.modules.contains_key(name) {
            interface_changed.insert(name.clone());
        }
    }

    // Build reverse dependency graph (imported -> importers)
    let mut reverse_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (importer, entry) in &new_manifest.modules {
        for imported in &entry.direct_imports {
            reverse_deps
                .entry(imported.clone())
                .or_default()
                .push(importer.clone());
        }
    }

    // Propagate invalidation ONLY along interface changes
    let mut affected = directly_changed_source;
    let mut queue: Vec<String> = interface_changed.into_iter().collect();
    let mut visited: BTreeSet<String> = BTreeSet::new();

    while let Some(changed_iface) = queue.pop() {
        if !visited.insert(changed_iface.clone()) {
            continue;
        }
        if let Some(importers) = reverse_deps.get(&changed_iface) {
            for imp in importers {
                affected.insert(imp.clone());
                queue.push(imp.clone());
            }
        }
    }

    affected
}

fn extract_exported_symbol(item: &Item) -> Option<ExportedSymbol> {
    match item {
        Item::Config(c) => Some(ExportedSymbol {
            name: c.name.clone(),
            kind: ExportKind::Config,
            signature: format!("config {} {:?} {:?}", c.name, c.params, c.body),
        }),
        Item::Fn(f) => {
            let params_sig: Vec<String> = f
                .params
                .iter()
                .map(|p| format!("{}:{:?}", p.name, p.ty))
                .collect();
            Some(ExportedSymbol {
                name: f.name.clone(),
                kind: ExportKind::Fn,
                signature: format!("fn({}):{:?}", params_sig.join(","), f.ret),
            })
        }
        Item::RelInput(r) => Some(ExportedSymbol {
            name: r.name.clone(),
            kind: ExportKind::RelInput,
            signature: format!("rel input {}:{:?} key {:?}", r.name, r.ty, r.key_fields),
        }),
        Item::RelDerived(r) => Some(ExportedSymbol {
            name: r.name.clone(),
            kind: ExportKind::RelDerived,
            // Until output types are inferred, conservatively invalidate the
            // interface when the expression changes; never miss schema edits.
            signature: format!("rel derived {} {:?}", r.name, r.query.select),
        }),
        Item::Rule(r) => Some(ExportedSymbol {
            name: r.name.clone(),
            kind: ExportKind::Rule,
            signature: format!("rule {}", r.name),
        }),
        Item::Let(l) => Some(ExportedSymbol {
            name: l.name.clone(),
            kind: ExportKind::Let,
            signature: format!("let {}:{:?}", l.name, l.ty),
        }),
        _ => None,
    }
}

fn compute_interface_digest(symbols: &[ExportedSymbol]) -> Digest {
    let mut w = CanonWriter::new();
    w.write_ident("interface/1");
    for s in symbols {
        w.write_ident(&s.name);
        w.write_uint(s.kind as u64);
        w.write_str(&s.signature);
    }
    w.digest(Domain::Value)
}

fn trace_rel_query_deps(
    query: &ast::RelQuery,
    current_module: &str,
    reachable: &mut BTreeSet<QualifiedName>,
) {
    trace_expr_deps(&query.select, current_module, reachable);
    for binding in &query.from {
        let q = resolve_symbol_ref(&binding.relation, current_module);
        reachable.insert(q);
    }
    if let Some(w) = &query.where_clause {
        trace_expr_deps(w, current_module, reachable);
    }
    for g in &query.group_by {
        trace_expr_deps(g, current_module, reachable);
    }
}

fn trace_config_deps(
    config: &ast::ConfigDecl,
    current_module: &str,
    reachable: &mut BTreeSet<QualifiedName>,
) {
    match &config.body {
        ast::ConfigBody::Record(fields) => {
            for f in fields {
                trace_ty_deps(&f.ty, current_module, reachable);
            }
        }
        ast::ConfigBody::Sum(variants) => {
            for v in variants {
                for p in &v.params {
                    trace_ty_deps(p, current_module, reachable);
                }
            }
        }
    }
}

fn trace_ty_deps(ty: &ast::Ty, current_module: &str, reachable: &mut BTreeSet<QualifiedName>) {
    match ty {
        ast::Ty::Named(name) => {
            let q = resolve_symbol_ref(name, current_module);
            reachable.insert(q);
        }
        ast::Ty::Graded(inner, _) => trace_ty_deps(inner, current_module, reachable),
        ast::Ty::Record(fields) => {
            for f in fields {
                trace_ty_deps(&f.ty, current_module, reachable);
            }
        }
        ast::Ty::App(name, args) => {
            let q = resolve_symbol_ref(name, current_module);
            reachable.insert(q);
            for a in args {
                trace_ty_deps(a, current_module, reachable);
            }
        }
    }
}

fn trace_expr_deps(
    expr: &ast::Expr,
    current_module: &str,
    reachable: &mut BTreeSet<QualifiedName>,
) {
    match expr {
        ast::Expr::Var(name) => {
            let q = resolve_symbol_ref(name, current_module);
            reachable.insert(q);
        }
        ast::Expr::Call { func, args } => {
            let q = resolve_symbol_ref(func, current_module);
            reachable.insert(q);
            for a in args {
                trace_expr_deps(a, current_module, reachable);
            }
        }
        ast::Expr::Record { config, fields } => {
            let q = resolve_symbol_ref(config, current_module);
            reachable.insert(q);
            for (_, f) in fields {
                trace_expr_deps(f, current_module, reachable);
            }
        }
        ast::Expr::AnonRecord(fields) => {
            for (_, f) in fields {
                trace_expr_deps(f, current_module, reachable);
            }
        }
        ast::Expr::Field(base, _) => trace_expr_deps(base, current_module, reachable),
        ast::Expr::Bin { lhs, rhs, .. } => {
            trace_expr_deps(lhs, current_module, reachable);
            trace_expr_deps(rhs, current_module, reachable);
        }
        ast::Expr::Match {
            scrutinee, arms, ..
        } => {
            trace_expr_deps(scrutinee, current_module, reachable);
            for arm in arms {
                trace_expr_deps(&arm.body, current_module, reachable);
            }
        }
        ast::Expr::Not(inner)
        | ast::Expr::Prove(inner)
        | ast::Expr::Why(inner)
        | ast::Expr::Audit(inner) => trace_expr_deps(inner, current_module, reachable),
        ast::Expr::Lambda { body, .. } => trace_expr_deps(body, current_module, reachable),
        ast::Expr::ListLit(items) => {
            for item in items {
                trace_expr_deps(item, current_module, reachable);
            }
        }
        ast::Expr::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            for (_, gen) in generators {
                trace_expr_deps(gen, current_module, reachable);
            }
            if let Some(w) = where_clause {
                trace_expr_deps(w, current_module, reachable);
            }
            trace_expr_deps(yield_expr, current_module, reachable);
        }
        ast::Expr::Num(_) | ast::Expr::Str(_) | ast::Expr::Bool(_) => {}
    }
}

fn resolve_symbol_ref(symbol: &str, current_module: &str) -> QualifiedName {
    if let Some((mod_part, name_part)) = symbol.rsplit_once("::") {
        QualifiedName::new(mod_part, name_part)
    } else {
        QualifiedName::new(current_module, symbol)
    }
}

fn declaration_name(item: &Item) -> Option<&str> {
    match item {
        Item::Config(x) => Some(&x.name),
        Item::Fn(x) | Item::Rule(x) => Some(&x.name),
        Item::RelInput(x) => Some(&x.name),
        Item::RelDerived(x) => Some(&x.name),
        Item::Decide(x) => Some(&x.name),
        Item::Let(x) => Some(&x.name),
        _ => None,
    }
}

fn trace_callable_deps(callable: &ast::Callable, module: &str, deps: &mut BTreeSet<QualifiedName>) {
    trace_expr_deps(&callable.body, module, deps);
    for param in &callable.params {
        if let Some(ty) = &param.ty {
            trace_ty_deps(ty, module, deps);
        }
    }
    if let Some(ty) = &callable.ret {
        trace_ty_deps(ty, module, deps);
    }
}

fn trace_item_deps(item: &Item, module: &str, deps: &mut BTreeSet<QualifiedName>) {
    match item {
        Item::Fn(callable) | Item::Rule(callable) => trace_callable_deps(callable, module, deps),
        Item::Config(config) => trace_config_deps(config, module, deps),
        Item::RelInput(input) => trace_ty_deps(&input.ty, module, deps),
        Item::RelDerived(derived) => trace_rel_query_deps(&derived.query, module, deps),
        Item::Let(binding) => {
            trace_expr_deps(&binding.value, module, deps);
            if let Some(ty) = &binding.ty {
                trace_ty_deps(ty, module, deps);
            }
        }
        Item::Show(expression) => trace_expr_deps(expression, module, deps),
        Item::Decide(decide) => {
            trace_expr_deps(&decide.list, module, deps);
            for proposal in &decide.proposals {
                trace_expr_deps(&proposal.guard, module, deps);
                trace_expr_deps(&proposal.value, module, deps);
            }
        }
        Item::Propose(proposal) => {
            trace_expr_deps(&proposal.guard, module, deps);
            trace_expr_deps(&proposal.value, module, deps);
        }
        _ => {}
    }
}
