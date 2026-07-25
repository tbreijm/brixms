//! Standard library prelude packages loader for BrixMS compiler.

use super::resolve::{LowerMeta, ProgramResolver};
use super::schema;
use super::tymap::{lower_type, TyPos};
use brix_ast::parse_file;
use brix_ir::frontend::FnSignature;
use brix_ir::ident::{Ident as IrIdent, QualIdent};
use std::sync::OnceLock;

pub const BRIX_CORE_SRC: &str = include_str!("../../../../packages/brix.core/src/core.brix");
pub const BRIX_TY_SRC: &str = include_str!("../../../../packages/brix.core/src/ty.brix");
pub const BRIX_CANON_SRC: &str = include_str!("../../../../packages/brix.core/src/canon.brix");
pub const BRIX_MATH_SRC: &str = include_str!("../../../../packages/brix.math/src/math.brix");
pub const BRIX_SIM_SRC: &str = include_str!("../../../../packages/brix.sim/src/sim.brix");
pub const BRIX_OPS_SRC: &str = include_str!("../../../../packages/brix.ops/src/ops.brix");

pub fn stdlib_resolver() -> &'static ProgramResolver {
    static STDLIB: OnceLock<ProgramResolver> = OnceLock::new();
    STDLIB.get_or_init(|| {
        let mut meta = LowerMeta::default();
        let mut diags = Vec::new();
        let mut resolver = ProgramResolver::new();

        // 1. Bare core symbols (units, ValidationError, Probability.try, count)
        let (core_file, cdiags) = parse_file(BRIX_CORE_SRC);
        assert!(!cdiags.has_errors(), "core.brix parses cleanly");
        resolver = schema::build_onto(&core_file, resolver, &mut meta, &mut diags);

        // 2. Package-qualified stdlib dependencies
        let stdlib_deps = [
            (vec!["brix", "math"], BRIX_MATH_SRC),
            (vec!["brix", "sim"], BRIX_SIM_SRC),
            (vec!["brix", "ops"], BRIX_OPS_SRC),
            (vec!["brix", "ty"], BRIX_TY_SRC),
            (vec!["brix", "canon"], BRIX_CANON_SRC),
        ];

        for (pkg_path, src) in stdlib_deps {
            let (file, pdiags) = parse_file(src);
            assert!(!pdiags.has_errors(), "stdlib dep parses cleanly");
            let dep_resolver =
                schema::build_onto(&file, ProgramResolver::new(), &mut meta, &mut diags);

            let qualify_path = |segments: &[IrIdent]| -> QualIdent {
                let mut segs: Vec<IrIdent> = pkg_path
                    .iter()
                    .map(|s| IrIdent::new((*s).to_string()))
                    .collect();
                segs.extend(segments.iter().cloned());
                QualIdent::from_segments(segs)
            };

            for schema in dep_resolver.relations() {
                let qname = qualify_path(schema.name.segments());
                let kind = dep_resolver.relation_kind(&schema.name);
                let mut qschema = schema.clone();
                qschema.name = qname.clone();
                resolver = resolver
                    .with_relation(qschema)
                    .with_relation_kind(qname, kind);
            }

            for ent in dep_resolver.entities() {
                resolver = resolver.with_entity(qualify_path(ent.segments()));
            }

            for (name, variants) in dep_resolver.enums() {
                resolver = resolver.with_enum(qualify_path(name.segments()), variants.to_vec());
            }

            for (name, ty) in dep_resolver.aliases() {
                resolver = resolver.with_alias(qualify_path(name.segments()), ty.clone());
            }

            for f in file.decls.iter().filter_map(|d| match d {
                brix_ast::ast::Decl::Fn(f) => Some(f),
                _ => None,
            }) {
                let qname = qualify_path(&[IrIdent::new(f.name.text.clone())]);
                let params = f
                    .params
                    .iter()
                    .map(|p| lower_type(&p.ty, TyPos::FnSig, &resolver, &mut meta, &mut diags))
                    .collect();
                let ret = lower_type(&f.ret, TyPos::FnSig, &resolver, &mut meta, &mut diags);
                resolver = resolver.with_function(FnSignature {
                    name: qname,
                    params,
                    ret,
                    may_diverge: false,
                    effects: brix_ir::effects::EffectRow::empty(),
                    is_aggregate: f.aggregate,
                });
            }
        }

        assert!(
            diags.is_empty(),
            "stdlib lowering has no errors: {:#?}",
            diags
        );
        resolver
    })
}
