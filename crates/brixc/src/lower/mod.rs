//! AST → Core-IR lowering (issue #6; design ruling: see the PR description).
//!
//! ```text
//! source -parse_file-> (ast::File, ast::Diagnostics)   [total, never Err]
//!        -lower_file-> Lowered { source, resolver, meta, diags }
//!        -check-> diags ∪ rendered ir::check::Finding    [run inside Lower stage]
//! ```
//!
//! Two passes (design §"Pass 1"/"Pass 2"): [`schema`] walks `File.uses` +
//! `File.decls` into the [`resolve::ProgramResolver`]'s schema tables
//! (relations/fns/enums/aliases/units/imports), never looking inside a rule
//! body; [`decl`] then walks the same decls again, lowering `derive`/
//! `constraint`/`query` bodies into Core IR (via [`expr`]/[`tymap`]) and
//! dispatching the v0 defer line for everything else. [`lower_file`] runs
//! both passes, then folds `brix_ir::check::Finding`s (from
//! `check_relation_keys` over every schema and `check_rule` over every
//! `Rule`) into the one diagnostic channel ([`diag`]).

mod decl;
mod diag;
mod expr;
mod resolve;
mod schema;
mod tymap;

pub use resolve::{FnInfo, LowerMeta, ProgramResolver, UnitClass, VariantLookup};

use brix_ast::File;
use brix_diag::{Diagnostic, Severity};
use brix_ir::check::{check_relation_keys, check_rule};
use brix_ir::frontend::FrontendSource;
use brix_ir::infer::infer_source;

use crate::pipeline::{Frontend, Lower, PipelineError};

/// The whole-program lowering output: Core IR ([`FrontendSource`]) plus the
/// resolver it type-checks against, the v0 side tables ([`LowerMeta`]), and
/// the one diagnostic channel (design §"Error strategy": parse diags ++
/// lowering diags (decl order) ++ rendered `Finding`s (decl order)).
#[derive(Default)]
pub struct Lowered {
    pub source: FrontendSource,
    pub resolver: ProgramResolver,
    pub meta: LowerMeta,
    pub diags: Vec<Diagnostic>,
}

impl Lowered {
    /// The one gate between a (possibly poisoned) `Lowered` and any
    /// downstream stage (design: "`Lowered::has_errors()` gate ... is the
    /// ONLY thing between poisoned IR and downstream").
    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }
}

/// Lower an already-parsed file. `parse_diags` rides first in the returned
/// channel (source order), then lowering diagnostics (decl order), then
/// rendered static-semantics `Finding`s (decl order).
pub fn lower_file(file: &File, parse_diags: &brix_ast::Diagnostics) -> Lowered {
    let mut diags: Vec<Diagnostic> = parse_diags.iter().cloned().collect();
    let mut meta = LowerMeta::default();

    let resolver = schema::build(file, &mut meta, &mut diags);
    let mut source = decl::lower_decls(file, &resolver, &mut meta, &mut diags);

    for schema in resolver.relations() {
        for finding in check_relation_keys(schema) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }
    for rule in &source.rules {
        for finding in check_rule(rule, &resolver) {
            diags.push(diag::render_finding(&finding, &meta));
        }
    }
    for error in infer_source(&mut source, &resolver) {
        diags.push(diag::render_type_error(&error));
    }

    Lowered {
        source,
        resolver,
        meta,
        diags,
    }
}

/// The `Frontend` seam (design §"Seams"): parsing is total, so this is
/// always `Ok`; parse diagnostics ride inside the artifact, not the
/// `Result`.
pub struct AstFrontend;

impl Frontend for AstFrontend {
    type Ast = (File, brix_ast::Diagnostics);
    fn parse(&self, source: &str) -> Result<Self::Ast, PipelineError> {
        Ok(brix_ast::parse_file(source))
    }
}

/// The `Lower` seam: lowering is also total (never `Err`) — a decl that
/// can't be lowered degrades to a skip/error diagnostic, never a `Result`
/// short-circuit. `PhaseAssign` (App. F) is the stage that refuses to run
/// when [`Lowered::has_errors`].
pub struct AstLower;

impl Lower for AstLower {
    type Ast = (File, brix_ast::Diagnostics);
    type Ir = Lowered;
    fn lower(&self, ast: Self::Ast) -> Result<Self::Ir, PipelineError> {
        let (file, parse_diags) = ast;
        Ok(lower_file(&file, &parse_diags))
    }
}
