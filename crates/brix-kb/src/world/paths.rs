//! Filesystem layout paths for a `brix.world@1` directory (ADR-0046, P3).

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct WorldPaths {
    pub root: PathBuf,
}

impl WorldPaths {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn world_json(&self) -> PathBuf {
        self.root.join("world.json")
    }

    pub fn head(&self) -> PathBuf {
        self.root.join("HEAD")
    }

    pub fn head_tmp(&self) -> PathBuf {
        self.root.join(".HEAD.tmp")
    }

    pub fn lock(&self) -> PathBuf {
        self.root.join(".lock")
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    pub fn revisions_dir(&self) -> PathBuf {
        self.root.join("revisions")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    pub fn revision_file(&self, seq: u64) -> PathBuf {
        self.revisions_dir().join(format!("{seq}.json"))
    }

    pub fn revision_tmp_file(&self, seq: u64) -> PathBuf {
        self.revisions_dir().join(format!(".{seq}.tmp"))
    }

    /// Durable body of one revision's decision delta (ADR-0046 P6 G2/G3,
    /// decided 2026-10-04): the canonical encoding whose digest is the
    /// revision record's `decision_delta_digest`. Written and fsynced before
    /// the revision record itself (`WorldSession::apply_batch`).
    pub fn decision_delta_file(&self, seq: u64) -> PathBuf {
        self.revisions_dir().join(format!("{seq}.decisions"))
    }

    pub fn decision_delta_tmp_file(&self, seq: u64) -> PathBuf {
        self.revisions_dir().join(format!(".{seq}.decisions.tmp"))
    }

    pub fn staging_upload_dir(&self, upload_id: &str) -> PathBuf {
        self.staging_dir().join(upload_id)
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(self.objects_dir())?;
        fs::create_dir_all(self.revisions_dir())?;
        fs::create_dir_all(self.staging_dir())?;
        Ok(())
    }
}
