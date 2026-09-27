//! On-disk layout of a knowledge base directory (ADR-0041).
//!
//! ```text
//! <root>/
//!   kb.json                  manifest, schema brix.kb@1
//!   HEAD                     current revision pointer, schema brix.kb.head@1
//!   .lock                    present while a writer holds the knowledge base
//!   programs/<program-id>.brix     content-addressed program sources
//!   snapshots/<snapshot-id>.json   content-addressed input snapshots (brix.input@2)
//!   revisions/<seq>.json           immutable revision records, schema brix.kb.revision@1
//! ```

use std::path::{Path, PathBuf};

use brix_canon::Digest;

pub fn kb_json(root: &Path) -> PathBuf {
    root.join("kb.json")
}

pub fn head(root: &Path) -> PathBuf {
    root.join("HEAD")
}

pub fn lock(root: &Path) -> PathBuf {
    root.join(".lock")
}

pub fn programs_dir(root: &Path) -> PathBuf {
    root.join("programs")
}

pub fn snapshots_dir(root: &Path) -> PathBuf {
    root.join("snapshots")
}

pub fn revisions_dir(root: &Path) -> PathBuf {
    root.join("revisions")
}

pub fn program_file(root: &Path, id: Digest) -> PathBuf {
    programs_dir(root).join(format!("{}.brix", id.to_hex()))
}

/// The path stored *inside* a revision record, relative to the knowledge
/// base root (never the absolute path — a knowledge base is portable).
pub fn program_rel_path(id: Digest) -> String {
    format!("programs/{}.brix", id.to_hex())
}

pub fn snapshot_file(root: &Path, id: Digest) -> PathBuf {
    snapshots_dir(root).join(format!("{}.json", id.to_hex()))
}

pub fn snapshot_rel_path(id: Digest) -> String {
    format!("snapshots/{}.json", id.to_hex())
}

pub fn revision_file(root: &Path, seq: u64) -> PathBuf {
    revisions_dir(root).join(format!("{seq}.json"))
}
