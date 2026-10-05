//! Library session API for the persistent world runtime (ADR-0046 §3.2, P3).

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::{lower_relations, OperatorNode};
use soc_core::store::{FileNodeStore, TrieMap};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::batch::{WorldBatch, WorldBatchOp};
use super::codec::{encode_secondary_key, extract_indexed_field, TupleRecord};
use super::error::{CrashPoint, WorldError};
use super::manifest::{RelationDecl, WorldManifest, WORLD_PROFILE};
use super::network::{DecisionExplanation, SettledDecision, WorldNetwork};
use super::oracle::DiffEvent;
use super::paths::WorldPaths;
use super::revision::{SettlementStatus, WorldRevision};
use super::staging::StagingManager;
use super::types::{WorldCursor, WorldKey, WorldTuple};

fn parse_digest(hex: &str, field: &str) -> Result<Digest, WorldError> {
    if hex.len() != 64 {
        return Err(WorldError::Json(format!("{field} must be 64 hex chars")));
    }
    let mut bytes = [0u8; 32];
    let chars: Vec<char> = hex.chars().collect();
    for i in (0..64).step_by(2) {
        let hi = chars[i]
            .to_digit(16)
            .ok_or_else(|| WorldError::Json(format!("invalid hex in {field}")))?;
        let lo = chars[i + 1]
            .to_digit(16)
            .ok_or_else(|| WorldError::Json(format!("invalid hex in {field}")))?;
        bytes[i / 2] = ((hi << 4) | lo) as u8;
    }
    Ok(Digest::from_bytes(bytes))
}

fn link_closure(
    root_module: &str,
    sources: &BTreeMap<String, String>,
) -> Result<(LinkedProgram, Digest, BTreeMap<String, String>), WorldError> {
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load(root_module, &loader, ModuleLoaderLimits::default())
        .map_err(|e| WorldError::NetworkError(format!("module load error: {e}")))?;
    let linked = graph
        .link()
        .map_err(|e| WorldError::NetworkError(format!("module link error: {e}")))?;
    let graph_manifest = graph.manifest(WORLD_PROFILE);
    let digest = graph_manifest.transitive_manifest_digest;
    let mut closure = BTreeMap::new();
    for name in graph_manifest.modules.keys() {
        let source = sources
            .get(name)
            .ok_or_else(|| WorldError::NetworkError(format!("missing loaded source '{name}'")))?;
        closure.insert(name.clone(), source.clone());
    }
    Ok((linked, digest, closure))
}

fn validate_program_relations(
    manifest: &WorldManifest,
    network: &WorldNetwork,
) -> Result<(), WorldError> {
    let declared: std::collections::BTreeSet<_> = manifest.relations.keys().collect();
    let program: std::collections::BTreeSet<_> = network.base_relations.keys().collect();
    if declared != program {
        return Err(WorldError::InvalidSchema(format!(
            "world relations do not match executable program base relations (world: {:?}, program: {:?})",
            declared, program,
        )));
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), WorldError> {
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Outcome receipt returned upon applying a batch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevisionReceipt {
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub idempotency_key: String,
    pub is_idempotent_replay: bool,
    pub changed_keys_count: usize,
    pub objects_written: usize,
}

/// A paged query slice of records from a relation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QueryPage {
    pub relation: String,
    pub entries: Vec<(WorldKey, WorldTuple)>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Opaque continuation for one secondary-index query at one immutable revision.
///
/// Tokens are transport encodings, not authorization credentials. A token cannot be
/// reused for a different revision, relation, index, or encoded indexed value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SecondaryIndexCursor {
    revision: Digest,
    relation: String,
    index_field: String,
    indexed_value: WorldKey,
    position: WorldCursor,
}

impl SecondaryIndexCursor {
    pub fn to_token(&self) -> String {
        let mut writer = CanonWriter::new();
        writer.write_str("brix.secondary-cursor@1");
        writer.write_bytes(self.revision.as_bytes());
        writer.write_str(&self.relation);
        writer.write_str(&self.index_field);
        writer.write_bytes(self.indexed_value.as_bytes());
        writer.write_bytes(&self.position.hash);
        writer.write_bytes(self.position.key.as_bytes());
        WorldKey::new(writer.finish()).to_hex()
    }

    pub fn from_token(token: &str) -> Result<Self, WorldError> {
        let bytes = WorldKey::from_hex(token)?;
        let mut reader = CanonReader::new(bytes.as_bytes());
        let invalid = || WorldError::InvalidCursor("malformed secondary-index cursor".into());
        if reader.read_bytes().map_err(|_| invalid())? != b"brix.secondary-cursor@1" {
            return Err(invalid());
        }
        let revision: [u8; 32] = reader
            .read_bytes()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let relation = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_owned();
        let index_field = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_owned();
        let indexed_value = WorldKey::new(reader.read_bytes().map_err(|_| invalid())?.to_vec());
        let hash: [u8; 32] = reader
            .read_bytes()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let key = WorldKey::new(reader.read_bytes().map_err(|_| invalid())?.to_vec());
        if !reader.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            revision: Digest::from_bytes(revision),
            relation,
            index_field,
            indexed_value,
            position: WorldCursor::new(hash, key),
        })
    }
}

/// Bounded primary-key memberships returned by an equality secondary index.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SecondaryIndexPage {
    pub keys: Vec<WorldKey>,
    pub next_cursor: Option<SecondaryIndexCursor>,
}

fn secondary_index_page(
    indexes: &BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    store: &FileNodeStore,
    revision: Digest,
    query: (&str, &str, &WorldKey),
    cursor: Option<&SecondaryIndexCursor>,
    limit: usize,
) -> Result<SecondaryIndexPage, WorldError> {
    let (relation, index_field, indexed_value) = query;
    if limit == 0 {
        return Err(WorldError::InvalidSchema(
            "secondary-index page limit must be positive".into(),
        ));
    }
    if let Some(cursor) = cursor {
        if cursor.revision != revision
            || cursor.relation != relation
            || cursor.index_field != index_field
            || cursor.indexed_value != *indexed_value
        {
            return Err(WorldError::InvalidCursor(
                "secondary-index cursor query scope mismatch".into(),
            ));
        }
    }
    let sec_name = format!("{relation}:{index_field}");
    let index = indexes
        .get(&sec_name)
        .ok_or_else(|| WorldError::UnknownSecondaryIndex(sec_name.clone()))?;
    let Some(membership) = index.get_with_store(indexed_value, store)? else {
        return Ok(SecondaryIndexPage {
            keys: Vec::new(),
            next_cursor: None,
        });
    };
    let digest: [u8; 32] = membership
        .as_bytes()
        .try_into()
        .map_err(|_| WorldError::CorruptedObject(index.root_digest()))?;
    let members = TrieMap::<WorldKey, WorldTuple>::from_root_digest(Digest::from_bytes(digest), 0);
    let (entries, next) = members.iter_page_with_store(
        cursor.map(|c| (&c.position.hash, &c.position.key)),
        limit,
        store,
    )?;
    Ok(SecondaryIndexPage {
        keys: entries.into_iter().map(|(key, _)| key).collect(),
        next_cursor: next.map(|(hash, key)| SecondaryIndexCursor {
            revision,
            relation: relation.to_owned(),
            index_field: index_field.to_owned(),
            indexed_value: indexed_value.clone(),
            position: WorldCursor::new(hash, key),
        }),
    })
}

/// A paged diff slice of modifications between two revisions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffPage {
    pub relation: String,
    pub from_revision: u64,
    pub to_revision: u64,
    pub events: Vec<DiffEvent>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Cache entry for committed batch idempotency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedBatchInfo {
    pub seq: u64,
    pub revision_digest: Digest,
    pub batch_digest: Option<Digest>,
}

/// RAII guard representing exclusive writer ownership over a world directory.
pub struct WorldLockGuard {
    _file: fs::File,
}

impl WorldLockGuard {
    pub fn acquire(root: &Path) -> Result<Self, WorldError> {
        let lock_path = root.join(".lock");
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => match file.try_lock() {
                Ok(()) => Ok(Self { _file: file }),
                Err(fs::TryLockError::WouldBlock) => Err(WorldError::WorldLocked(format!(
                    "world '{}' is locked by another writer",
                    root.display()
                ))),
                Err(fs::TryLockError::Error(e)) => Err(WorldError::Io(e)),
            },
            Err(e) => Err(WorldError::Io(e)),
        }
    }
}

// The OS releases the lock when the handle closes, including process termination.
// Do not unlink the lock file: a replacement inode would allow a second writer.

/// An immutable read snapshot pinned to a specific revision.
#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    pub revision: WorldRevision,
    pub relations: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub secondary_indexes: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub node_store: FileNodeStore,
}

impl WorldSnapshot {
    pub fn revision_seq(&self) -> u64 {
        self.revision.seq
    }

    pub fn revision_digest(&self) -> Digest {
        self.revision.revision_digest
    }

    pub fn get(&self, relation: &str, key: &WorldKey) -> Result<Option<WorldTuple>, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;
        let val = trie.get_with_store(key, &self.node_store)?;
        Ok(val)
    }

    /// Collect every matching primary key. For bounded memory, use the paged API.
    /// `indexed_value` must be produced by [`encode_secondary_key`].
    pub fn query_secondary_index(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
    ) -> Result<Vec<WorldKey>, WorldError> {
        let mut keys = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.query_secondary_index_page(
                relation,
                index_field,
                indexed_value,
                cursor.as_ref(),
                1024,
            )?;
            keys.extend(page.keys);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(keys);
            }
        }
    }

    /// Read a bounded page using an explicitly encoded secondary key.
    /// Continuations stay valid on this pinned revision after the writer advances.
    pub fn query_secondary_index_page(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
        cursor: Option<&SecondaryIndexCursor>,
        limit: usize,
    ) -> Result<SecondaryIndexPage, WorldError> {
        secondary_index_page(
            &self.secondary_indexes,
            &self.node_store,
            self.revision.revision_digest,
            (relation, index_field, indexed_value),
            cursor,
            limit,
        )
    }

    pub fn query_page(
        &self,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let parsed_cursor = match cursor {
            Some(token) => Some(WorldCursor::from_token(token)?),
            None => None,
        };

        let (entries, next_pos) = trie.iter_page_with_store(
            parsed_cursor.as_ref().map(|c| (&c.hash, &c.key)),
            limit,
            &self.node_store,
        )?;

        let next_cursor = next_pos.map(|(hash, key)| WorldCursor::new(hash, key).to_token());
        let has_more = next_cursor.is_some();

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }
}

/// Schema tag for [`ExecProfileV1`] (ADR-0046 P6 §1.1).
pub const EXEC_PROFILE_SCHEMA: &str = "brix.world.exec@1";

/// Module-loader resource limits as recorded in [`ExecProfileV1`] — the same
/// four bounds as `brix_lower::module_graph::ModuleLoaderLimits`, named per
/// the P6 audit contract (`depth`/`modules`/`module_bytes`/`total_bytes`)
/// rather than that type's internal field names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExecModuleLoaderLimits {
    pub depth: usize,
    pub modules: usize,
    pub module_bytes: usize,
    pub total_bytes: usize,
}

/// Recorded, **non-authoritative** execution profile (`brix.world.exec@1`,
/// ADR-0046 P6 §1.1, decided 2026-10-04): written into `program.json` at
/// `world init` as an additive field (older files simply lack it — see
/// [`WorldSession::open`]). A verifier *compares* this to its own profile and
/// refuses on mismatch; it never adopts limits from an untrusted artifact
/// (ADR-0046 §3.8, "host operational limits must never be relaxed by an
/// untrusted artifact").
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExecProfileV1 {
    /// Which evaluator produced the world this profile is recorded in —
    /// `"brix-world-net@1"` for the maintained incremental `WorldNetwork`, or
    /// `"brix-world-ref@1"` for the independent reference evaluator.
    pub evaluator: String,
    pub crate_version: String,
    pub module_loader_limits: ExecModuleLoaderLimits,
    pub numeric_semantics: String,
    pub settlement: String,
}

impl ExecProfileV1 {
    /// This crate's current profile for worlds driven by [`WorldNetwork`].
    pub fn current_for_network() -> Self {
        let limits = ModuleLoaderLimits::default();
        Self {
            evaluator: "brix-world-net@1".to_string(),
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            module_loader_limits: ExecModuleLoaderLimits {
                depth: limits.max_import_depth,
                modules: limits.max_import_modules,
                module_bytes: limits.max_module_source_bytes,
                total_bytes: limits.max_total_source_bytes,
            },
            numeric_semantics: "ADR-0045".to_string(),
            settlement: "least-key(phase,priority,tiebreak)".to_string(),
        }
    }

    /// Canonical profile identity, including the candidate-value admissibility rule.
    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(EXEC_PROFILE_SCHEMA);
        w.write_str(&self.evaluator);
        w.write_str(&self.crate_version);
        for n in [self.module_loader_limits.depth, self.module_loader_limits.modules,
            self.module_loader_limits.module_bytes, self.module_loader_limits.total_bytes] { w.write_uint(n as u64); }
        w.write_str(&self.numeric_semantics);
        w.write_str(&self.settlement);
        w.write_str("same-proposal-entity-supports-must-agree@1");
        w.digest(Domain::Value)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "profile": EXEC_PROFILE_SCHEMA,
            "evaluator": self.evaluator,
            "crate_version": self.crate_version,
            "module_loader_limits": {
                "depth": self.module_loader_limits.depth,
                "modules": self.module_loader_limits.modules,
                "module_bytes": self.module_loader_limits.module_bytes,
                "total_bytes": self.module_loader_limits.total_bytes,
            },
            "numeric_semantics": self.numeric_semantics,
            "settlement": self.settlement,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Result<Self, WorldError> {
        let profile = v
            .get("profile")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'profile'".into()))?;
        if profile != EXEC_PROFILE_SCHEMA {
            return Err(WorldError::InvalidSchema(format!(
                "expected exec profile {EXEC_PROFILE_SCHEMA}, got {profile}"
            )));
        }
        let evaluator = v
            .get("evaluator")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'evaluator'".into()))?
            .to_string();
        let crate_version = v
            .get("crate_version")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'crate_version'".into()))?
            .to_string();
        let limits_val = v.get("module_loader_limits").ok_or_else(|| {
            WorldError::Json("exec_profile missing 'module_loader_limits'".into())
        })?;
        let get_usize = |field: &str| -> Result<usize, WorldError> {
            limits_val
                .get(field)
                .and_then(|x| x.as_u64())
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    WorldError::Json(format!(
                        "exec_profile.module_loader_limits missing '{field}'"
                    ))
                })
        };
        let module_loader_limits = ExecModuleLoaderLimits {
            depth: get_usize("depth")?,
            modules: get_usize("modules")?,
            module_bytes: get_usize("module_bytes")?,
            total_bytes: get_usize("total_bytes")?,
        };
        let numeric_semantics = v
            .get("numeric_semantics")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'numeric_semantics'".into()))?
            .to_string();
        let settlement = v
            .get("settlement")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'settlement'".into()))?
            .to_string();
        Ok(Self {
            evaluator,
            crate_version,
            module_loader_limits,
            numeric_semantics,
            settlement,
        })
    }
}

/// The active persistent world runtime session.
pub struct WorldSession {
    pub root: PathBuf,
    pub paths: WorldPaths,
    pub manifest: WorldManifest,
    pub current_revision: u64,
    pub current_revision_digest: Option<Digest>,
    pub node_store: FileNodeStore,
    pub relations: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub secondary_indexes: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub committed_idempotency_keys: BTreeMap<String, CommittedBatchInfo>,
    pub crash_injector: Option<CrashPoint>,
    pub network: Option<WorldNetwork>,
    /// Recorded (non-authoritative) execution profile (ADR-0046 P6 §1.1,
    /// decided 2026-10-04); `None` for a storage-only world or a
    /// `program.json` written before this field existed.
    pub exec_profile: Option<ExecProfileV1>,
}

impl WorldSession {
    /// Create a new world directory from a manifest.
    pub fn create(root: impl AsRef<Path>, manifest: WorldManifest) -> Result<Self, WorldError> {
        manifest.validate()?;
        let root = root.as_ref().to_path_buf();
        if root.exists() && (root.join("world.json").exists() || root.join("HEAD").exists()) {
            return Err(WorldError::WorldAlreadyExists);
        }

        let paths = WorldPaths::new(&root);
        paths.ensure_dirs()?;

        // Write world.json manifest
        let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;
        fs::write(paths.world_json(), manifest_bytes)?;

        // Write initial HEAD pointing to genesis (revision 0)
        fs::write(paths.head(), "0\n")?;

        let node_store = FileNodeStore::new(&root)?;
        let mut relations = BTreeMap::new();
        let mut secondary_indexes = BTreeMap::new();

        let mut rev0_sec_roots = BTreeMap::new();
        for (name, decl) in &manifest.relations {
            relations.insert(name.clone(), TrieMap::new());
            for idx in &decl.secondary_indexes {
                let sec_name = format!("{}:{}", name, idx);
                let trie = TrieMap::new();
                rev0_sec_roots.insert(sec_name.clone(), trie.root_digest());
                secondary_indexes.insert(sec_name, trie);
            }
        }

        // Write revision 0 record
        let rev0 = WorldRevision::new(
            0,
            &manifest.created_at,
            0,
            "genesis",
            None,
            None,
            None,
            BTreeMap::new(),
            BTreeMap::new(),
            rev0_sec_roots,
            None,
            BTreeMap::new(),
            SettlementStatus::Committed,
            None,
        );
        let rev0_bytes = serde_json::to_vec_pretty(&rev0.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;
        fs::write(paths.revision_file(0), rev0_bytes)?;

        Ok(Self {
            root,
            paths,
            manifest,
            current_revision: 0,
            current_revision_digest: Some(rev0.revision_digest),
            node_store,
            relations,
            secondary_indexes,
            committed_idempotency_keys: BTreeMap::new(),
            crash_injector: None,
            network: None,
            exec_profile: None,
        })
    }

    /// Open an existing world directory (lazy cold open).
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorldError> {
        let root = root.as_ref().to_path_buf();
        let paths = WorldPaths::new(&root);

        if !paths.world_json().exists() {
            return Err(WorldError::ManifestNotFound);
        }

        let manifest_bytes = fs::read(paths.world_json())?;
        let manifest_val: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let manifest = WorldManifest::from_json(&manifest_val)?;

        if !paths.head().exists() {
            return Err(WorldError::CorruptedHead("HEAD file missing".to_string()));
        }

        let head_str = fs::read_to_string(paths.head())?;
        let head_line = head_str.lines().next().unwrap_or("").trim();
        let current_revision: u64 = head_line
            .split_whitespace()
            .next()
            .ok_or_else(|| WorldError::CorruptedHead("empty HEAD".to_string()))?
            .parse()
            .map_err(|_| WorldError::CorruptedHead("invalid integer in HEAD".to_string()))?;

        let node_store = FileNodeStore::new(&root)?;

        // Read current revision record
        let rev_file = paths.revision_file(current_revision);
        if !rev_file.exists() {
            return Err(WorldError::RevisionNotFound(current_revision));
        }
        let rev_bytes = fs::read(rev_file)?;
        let rev_val: serde_json::Value =
            serde_json::from_slice(&rev_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let current_rev = WorldRevision::from_json(&rev_val)?;
        let head_fields: Vec<_> = head_line.split_whitespace().collect();
        if current_rev.seq != current_revision || head_fields.len() > 2 ||
           (head_fields.len() == 1 && current_revision != 0) ||
           (head_fields.len() == 2 && head_fields[1] != current_rev.revision_digest.to_hex()) {
            return Err(WorldError::CorruptedHead("HEAD revision digest mismatch".into()));
        }

        if manifest.program_required && current_rev.program_digest != Some(manifest.program_digest)
        {
            return Err(WorldError::InvalidSchema(
                "executable world revision is not bound to its manifest program digest".into(),
            ));
        }

        // Lazy hydration: construct TrieMaps from Merkle root digests without loading full tries
        let mut relations = BTreeMap::new();
        for name in manifest.relations.keys() {
            let root_digest = current_rev.relation_roots.get(name).copied();
            let card = current_rev
                .relation_cardinalities
                .get(name)
                .copied()
                .unwrap_or(0);
            match root_digest {
                Some(digest) => {
                    relations.insert(name.clone(), TrieMap::from_root_digest(digest, card));
                }
                None => {
                    relations.insert(name.clone(), TrieMap::new());
                }
            }
        }

        let mut secondary_indexes = BTreeMap::new();
        for (idx_name, &digest) in &current_rev.secondary_index_roots {
            secondary_indexes.insert(idx_name.clone(), TrieMap::from_root_digest(digest, 0));
        }
        for (name, decl) in &manifest.relations {
            for idx in &decl.secondary_indexes {
                let sec_name = format!("{}:{}", name, idx);
                secondary_indexes
                    .entry(sec_name)
                    .or_insert_with(TrieMap::new);
            }
        }

        // Scan past revision headers for idempotency keys
        let mut committed_idempotency_keys = BTreeMap::new();
        if let Ok(entries) = fs::read_dir(paths.revisions_dir()) {
            for entry in entries.flatten() {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let (Some(seq), Some(ikey), Some(d_hex)) = (
                            v.get("seq").and_then(|x| x.as_u64()),
                            v.get("idempotency_key").and_then(|x| x.as_str()),
                            v.get("revision_digest").and_then(|x| x.as_str()),
                        ) {
                            if seq > 0 && seq <= current_revision && d_hex.len() == 64 {
                                if let Ok(d) = hex_to_digest(d_hex) {
                                    let b_digest = v
                                        .get("batch_digest")
                                        .and_then(|x| x.as_str())
                                        .and_then(|s| hex_to_digest(s).ok());
                                    committed_idempotency_keys.insert(
                                        ikey.to_string(),
                                        CommittedBatchInfo {
                                            seq,
                                            revision_digest: d,
                                            batch_digest: b_digest,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut session = Self {
            root,
            paths,
            manifest,
            current_revision,
            current_revision_digest: Some(current_rev.revision_digest),
            node_store,
            relations,
            secondary_indexes,
            committed_idempotency_keys,
            crash_injector: None,
            network: None,
            exec_profile: None,
        };

        let program_file = session.paths.root.join("program.json");
        if session.manifest.program_required && !program_file.exists() {
            return Err(WorldError::InvalidSchema(
                "executable world is missing program.json source closure".into(),
            ));
        }
        if program_file.exists() {
            let bytes = fs::read(&program_file)?;
            let val: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| WorldError::Json(e.to_string()))?;
            let root_mod = val
                .get("root_module")
                .and_then(|x| x.as_str())
                .ok_or_else(|| WorldError::Json("program closure missing root_module".into()))?;
            let src_map = val
                .get("sources")
                .and_then(|x| x.as_object())
                .ok_or_else(|| WorldError::Json("program closure missing sources object".into()))?;
            let mut sources = BTreeMap::new();
            for (name, source) in src_map {
                let text = source.as_str().ok_or_else(|| {
                    WorldError::Json(format!("program source '{name}' must be a string"))
                })?;
                sources.insert(name.clone(), text.to_owned());
            }
            if val.get("schema").and_then(|x| x.as_str()) != Some("brix.world.program@1") {
                return Err(WorldError::InvalidSchema(
                    "unsupported program closure schema".into(),
                ));
            }
            let (linked, closure_digest, closure_sources) = link_closure(root_mod, &sources)?;
            if closure_sources.len() != sources.len() {
                return Err(WorldError::InvalidSchema(
                    "program source map contains modules outside the transitive closure".into(),
                ));
            }
            let pinned = val
                .get("program_manifest_digest")
                .and_then(|x| x.as_str())
                .ok_or_else(|| {
                    WorldError::Json("program closure missing program_manifest_digest".into())
                })?;
            let pinned = parse_digest(pinned, "program_manifest_digest")?;
            if pinned != closure_digest || pinned != session.manifest.program_digest {
                return Err(WorldError::InvalidSchema(
                    "program closure digest does not match world manifest".into(),
                ));
            }
            let network = WorldNetwork::from_program(&linked)?;
            validate_program_relations(&session.manifest, &network)?;
            session.set_network(network)?;

            // Additive field (ADR-0046 P6 §1.1, decided 2026-10-04): a
            // `program.json` written before `exec_profile` existed simply
            // lacks the key, and decodes as `None` here, not a default-valued
            // profile — the file still opens either way.
            if let Some(profile_val) = val.get("exec_profile") {
                session.exec_profile = Some(ExecProfileV1::from_json(profile_val)?);
            }
        }

        if let Some(bound) = current_rev.exec_profile_digest {
            if session.exec_profile.as_ref().map(ExecProfileV1::digest) != Some(bound) {
                return Err(WorldError::InvalidSchema("execution profile digest mismatch".into()));
            }
        }
        if let Some(profile) = &session.exec_profile {
            if profile != &ExecProfileV1::current_for_network() {
                return Err(WorldError::InvalidSchema("unsupported execution profile".into()));
            }
        }
        if current_rev.decision_delta_digest.is_some() { session.decision_delta(current_revision)?; }
        if let (Some(expected), Some(net)) = (current_rev.decision_root, &session.network) {
            if net.decision_root() != expected {
                return Err(WorldError::NetworkError("restored decision root mismatch".into()));
            }
        }
        Ok(session)
    }

    pub fn set_crash_point(&mut self, cp: Option<CrashPoint>) {
        self.crash_injector = cp;
    }

    pub fn current_revision(&self) -> u64 {
        self.current_revision
    }

    pub fn current_revision_digest(&self) -> Option<Digest> {
        self.current_revision_digest
    }

    pub fn manifest(&self) -> &WorldManifest {
        &self.manifest
    }

    fn read_disk_head(&self) -> Result<u64, WorldError> {
        if !self.paths.head().exists() {
            return Err(WorldError::CorruptedHead("HEAD file missing".to_string()));
        }
        let head_str = fs::read_to_string(self.paths.head())?;
        let head_line = head_str.lines().next().unwrap_or("").trim();
        let current_revision: u64 = head_line
            .split_whitespace()
            .next()
            .ok_or_else(|| WorldError::CorruptedHead("empty HEAD".to_string()))?
            .parse()
            .map_err(|_| WorldError::CorruptedHead("invalid integer in HEAD".to_string()))?;
        Ok(current_revision)
    }

    /// Read a record by key from a relation.
    pub fn get(&self, relation: &str, key: &WorldKey) -> Result<Option<WorldTuple>, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;
        let val = trie.get_with_store(key, &self.node_store)?;
        Ok(val)
    }

    /// Collect every matching primary key. For bounded memory, use the paged API.
    /// `indexed_value` must be produced by [`encode_secondary_key`].
    pub fn query_secondary_index(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
    ) -> Result<Vec<WorldKey>, WorldError> {
        let mut keys = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.query_secondary_index_page(
                relation,
                index_field,
                indexed_value,
                cursor.as_ref(),
                1024,
            )?;
            keys.extend(page.keys);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(keys);
            }
        }
    }

    /// Read a bounded page using an explicitly encoded secondary key.
    /// A continuation from an earlier revision is rejected; pin a snapshot to page
    /// across concurrent commits without changing the result set.
    pub fn query_secondary_index_page(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
        cursor: Option<&SecondaryIndexCursor>,
        limit: usize,
    ) -> Result<SecondaryIndexPage, WorldError> {
        let revision = self
            .current_revision_digest
            .ok_or_else(|| WorldError::CorruptedHead("current revision digest missing".into()))?;
        secondary_index_page(
            &self.secondary_indexes,
            &self.node_store,
            revision,
            (relation, index_field, indexed_value),
            cursor,
            limit,
        )
    }

    /// Apply a transactional mutation batch.
    pub fn apply_batch(&mut self, batch: WorldBatch) -> Result<RevisionReceipt, WorldError> {
        let _lock = WorldLockGuard::acquire(&self.root)?;

        // Verify disk HEAD matches in-memory current_revision
        let disk_head = self.read_disk_head()?;
        if disk_head != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: batch.expected_base_revision,
                current: disk_head,
            });
        }

        // 1. Check idempotency key
        let batch_digest = batch.digest();
        if let Some(info) = self.committed_idempotency_keys.get(&batch.idempotency_key) {
            if info.batch_digest == Some(batch_digest) {
                return Ok(RevisionReceipt {
                    revision_seq: info.seq,
                    revision_digest: info.revision_digest,
                    idempotency_key: batch.idempotency_key,
                    is_idempotent_replay: true,
                    changed_keys_count: 0,
                    objects_written: 0,
                });
            } else {
                return Err(WorldError::IdempotencyConflict {
                    key: batch.idempotency_key,
                    reason: "idempotency key reused with different batch payload".to_string(),
                });
            }
        }

        // 2. Validate expected base revision
        if batch.expected_base_revision != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: batch.expected_base_revision,
                current: self.current_revision,
            });
        }

        // 3. Normalize operations and reject conflicting duplicate operations
        let normalized_ops = batch.validate_and_normalize()?;

        // Stage operator network updates and deliberation if network is present
        let (staged_network, decision_root, decision_delta_digest, decision_delta_body) =
            if let Some(ref net) = self.network {
                let mut sn = net.clone();
                let delta_report = sn.apply_ops_staged(&normalized_ops)?;
                if sn.decides.is_empty() {
                    (Some(sn), None, None, None)
                } else {
                    let d_root = Some(sn.decision_root());
                    let body = super::network::encode_decision_delta(
                        &delta_report.settlements,
                        &delta_report.removed_settlements,
                    );
                    let d_delta_digest = Some(Digest::of(Domain::Value, &body));
                    (Some(sn), d_root, d_delta_digest, Some(body))
                }
            } else {
                (None, None, None, None)
            };

        // 4. Stage updates across relations and maintain changed keys
        let mut staged_relations = self.relations.clone();
        let mut staged_secondary_indexes = self.secondary_indexes.clone();
        let mut changed_keys: BTreeMap<String, Vec<WorldKey>> = BTreeMap::new();

        let mut inner_objects_written = 0;
        for op in normalized_ops {
            let rel_name = op.relation().to_string();
            let decl = match self.manifest.relations.get(&rel_name) {
                Some(d) => d,
                None => return Err(WorldError::UnknownRelation(rel_name)),
            };

            let trie = staged_relations
                .get_mut(&rel_name)
                .expect("relation in manifest");

            match op {
                WorldBatchOp::Upsert { key, tuple, .. } => {
                    let current_val = trie.get_with_store(&key, &self.node_store)?;
                    if current_val.as_ref() == Some(&tuple) {
                        // Value is identical: no-op!
                        continue;
                    }

                    // Update secondary indexes if any
                    for idx in &decl.secondary_indexes {
                        let sec_name = format!("{}:{}", rel_name, idx);
                        if let Some(sec_trie) = staged_secondary_indexes.get_mut(&sec_name) {
                            let old_field_opt = match &current_val {
                                Some(old_t) => {
                                    Some(extract_indexed_field(&rel_name, decl, idx, old_t)?)
                                }
                                None => None,
                            };
                            let new_field = extract_indexed_field(&rel_name, decl, idx, &tuple)?;

                            // Skip maintenance when projected key is unchanged!
                            if old_field_opt.as_ref() == Some(&new_field) {
                                continue;
                            }

                            // 1. If old field exists, remove key from old customer's set
                            if let Some(old_val) = old_field_opt {
                                let old_k = encode_secondary_key(&old_val);
                                if let Some(old_set_tuple) =
                                    sec_trie.get_with_store(&old_k, &self.node_store)?
                                {
                                    if old_set_tuple.as_bytes().len() == 32 {
                                        let mut d = [0u8; 32];
                                        d.copy_from_slice(old_set_tuple.as_bytes());
                                        let old_digest = Digest::from_bytes(d);
                                        let old_set =
                                            TrieMap::<WorldKey, WorldTuple>::from_root_digest(
                                                old_digest, 0,
                                            );
                                        let (next_old_set, _) =
                                            old_set.remove_with_store(&key, &self.node_store)?;
                                        inner_objects_written +=
                                            next_old_set.persist_to_store(&mut self.node_store);

                                        let empty_d = Digest::of(
                                            brix_canon::Domain::Value,
                                            b"brix:trie:empty:v1",
                                        );
                                        if next_old_set.is_empty()
                                            || next_old_set.root_digest() == empty_d
                                        {
                                            let (next_sec, _) = sec_trie
                                                .remove_with_store(&old_k, &self.node_store)?;
                                            *sec_trie = next_sec;
                                        } else {
                                            let next_set_root = next_old_set.root_digest();
                                            let (next_sec, _) = sec_trie.insert_with_store(
                                                old_k,
                                                WorldTuple::new(next_set_root.as_bytes().to_vec()),
                                                &self.node_store,
                                            )?;
                                            *sec_trie = next_sec;
                                        }
                                    }
                                }
                            }

                            // 2. Insert key into new customer's set
                            let new_k = encode_secondary_key(&new_field);
                            let current_set =
                                match sec_trie.get_with_store(&new_k, &self.node_store)? {
                                    Some(set_tuple) if set_tuple.as_bytes().len() == 32 => {
                                        let mut d = [0u8; 32];
                                        d.copy_from_slice(set_tuple.as_bytes());
                                        let digest = Digest::from_bytes(d);
                                        TrieMap::<WorldKey, WorldTuple>::from_root_digest(digest, 0)
                                    }
                                    _ => TrieMap::<WorldKey, WorldTuple>::new(),
                                };
                            let (next_set, _) = current_set.insert_with_store(
                                key.clone(),
                                WorldTuple::new(vec![]),
                                &self.node_store,
                            )?;
                            inner_objects_written +=
                                next_set.persist_to_store(&mut self.node_store);
                            let next_set_root = next_set.root_digest();
                            let (next_sec, _) = sec_trie.insert_with_store(
                                new_k,
                                WorldTuple::new(next_set_root.as_bytes().to_vec()),
                                &self.node_store,
                            )?;
                            *sec_trie = next_sec;
                        }
                    }

                    let (next_trie, _) =
                        trie.insert_with_store(key.clone(), tuple, &self.node_store)?;
                    *trie = next_trie;
                    changed_keys.entry(rel_name).or_default().push(key);
                }
                WorldBatchOp::Remove { key, .. } => {
                    let current_val = trie.get_with_store(&key, &self.node_store)?;
                    if current_val.is_none() {
                        // Key does not exist: no-op!
                        continue;
                    }

                    if let Some(ref old_t) = current_val {
                        for idx in &decl.secondary_indexes {
                            let sec_name = format!("{}:{}", rel_name, idx);
                            if let Some(sec_trie) = staged_secondary_indexes.get_mut(&sec_name) {
                                let old_val = extract_indexed_field(&rel_name, decl, idx, old_t)?;
                                let old_k = encode_secondary_key(&old_val);
                                if let Some(old_set_tuple) =
                                    sec_trie.get_with_store(&old_k, &self.node_store)?
                                {
                                    if old_set_tuple.as_bytes().len() == 32 {
                                        let mut d = [0u8; 32];
                                        d.copy_from_slice(old_set_tuple.as_bytes());
                                        let old_digest = Digest::from_bytes(d);
                                        let old_set =
                                            TrieMap::<WorldKey, WorldTuple>::from_root_digest(
                                                old_digest, 0,
                                            );
                                        let (next_old_set, _) =
                                            old_set.remove_with_store(&key, &self.node_store)?;
                                        inner_objects_written +=
                                            next_old_set.persist_to_store(&mut self.node_store);

                                        let empty_d = Digest::of(
                                            brix_canon::Domain::Value,
                                            b"brix:trie:empty:v1",
                                        );
                                        if next_old_set.is_empty()
                                            || next_old_set.root_digest() == empty_d
                                        {
                                            let (next_sec, _) = sec_trie
                                                .remove_with_store(&old_k, &self.node_store)?;
                                            *sec_trie = next_sec;
                                        } else {
                                            let next_set_root = next_old_set.root_digest();
                                            let (next_sec, _) = sec_trie.insert_with_store(
                                                old_k,
                                                WorldTuple::new(next_set_root.as_bytes().to_vec()),
                                                &self.node_store,
                                            )?;
                                            *sec_trie = next_sec;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let (next_trie, _) = trie.remove_with_store(&key, &self.node_store)?;
                    *trie = next_trie;
                    changed_keys.entry(rel_name).or_default().push(key);
                }
            }
        }

        // 5. Persist modified trie nodes to node store
        let mut objects_written = inner_objects_written;
        for trie in staged_relations.values() {
            objects_written += trie.persist_to_store(&mut self.node_store);
        }
        let mut secondary_index_roots = BTreeMap::new();
        for (sec_name, trie) in &staged_secondary_indexes {
            objects_written += trie.persist_to_store(&mut self.node_store);
            secondary_index_roots.insert(sec_name.clone(), trie.root_digest());
        }
        // Persist the settled-decision trie's own nodes (ADR-0046 P6 G2,
        // decided 2026-10-04): previously only its root digest reached the
        // revision record, so historical settlements could not be read back
        // without replaying every base record. Same persistence seam as the
        // relation/secondary-index tries above — same crash-recovery story.
        if let Some(ref sn) = staged_network {
            objects_written += sn.decision_tree.persist_to_store(&mut self.node_store);
        }

        // Crash injection seam 1
        if self.crash_injector == Some(CrashPoint::BeforeObjectsFsync) {
            return Err(WorldError::InjectedCrash(CrashPoint::BeforeObjectsFsync));
        }

        // Fsync newly written objects
        self.node_store.flush()?;

        // Crash injection seam 2
        if self.crash_injector == Some(CrashPoint::AfterObjectsFsyncBeforeRevisionFsync) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterObjectsFsyncBeforeRevisionFsync,
            ));
        }

        let new_seq = self.current_revision + 1;

        // Write the decision-delta body durably BEFORE the revision record
        // (ADR-0046 P6 G2/G3, decided 2026-10-04): tmp-write, fsync, rename —
        // the same atomic-write discipline as the revision record itself, one
        // step earlier in the publication order. An uncommitted attempt may
        // leave an orphaned `{seq}.decisions` file; harmless, since nothing
        // ever reads one without the matching revision record's digest.
        if let Some(ref body) = decision_delta_body {
            let tmp_delta_file = self.paths.decision_delta_tmp_file(new_seq);
            fs::write(&tmp_delta_file, body)?;
            {
                let f = fs::File::open(&tmp_delta_file)?;
                f.sync_all()?;
            }
            fs::rename(&tmp_delta_file, self.paths.decision_delta_file(new_seq))?;
            fs::File::open(self.paths.revisions_dir())?.sync_all()?;
        }

        if self.crash_injector == Some(CrashPoint::AfterDecisionDeltaFsyncBeforeRevisionFsync) {
            return Err(WorldError::InjectedCrash(CrashPoint::AfterDecisionDeltaFsyncBeforeRevisionFsync));
        }
        // 6. Assemble revision record
        let mut relation_roots = BTreeMap::new();
        let mut relation_cardinalities = BTreeMap::new();
        for (rel, trie) in &staged_relations {
            relation_roots.insert(rel.clone(), trie.root_digest());
            relation_cardinalities.insert(rel.clone(), trie.len());
        }

        let new_revision = WorldRevision::new(
            new_seq,
            iso_now(),
            self.current_revision,
            &batch.idempotency_key,
            Some(batch_digest),
            self.manifest
                .program_required
                .then_some(self.manifest.program_digest),
            self.current_revision_digest,
            relation_roots,
            relation_cardinalities,
            secondary_index_roots,
            decision_root,
            changed_keys.clone(),
            SettlementStatus::Committed,
            decision_delta_digest,
        ).bind_exec_profile(self.exec_profile.as_ref().map(ExecProfileV1::digest));

        let rev_json = serde_json::to_vec_pretty(&new_revision.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;

        let tmp_rev_file = self.paths.revision_tmp_file(new_seq);
        fs::write(&tmp_rev_file, rev_json)?;
        {
            let f = fs::File::open(&tmp_rev_file)?;
            f.sync_all()?;
        }
        fs::rename(&tmp_rev_file, self.paths.revision_file(new_seq))?;
        fs::File::open(self.paths.revisions_dir())?.sync_all()?;

        // Crash injection seam 3
        if self.crash_injector == Some(CrashPoint::AfterRevisionFsyncBeforeHeadRename) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterRevisionFsyncBeforeHeadRename,
            ));
        }

        // 7. Atomically publish HEAD pointer
        let head_content = format!("{} {}\n", new_seq, new_revision.revision_digest.to_hex());
        fs::write(self.paths.head_tmp(), &head_content)?;
        {
            let f = fs::File::open(self.paths.head_tmp())?;
            f.sync_all()?;
        }
        fs::rename(self.paths.head_tmp(), self.paths.head())?;

        // Crash injection seam 4
        if self.crash_injector == Some(CrashPoint::AfterHeadRenameBeforeDirectoryFsync) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterHeadRenameBeforeDirectoryFsync,
            ));
        }

        // Fsync parent directory
        fs::File::open(&self.root)?.sync_all()?;

        // 8. Update in-memory session state
        let total_changed: usize = changed_keys.values().map(|v| v.len()).sum();
        self.current_revision = new_seq;
        self.current_revision_digest = Some(new_revision.revision_digest);
        self.relations = staged_relations;
        self.secondary_indexes = staged_secondary_indexes;
        if let Some(sn) = staged_network {
            self.network = Some(sn);
        }
        self.committed_idempotency_keys.insert(
            batch.idempotency_key.clone(),
            CommittedBatchInfo {
                seq: new_seq,
                revision_digest: new_revision.revision_digest,
                batch_digest: Some(batch_digest),
            },
        );

        Ok(RevisionReceipt {
            revision_seq: new_seq,
            revision_digest: new_revision.revision_digest,
            idempotency_key: batch.idempotency_key,
            is_idempotent_replay: false,
            changed_keys_count: total_changed,
            objects_written,
        })
    }

    /// Pin an immutable read snapshot at revision `seq`.
    pub fn pin_revision(&self, seq: u64) -> Result<WorldSnapshot, WorldError> {
        if seq > self.current_revision { return Err(WorldError::RevisionNotFound(seq)); }
        let rev_file = self.paths.revision_file(seq);
        if !rev_file.exists() {
            return Err(WorldError::RevisionNotFound(seq));
        }
        let rev_bytes = fs::read(rev_file)?;
        let rev_val: serde_json::Value =
            serde_json::from_slice(&rev_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let rev = WorldRevision::from_json(&rev_val)?;
        if self.manifest.program_required
            && rev.program_digest != Some(self.manifest.program_digest)
        {
            return Err(WorldError::InvalidSchema(format!(
                "revision {seq} is not bound to the world's executable program digest"
            )));
        }

        let mut relations = BTreeMap::new();
        for name in self.manifest.relations.keys() {
            let root_digest = rev.relation_roots.get(name).copied();
            let card = rev.relation_cardinalities.get(name).copied().unwrap_or(0);
            match root_digest {
                Some(digest) => {
                    relations.insert(name.clone(), TrieMap::from_root_digest(digest, card));
                }
                None => {
                    relations.insert(name.clone(), TrieMap::new());
                }
            }
        }

        let mut secondary_indexes = BTreeMap::new();
        for (sec_name, &digest) in &rev.secondary_index_roots {
            secondary_indexes.insert(sec_name.clone(), TrieMap::from_root_digest(digest, 0));
        }

        Ok(WorldSnapshot {
            revision: rev,
            relations,
            secondary_indexes,
            node_store: self.node_store.clone(),
        })
    }

    /// Paginate records from a relation.
    pub fn query_page(
        &self,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let parsed_cursor = match cursor {
            Some(token) => Some(WorldCursor::from_token(token)?),
            None => None,
        };

        let (entries, next_pos) = trie.iter_page_with_store(
            parsed_cursor.as_ref().map(|c| (&c.hash, &c.key)),
            limit,
            &self.node_store,
        )?;

        let next_cursor = next_pos.map(|(hash, key)| WorldCursor::new(hash, key).to_token());
        let has_more = next_cursor.is_some();

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Retrieve derived tuples for a relation from the active operator network.
    pub fn get_derived(&self, relation: &str) -> Option<Vec<TupleRecord>> {
        let net = self.network.as_ref()?;
        net.get_derived_tuples(relation).or_else(|| {
            net.derived_relations
                .keys()
                .find(|k| k.ends_with(&format!("::{relation}")))
                .and_then(|k| net.get_derived_tuples(k))
        })
    }

    /// Query a paged slice of derived tuples from the active operator network.
    pub fn query_derived_page(
        &self,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        if limit == 0 {
            return Err(WorldError::InvalidSchema(
                "query page limit must be positive".to_string(),
            ));
        }
        let records = self
            .get_derived(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let offset = match cursor {
            Some(token) => token.parse::<usize>().map_err(|_| {
                WorldError::InvalidCursor(
                    "invalid derived page cursor (expected integer offset)".to_string(),
                )
            })?,
            None => 0,
        };

        let total = records.len();
        let entries: Vec<(WorldKey, WorldTuple)> = records
            .iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(i, r)| {
                let key = WorldKey::from_str(&format!("{}", offset + i));
                (key, r.to_tuple())
            })
            .collect();

        let next_offset = offset + entries.len();
        let has_more = next_offset < total;
        let next_cursor = if has_more {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Retrieve all settled decisions across all decide blocks.
    pub fn all_settlements(&self) -> BTreeMap<String, BTreeMap<String, SettledDecision>> {
        self.network
            .as_ref()
            .map(|n| n.all_settlements())
            .unwrap_or_default()
    }

    /// Explain the decision deliberation for an entity key.
    pub fn explain_decision(&self, entity_key: &str) -> Option<DecisionExplanation> {
        self.network.as_ref()?.explain_decision(entity_key)
    }

    /// Explain the decision deliberation for an entity key within a specific decide block.
    pub fn explain_decision_for(
        &self,
        decide_name: &str,
        entity_key: &str,
    ) -> Option<DecisionExplanation> {
        self.network
            .as_ref()?
            .explain_decision_for(decide_name, entity_key)
    }

    /// Read and authenticate a committed revision's canonical decision changes.
    /// Legacy records have no trustworthy delta body and explicitly refuse.
    pub fn decision_delta(&self, seq: u64) -> Result<super::decision_codec::DecisionDelta, WorldError> {
        let snapshot = self.pin_revision(seq)?;
        let rev = snapshot.revision;
        if rev.schema != super::revision::REVISION_SCHEMA_V2 {
            return Err(WorldError::NetworkError("legacy-history-unavailable".into()));
        }
        let Some(expected) = rev.decision_delta_digest else {
            if rev.decision_root.is_some() { return Err(WorldError::NetworkError("missing decision delta digest".into())); }
            return Ok(BTreeMap::new());
        };
        let path = self.paths.decision_delta_file(seq);
        const MAX_BYTES: u64 = 64 * 1024 * 1024;
        if fs::metadata(&path)?.len() > MAX_BYTES { return Err(WorldError::NetworkError("decision delta byte limit".into())); }
        let bytes = fs::read(path)?;
        if Digest::of(Domain::Value, &bytes) != expected {
            return Err(WorldError::NetworkError("decision delta digest mismatch".into()));
        }
        super::decision_codec::decode_decision_delta(&bytes, 1_000_000)
    }

    /// Read a settled decision directly from the persisted decision trie at a
    /// past revision (ADR-0046 P6 G2, decided 2026-10-04) — `brix world
    /// explain --rev n` reads real historical evidence from the node store,
    /// not a replay. Decide-block declarations never change after genesis
    /// (`save_program_closure` refuses re-binding once revisions exist), so
    /// the *current* session's known decide names are valid candidates to
    /// search at any past revision of the same program. Returns `Ok(None)`
    /// when revision `seq` records no settlement for `entity_id` in any
    /// (optionally filtered) decide block — distinct from a decode failure,
    /// which is a typed error.
    pub fn historical_settlement(
        &self,
        seq: u64,
        decide_filter: Option<&str>,
        entity_id: &str,
    ) -> Result<Option<(String, SettledDecision)>, WorldError> {
        if seq > self.current_revision { return Err(WorldError::RevisionNotFound(seq)); }
        let rev_file = self.paths.revision_file(seq);
        if !rev_file.exists() {
            return Err(WorldError::RevisionNotFound(seq));
        }
        let rev_bytes = fs::read(rev_file)?;
        let rev_val: serde_json::Value =
            serde_json::from_slice(&rev_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let rev = WorldRevision::from_json(&rev_val)?;
        if rev.schema != super::revision::REVISION_SCHEMA_V2 {
            return Err(WorldError::NetworkError("legacy-history-unavailable: revision@1 did not persist decision evidence".into()));
        }
        let Some(decision_root) = rev.decision_root else {
            return Ok(None);
        };

        let known_decides: Vec<String> = self
            .network
            .as_ref()
            .map(|n| n.decides.keys().cloned().collect())
            .unwrap_or_default();
        let candidates: Vec<String> = match decide_filter {
            Some(d) => known_decides
                .into_iter()
                .filter(|k| k == d || k.ends_with(&format!("::{d}")))
                .collect(),
            None => known_decides,
        };

        let tree = TrieMap::<WorldKey, WorldTuple>::from_root_digest(decision_root, 0);
        for decide_name in candidates {
            let key = super::network::decision_key(&decide_name, entity_id);
            if let Some(tuple) = tree.get_with_store(&key, &self.node_store)? {
                let settled = super::network::decode_settled_decision(entity_id, tuple.as_bytes())?;
                return Ok(Some((decide_name, settled)));
            }
        }
        Ok(None)
    }

    /// Attach an operator network to this session and hydrate it with existing committed base records.
    pub fn set_network(&mut self, mut network: WorldNetwork) -> Result<(), WorldError> {
        let mut ops = Vec::new();
        for (rel_name, trie) in &self.relations {
            if network.base_relations.contains_key(rel_name) {
                let (entries, _) = trie.iter_page_with_store(None, usize::MAX, &self.node_store)?;
                for (key, tuple) in entries {
                    ops.push(WorldBatchOp::Upsert {
                        relation: rel_name.clone(),
                        key,
                        tuple,
                    });
                }
            }
        }
        if !ops.is_empty() {
            network.apply_ops(&ops)?;
        }
        network.current_revision = self.current_revision;
        self.network = Some(network);
        Ok(())
    }

    /// Save program closure (source map of modules) to `program.json` in the world directory.
    pub fn save_program_closure(
        &mut self,
        root_module: &str,
        sources: &BTreeMap<String, String>,
    ) -> Result<(), WorldError> {
        let (linked, closure_digest, closure_sources) = link_closure(root_module, sources)?;
        let network = WorldNetwork::from_program(&linked)?;
        validate_program_relations(&self.manifest, &network)?;
        if self.current_revision > 0 && self.manifest.program_digest != closure_digest {
            return Err(WorldError::InvalidSchema(
                "cannot change executable program after world revisions exist".into(),
            ));
        }
        let exec_profile = ExecProfileV1::current_for_network();
        let val = serde_json::json!({
            "schema": "brix.world.program@1",
            "root_module": root_module,
            "sources": closure_sources,
            "program_manifest_digest": closure_digest.to_hex(),
            "exec_profile": exec_profile.to_json(),
        });
        let bytes = serde_json::to_vec_pretty(&val).map_err(|e| WorldError::Json(e.to_string()))?;
        atomic_write(&self.paths.root.join("program.json"), &bytes)?;
        self.exec_profile = Some(exec_profile);
        if self.current_revision == 0 || self.manifest.program_digest != closure_digest || !self.manifest.program_required {
            let mut bound_manifest = self.manifest.clone();
            bound_manifest.program_digest = closure_digest;
            bound_manifest.program_required = true;
            let genesis_path = self.paths.revision_file(0);
            let genesis_json: serde_json::Value = serde_json::from_slice(&fs::read(&genesis_path)?)
                .map_err(|e| WorldError::Json(e.to_string()))?;
            let genesis = WorldRevision::from_json(&genesis_json)?;
            if genesis.seq != 0 || self.current_revision != 0 {
                return Err(WorldError::InvalidSchema(
                    "program closure can only bind at genesis".into(),
                ));
            }
            let rebound = WorldRevision::new(
                genesis.seq,
                genesis.timestamp,
                genesis.expected_base_revision,
                genesis.idempotency_key,
                genesis.batch_digest,
                Some(closure_digest),
                genesis.previous_revision_digest,
                genesis.relation_roots,
                genesis.relation_cardinalities,
                genesis.secondary_index_roots,
                genesis.decision_root,
                genesis.changed_keys,
                genesis.status,
                genesis.decision_delta_digest,
            ).bind_exec_profile(self.exec_profile.as_ref().map(ExecProfileV1::digest));
            let manifest_bytes = serde_json::to_vec_pretty(&bound_manifest.to_json())
                .map_err(|e| WorldError::Json(e.to_string()))?;
            let revision_bytes = serde_json::to_vec_pretty(&rebound.to_json())
                .map_err(|e| WorldError::Json(e.to_string()))?;
            // Write the revision before publishing the manifest requirement. A crash
            // between these writes leaves a reopenable storage-only world.
            atomic_write(&genesis_path, &revision_bytes)?;
            atomic_write(&self.paths.world_json(), &manifest_bytes)?;
            self.manifest = bound_manifest;
            self.current_revision_digest = Some(rebound.revision_digest);
        }
        self.set_network(network)?;
        Ok(())
    }

    /// Initialize or open a world session bound to a linked program.
    pub fn from_program(
        root: impl AsRef<Path>,
        program: &LinkedProgram,
    ) -> Result<Self, WorldError> {
        let root = root.as_ref().to_path_buf();
        if root.exists() && (root.join("world.json").exists() || root.join("HEAD").exists()) {
            let mut session = Self::open(&root)?;
            let network = WorldNetwork::from_program(program)?;
            validate_program_relations(&session.manifest, &network)?;
            session.set_network(network)?;
            return Ok(session);
        }

        let dag = lower_relations(program)?;
        let mut relations = Vec::new();
        for node in &dag.nodes {
            if let OperatorNode::Scan {
                relation,
                schema,
                key_fields,
            } = node
            {
                let all_fields: Vec<String> = match schema {
                    brix_syntax::ast::Ty::Record(fields) => {
                        fields.iter().map(|f| f.name.clone()).collect()
                    }
                    _ => vec![],
                };
                let value_fields: Vec<String> = all_fields
                    .iter()
                    .filter(|f| !key_fields.contains(f))
                    .cloned()
                    .collect();
                relations.push(RelationDecl::new(
                    relation.clone(),
                    key_fields.clone(),
                    value_fields,
                    vec![],
                ));
            }
        }

        let manifest = WorldManifest::new(
            "world-1",
            iso_now(),
            Digest::of(Domain::Value, program.root_module.as_bytes()),
            relations,
        );
        let mut manifest = manifest;
        manifest.program_required = true;
        let mut session = Self::create(&root, manifest)?;
        let network = WorldNetwork::from_program(program)?;
        session.set_network(network)?;
        Ok(session)
    }

    /// Initialize a world directly from its source closure, binding the full graph digest.
    pub fn from_program_with_sources(
        root: impl AsRef<Path>,
        root_module: &str,
        sources: &BTreeMap<String, String>,
    ) -> Result<Self, WorldError> {
        let (linked, _, _) = link_closure(root_module, sources)?;
        let mut session = Self::from_program(root, &linked)?;
        session.save_program_closure(root_module, sources)?;
        Ok(session)
    }

    /// Verify differential correctness between incremental network state and oracle full recompute.
    pub fn verify_oracle(&self) -> Result<(), WorldError> {
        if let Some(ref net) = self.network {
            net.verify_differential_correctness()?;
        }
        Ok(())
    }

    /// Paginate diff events between two revisions without scanning or serializing the entire world.
    pub fn diff_page(
        &self,
        relation: &str,
        from_rev: u64,
        to_rev: u64,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<DiffPage, WorldError> {
        if from_rev == to_rev {
            return Ok(DiffPage {
                relation: relation.to_string(),
                from_revision: from_rev,
                to_revision: to_rev,
                events: Vec::new(),
                next_cursor: None,
                has_more: false,
            });
        }

        let snap_from = self.pin_revision(from_rev)?;
        let snap_to = self.pin_revision(to_rev)?;

        let from_root = snap_from
            .revision
            .relation_roots
            .get(relation)
            .copied()
            .unwrap_or_else(|| Digest::of(brix_canon::Domain::Value, b"brix:trie:empty:v1"));
        let to_root = snap_to
            .revision
            .relation_roots
            .get(relation)
            .copied()
            .unwrap_or_else(|| Digest::of(brix_canon::Domain::Value, b"brix:trie:empty:v1"));

        // O(1) fast path: equal Merkle roots imply identical relation state!
        if from_root == to_root {
            return Ok(DiffPage {
                relation: relation.to_string(),
                from_revision: from_rev,
                to_revision: to_rev,
                events: Vec::new(),
                next_cursor: None,
                has_more: false,
            });
        }

        // Collect changed keys from revision journal
        let (min_rev, max_rev) = if from_rev < to_rev {
            (from_rev + 1, to_rev)
        } else {
            (to_rev + 1, from_rev)
        };

        let mut affected_keys = BTreeMap::new();
        for r in min_rev..=max_rev {
            if let Ok(snap) = self.pin_revision(r) {
                if let Some(keys) = snap.revision.changed_keys.get(relation) {
                    for k in keys {
                        affected_keys.insert(k.clone(), ());
                    }
                }
            }
        }

        let page_limit = if limit == 0 { 100 } else { limit };
        let start_key = match cursor {
            Some(token) => {
                if token.contains(':') {
                    Some(WorldCursor::from_token(token)?.key)
                } else {
                    Some(WorldKey::from_hex(token)?)
                }
            }
            None => None,
        };

        let mut events = Vec::new();
        let mut next_cursor = None;
        let mut has_more = false;

        let iter: Box<dyn Iterator<Item = (&WorldKey, &())>> = match &start_key {
            Some(sk) => Box::new(
                affected_keys.range((std::ops::Bound::Excluded(sk), std::ops::Bound::Unbounded)),
            ),
            None => Box::new(affected_keys.iter()),
        };

        for (k, _) in iter {
            let val_from = snap_from.get(relation, k)?;
            let val_to = snap_to.get(relation, k)?;

            let event = match (val_from, val_to) {
                (None, Some(t)) => Some(DiffEvent::Upserted {
                    key: k.clone(),
                    tuple: t,
                }),
                (Some(_), Some(t)) => Some(DiffEvent::Upserted {
                    key: k.clone(),
                    tuple: t,
                }),
                (Some(_), None) => Some(DiffEvent::Removed { key: k.clone() }),
                (None, None) => None,
            };

            if let Some(ev) = event {
                if events.len() < page_limit {
                    events.push(ev);
                } else {
                    has_more = true;
                    break;
                }
            }
        }

        if has_more {
            if let Some(last_ev) = events.last() {
                next_cursor = Some(last_ev.key().to_hex());
            }
        }

        Ok(DiffPage {
            relation: relation.to_string(),
            from_revision: from_rev,
            to_revision: to_rev,
            events,
            next_cursor,
            has_more,
        })
    }

    /// Stage a chunk for bulk ingestion.
    pub fn stage_chunk(
        &mut self,
        upload_id: &str,
        chunk_seq: u64,
        batch: &WorldBatch,
    ) -> Result<(), WorldError> {
        StagingManager::stage_chunk(
            self.paths.staging_dir().as_path(),
            upload_id,
            chunk_seq,
            batch,
        )
    }

    /// Consolidate staged chunks and atomically publish as a world revision.
    pub fn commit_staged(
        &mut self,
        upload_id: &str,
        expected_chunks: u64,
        idempotency_key: &str,
    ) -> Result<RevisionReceipt, WorldError> {
        let consolidated_batch = StagingManager::consolidate(
            self.paths.staging_dir().as_path(),
            upload_id,
            expected_chunks,
            self.current_revision,
            idempotency_key,
        )?;
        let receipt = self.apply_batch(consolidated_batch)?;
        StagingManager::cleanup_upload(self.paths.staging_dir().as_path(), upload_id)?;
        Ok(receipt)
    }

    /// Close the session.
    pub fn close(self) -> Result<(), WorldError> {
        self.node_store.flush()?;
        Ok(())
    }

    /// Reopen the session from disk.
    pub fn reopen(root: impl AsRef<Path>) -> Result<Self, WorldError> {
        Self::open(root)
    }
}

fn iso_now() -> String {
    "2026-10-04T00:00:00Z".to_string()
}

fn hex_to_digest(hex: &str) -> Result<Digest, WorldError> {
    if hex.len() != 64 {
        return Err(WorldError::Json(
            "digest hex must be 64 characters".to_string(),
        ));
    }
    let mut b = [0u8; 32];
    let chars: Vec<char> = hex.chars().collect();
    for i in (0..64).step_by(2) {
        let high = chars[i]
            .to_digit(16)
            .ok_or_else(|| WorldError::Json("invalid hex".to_string()))?;
        let low = chars[i + 1]
            .to_digit(16)
            .ok_or_else(|| WorldError::Json("invalid hex".to_string()))?;
        b[i / 2] = ((high << 4) | low) as u8;
    }
    Ok(Digest::from_bytes(b))
}
