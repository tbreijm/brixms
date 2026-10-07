//! Chunked bulk ingestion with staging area and atomic consolidation (ADR-0046 §3.6, P3).

use std::fs;
use std::path::Path;

use super::batch::{WorldBatch, WorldBatchOp};
use super::error::WorldError;

/// Manage staging of bulk chunks before final atomic publication.
pub struct StagingManager;

impl StagingManager {
    /// Validate upload_id to prevent path traversal or malformed identifier attacks.
    pub fn validate_upload_id(upload_id: &str) -> Result<(), WorldError> {
        if upload_id.is_empty() || upload_id.len() > 128 {
            return Err(WorldError::InvalidUploadId(
                "upload_id must be between 1 and 128 characters".to_string(),
            ));
        }
        if upload_id == "."
            || upload_id == ".."
            || upload_id.contains('/')
            || upload_id.contains('\\')
            || upload_id.contains("..")
        {
            return Err(WorldError::InvalidUploadId(
                "upload_id contains invalid path traversal characters".to_string(),
            ));
        }
        if !upload_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(WorldError::InvalidUploadId(
                "upload_id must contain only alphanumeric, '-', or '_' characters".to_string(),
            ));
        }
        Ok(())
    }

    /// Stage a single chunk under `staging/<upload_id>/chunk-<chunk_seq>.json`.
    pub fn stage_chunk(
        staging_dir: &Path,
        upload_id: &str,
        chunk_seq: u64,
        batch: &WorldBatch,
    ) -> Result<(), WorldError> {
        Self::validate_upload_id(upload_id)?;
        let upload_dir = staging_dir.join(upload_id);
        fs::create_dir_all(&upload_dir)?;
        let chunk_file = upload_dir.join(format!("chunk-{chunk_seq:06}.json"));
        let json_bytes = serde_json::to_vec_pretty(&batch.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;
        fs::write(chunk_file, json_bytes)?;
        Ok(())
    }

    /// Read all staged chunks `0..expected_chunks` and consolidate their operations into one batch.
    /// Does NOT remove the staging directory, ensuring staged data survives commit failures.
    pub fn consolidate(
        staging_dir: &Path,
        upload_id: &str,
        expected_chunks: u64,
        expected_base_revision: u64,
        idempotency_key: &str,
    ) -> Result<WorldBatch, WorldError> {
        Self::validate_upload_id(upload_id)?;
        if expected_chunks == 0 {
            return Err(WorldError::StagingError(
                "expected_chunks must be greater than 0".to_string(),
            ));
        }
        let upload_dir = staging_dir.join(upload_id);
        if !upload_dir.exists() {
            return Err(WorldError::StagingError(format!(
                "upload directory for '{upload_id}' not found"
            )));
        }

        let mut combined_ops: Vec<WorldBatchOp> = Vec::new();
        for seq in 0..expected_chunks {
            let chunk_file = upload_dir.join(format!("chunk-{seq:06}.json"));
            if !chunk_file.exists() {
                return Err(WorldError::StagingError(format!(
                    "missing chunk {seq} for upload '{upload_id}'"
                )));
            }
            let bytes = fs::read(&chunk_file)?;
            let json_val: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| WorldError::Json(e.to_string()))?;
            let chunk_batch = WorldBatch::from_json(&json_val)?;
            combined_ops.extend(chunk_batch.operations);
        }

        Ok(WorldBatch::new(
            expected_base_revision,
            idempotency_key,
            combined_ops,
        ))
    }

    /// Remove the staging directory for an upload after publication has succeeded.
    pub fn cleanup_upload(staging_dir: &Path, upload_id: &str) -> Result<(), WorldError> {
        Self::validate_upload_id(upload_id)?;
        let upload_dir = staging_dir.join(upload_id);
        if upload_dir.exists() {
            let _ = fs::remove_dir_all(&upload_dir);
        }
        Ok(())
    }
}
