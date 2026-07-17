//! `brix new` — the package skeleton.
//!
//! Emits a real, minimal package including an `OWNER.md` template
//! (Ring0_Build_Plan §1.9: "`brix new` (package skeleton + OWNER.md template)").
//! The Ring 0 discipline of *one OWNER per lane* mirrors down to Ring 1: every
//! package a developer creates gets an ownership + discipline doc from the first
//! commit (Part XXII §22.3 "Ownership and modularity"), so it is never bolted on
//! later.
//!
//! The scaffold is produced as an in-memory `path -> bytes` map so it is testable
//! without touching disk; [`write_skeleton`] fans it out to a directory.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};

/// Build the file set for a new package named `name` (a dotted qualified
/// identifier, e.g. `demo.logistics`). Paths are relative to the new package
/// root. The module name is derived from the last name segment, PascalCased.
pub fn skeleton(name: &str) -> BTreeMap<Utf8PathBuf, String> {
    let module = module_name(name);
    let mut files: BTreeMap<Utf8PathBuf, String> = BTreeMap::new();

    files.insert(
        Utf8PathBuf::from("brix.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nauthors = []\n\n\
             [dependencies]\n"
        ),
    );

    files.insert(
        Utf8PathBuf::from("src").join("world.brix"),
        format!(
            "package {name} @ 0.1.0\n\
             module {module}\n\
             \n\
             // Structure — what exists.\n\
             entity Widget {{ key code: String }}\n\
             \n\
             // A scenario drives every rule at least once (Part I).\n\
             scenario Smoke {{\n\
             \x20\x20seed 1\n\
             \x20\x20setup {{ ensure Widget {{ code: \"w1\" }} }}\n\
             \x20\x20assert at end {{ true }}\n\
             }}\n"
        ),
    );

    files.insert(Utf8PathBuf::from("OWNER.md"), owner_template(name));

    files.insert(
        Utf8PathBuf::from(".gitignore"),
        "/target\n/.brix-cache\nbrix.lock.bak\n".to_string(),
    );

    files
}

/// The Ring 1 `OWNER.md` template. Parallels the Ring 0 lane OWNER.md structure
/// (Contract / Discipline) but scoped to a package a developer owns.
fn owner_template(name: &str) -> String {
    format!(
        "# OWNER — {name}\n\
         \n\
         **Package:** `{name}`\n\
         **Owner:** <your name / team>\n\
         **Support level:** Community package (Part XIII §1)\n\
         \n\
         ## Contract\n\
         \n\
         What this package models and the public surface it promises: which\n\
         relations are `pub read` / `pub write` / `pub derive`, which protocols and\n\
         tools it exposes, and the identity domains it commits to keeping stable\n\
         across semver (Part XXVIII §28.3).\n\
         \n\
         ## Discipline\n\
         \n\
         - `brix fmt` is canonical and non-configurable; run it before every commit.\n\
         - `brix test` (scenarios + doctests) is the merge bar.\n\
         - `brix why` / `brix whynot` answer in graph terms — prefer them to `println`\n\
           debugging.\n\
         - Dependencies are pinned in `brix.lock`; upgrade the toolchain and deps\n\
           deliberately, never ambiently (Part XIII, CONTRIBUTING feedback protocol).\n\
         \n\
         ## Feedback\n\
         \n\
         A failure is a package bug (fix here), a toolchain bug (minimal repro as a\n\
         fixture, file upstream), or a spec ambiguity (erratum). Triage into exactly\n\
         one bin.\n"
    )
}

/// Derive a PascalCase module name from a package name's last segment.
fn module_name(package: &str) -> String {
    let last = package.rsplit('.').next().unwrap_or(package);
    let mut out = String::with_capacity(last.len());
    let mut capitalize = true;
    for ch in last.chars() {
        if ch == '_' || ch == '-' {
            capitalize = true;
        } else if capitalize {
            out.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "World".to_string()
    } else {
        out
    }
}

/// Errors writing a skeleton to disk.
#[derive(Debug)]
pub enum ScaffoldError {
    /// The target directory already exists and is non-empty.
    TargetNotEmpty {
        path: Utf8PathBuf,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldError::TargetNotEmpty { path } => {
                write!(f, "target directory {path} already exists and is not empty")
            }
            ScaffoldError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for ScaffoldError {}

impl From<std::io::Error> for ScaffoldError {
    fn from(e: std::io::Error) -> Self {
        ScaffoldError::Io(e)
    }
}

/// Write the skeleton for `name` into `root`, creating directories as needed.
/// Refuses to write into a non-empty existing directory.
pub fn write_skeleton(root: &Utf8Path, name: &str) -> Result<(), ScaffoldError> {
    if root.exists() {
        let mut entries = std::fs::read_dir(root)?;
        if entries.next().is_some() {
            return Err(ScaffoldError::TargetNotEmpty {
                path: root.to_path_buf(),
            });
        }
    }
    for (rel, contents) in skeleton(name) {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_includes_manifest_source_and_owner() {
        let files = skeleton("demo.logistics");
        assert!(files.contains_key(&Utf8PathBuf::from("brix.toml")));
        assert!(files.contains_key(&Utf8PathBuf::from("src/world.brix")));
        assert!(files.contains_key(&Utf8PathBuf::from("OWNER.md")));
        assert!(files.contains_key(&Utf8PathBuf::from(".gitignore")));
    }

    #[test]
    fn manifest_names_the_package() {
        let files = skeleton("demo.logistics");
        let manifest = &files[&Utf8PathBuf::from("brix.toml")];
        assert!(manifest.contains("name = \"demo.logistics\""));
        // The generated manifest must parse and round-trip through brixpkg.
        let parsed = brixpkg::Manifest::parse(manifest).unwrap();
        assert_eq!(parsed.name.as_str(), "demo.logistics");
        assert_eq!(parsed.version.to_string(), "0.1.0");
    }

    #[test]
    fn source_declaration_matches_manifest() {
        let files = skeleton("demo.logistics");
        let src = &files[&Utf8PathBuf::from("src/world.brix")];
        // The `package NAME @ VERSION` line must agree with the manifest — this
        // is exactly the cross-check brixpkg enforces at build time.
        assert!(src.contains("package demo.logistics @ 0.1.0"));
        assert!(src.contains("module Logistics"));
    }

    #[test]
    fn owner_template_is_present_and_scoped() {
        let files = skeleton("demo.logistics");
        let owner = &files[&Utf8PathBuf::from("OWNER.md")];
        assert!(owner.contains("# OWNER — demo.logistics"));
        assert!(owner.contains("## Contract"));
        assert!(owner.contains("## Discipline"));
    }

    #[test]
    fn module_name_pascalcases_last_segment() {
        assert_eq!(module_name("demo.logistics"), "Logistics");
        assert_eq!(module_name("my_app"), "MyApp");
        assert_eq!(module_name("world"), "World");
    }
}
