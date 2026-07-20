//! Shared "parse + lower a whole located package" plumbing (issue #42): the
//! seam every verb that needs a checked program (`check`/`build`/`test`/
//! `quality`) goes through, so a multi-file package (local submodules,
//! resolved dependencies — themselves possibly multi-file) is exactly one
//! program everywhere, not "`build` sees the graph, everything else sees one
//! file" (the pre-#42 gap `packages/STATUS.md` called out).

use brix_ast::{parse_file, Diagnostics, File};
use brixc::{DepPackage, PackageLowered, SubmoduleInput};
use camino::Utf8Path;

use crate::package::LocatedPackage;

/// One parsed source: its own text (diagnostics render against this) plus
/// the parsed AST and that parse's diagnostics.
pub struct ParsedFile {
    pub source: String,
    pub file: File,
    pub diags: Diagnostics,
}

impl ParsedFile {
    fn read(path: &Utf8Path) -> std::io::Result<Self> {
        let source = std::fs::read_to_string(path)?;
        let (file, diags) = parse_file(&source);
        Ok(Self { source, file, diags })
    }

    fn from_source(source: String) -> Self {
        let (file, diags) = parse_file(&source);
        Self { source, file, diags }
    }
}

/// One dependency's parsed entry plus its own local submodules (issue #42: a
/// dependency may itself be a multi-file package).
pub struct ParsedDep {
    pub name_segments: Vec<String>,
    pub entry: ParsedFile,
    pub submodules: Vec<(String, ParsedFile)>,
}

/// A [`LocatedPackage`], fully parsed: entry, local submodules (each tagged
/// with its module qualifier), and the parsed dependency graph. Built once
/// per verb invocation and handed to [`lower`] (and, for verbs that also
/// inspect the AST directly — `quality`'s decl-coverage rule, `fmt`'s
/// canonical-format rule — [`local_files`]).
pub struct ParsedPackage {
    pub entry: ParsedFile,
    pub submodules: Vec<(String, ParsedFile)>,
    pub deps: Vec<ParsedDep>,
}

pub fn parse(located: &LocatedPackage) -> std::io::Result<ParsedPackage> {
    let entry = ParsedFile::read(&located.source_path)?;
    let submodules = located
        .submodules
        .iter()
        .map(|s| (s.qualifier.clone(), ParsedFile::from_source(s.source.clone())))
        .collect();
    let deps = located
        .deps
        .iter()
        .map(|dep| ParsedDep {
            name_segments: dep.name_segments.clone(),
            entry: ParsedFile::from_source(dep.source.clone()),
            submodules: dep
                .submodules
                .iter()
                .map(|s| (s.qualifier.clone(), ParsedFile::from_source(s.source.clone())))
                .collect(),
        })
        .collect();
    Ok(ParsedPackage {
        entry,
        submodules,
        deps,
    })
}

/// Lower the whole parsed package (entry + local submodules + dependency
/// graph, each dependency's own submodules included) into one checked
/// program. `entry_label` is the path `brix check`/`build` should attribute
/// entry-file (and dependency-graph) diagnostics to — normally
/// `located.source_path`.
pub fn lower(parsed: &ParsedPackage, entry_label: &str) -> PackageLowered {
    let submodule_inputs: Vec<SubmoduleInput> = parsed
        .submodules
        .iter()
        .map(|(qualifier, pf)| SubmoduleInput {
            qualifier: qualifier.clone(),
            file: &pf.file,
            parse_diags: &pf.diags,
        })
        .collect();
    let dep_submodule_inputs: Vec<Vec<SubmoduleInput>> = parsed
        .deps
        .iter()
        .map(|dep| {
            dep.submodules
                .iter()
                .map(|(qualifier, pf)| SubmoduleInput {
                    qualifier: qualifier.clone(),
                    file: &pf.file,
                    parse_diags: &pf.diags,
                })
                .collect()
        })
        .collect();
    let dep_packages: Vec<DepPackage> = parsed
        .deps
        .iter()
        .zip(dep_submodule_inputs.iter())
        .map(|(dep, submodules)| DepPackage {
            name_segments: dep.name_segments.clone(),
            file: &dep.entry.file,
            parse_diags: &dep.entry.diags,
            submodules: submodules.as_slice(),
        })
        .collect();
    brixc::lower_program(
        &parsed.entry.file,
        &parsed.entry.diags,
        entry_label,
        &dep_packages,
        &submodule_inputs,
    )
}

/// Every **local** file (entry + submodules — never dependencies), labeled
/// the same way [`PackageLowered::reports`] labels them, for verbs that walk
/// the AST/source directly rather than the lowered IR (`quality`'s
/// decl-coverage and canonical-format rules).
pub fn local_files<'a>(
    parsed: &'a ParsedPackage,
    entry_label: &str,
) -> Vec<(String, &'a ParsedFile)> {
    let mut files = vec![(entry_label.to_string(), &parsed.entry)];
    for (qualifier, pf) in &parsed.submodules {
        files.push((format!("src/{qualifier}.brix"), pf));
    }
    files
}
