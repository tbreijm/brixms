//! Chunk-size policy for chunked persist/dehydrate (CPD, R0 M7 seam).
//!
//! A large batch is applied as contiguous chunks of its normalized operations; between
//! chunks the staged network is persisted and dehydrated so the hydrated working set stays
//! near a byte budget. The policy only decides *where the chunk boundaries fall* (and when
//! to publish the intermediate pack). Every root, digest and decision delta is independent
//! of that choice (proved by `r0_chunked_persist`), so a policy can be heuristic, adaptive
//! or learned without being an authority.

#![deny(unsafe_code)]

use std::fmt::Debug;

/// Smallest chunk the heuristic plans: below this, per-chunk rewrites of the shared upper
/// trie nodes (write amplification) outweigh the memory saved.
pub const MIN_CHUNK_OPS: usize = 16_384;
/// Estimated hydrated heap per node written by the staged network (path-copied node plus
/// the support caches and cache bookkeeping it drags along). Calibrated against the lane A
/// heap test (`lane_a_node_heap.rs`: about 612 B of node heap, about 2.1 KB staged).
pub const HYDRATED_NODE_BYTES: u64 = 2048;
/// Nodes written per operation assumed before the first chunk has been measured.
pub const FIRST_CHUNK_NODES_PER_OP: u64 = 24;
/// Default `apply_hydrated_bytes` budget.
pub const DEFAULT_APPLY_HYDRATED_BYTES: u64 = 1024 * 1024 * 1024;
/// The pending pack's in-memory index costs about this many bytes per node.
pub const PENDING_INDEX_BYTES_PER_NODE: u64 = 130;
/// Publish an intermediate pack once the pending index exceeds this.
pub const PENDING_FLUSH_INDEX_BYTES: u64 = 256 * 1024 * 1024;

/// What the engine tells the policy before each chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkInput {
    pub ops_total: usize,
    pub ops_done: usize,
    /// Operations in the previous chunk (0 before the first).
    pub prev_chunk_ops: usize,
    /// Objects the previous chunk's persist wrote (0 before the first).
    pub prev_nodes_written: usize,
    /// Estimated in-memory index bytes of the pending (unpublished) pack.
    pub pending_index_bytes: u64,
    /// Hydrated-bytes budget; 0 means chunking is off.
    pub budget_bytes: u64,
}

/// The policy's answer. The engine clamps `ops` to `[1, remaining]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkDecision {
    /// Operations in the next chunk.
    pub ops: usize,
    /// Publish the pending pack after this chunk is persisted (bounds the pending index).
    pub flush_pack: bool,
}

/// Decides chunk boundaries. Must be deterministic enough to be useful, but nothing about
/// the world's contents may depend on it.
pub trait ChunkPolicy: Send + Sync + Debug {
    fn chunk_plan(&self, input: &ChunkInput) -> ChunkDecision;
}

/// The v1 heuristic (see module docs): size each chunk so its estimated hydrated bytes fit
/// the budget, never plan a chunk below [`MIN_CHUNK_OPS`], and leave small batches whole.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeuristicChunkPolicy;

impl ChunkPolicy for HeuristicChunkPolicy {
    fn chunk_plan(&self, i: &ChunkInput) -> ChunkDecision {
        let remaining = i.ops_total.saturating_sub(i.ops_done);
        let flush_pack = i.pending_index_bytes > PENDING_FLUSH_INDEX_BYTES;
        let whole = ChunkDecision {
            ops: remaining,
            flush_pack: false,
        };
        if i.budget_bytes == 0 {
            return whole;
        }
        if i.ops_done == 0 {
            // Engage only when the batch is estimated to exceed the budget and is big
            // enough to be worth splitting: one-row and small batches are untouched.
            let est = (i.ops_total as u64)
                .saturating_mul(FIRST_CHUNK_NODES_PER_OP)
                .saturating_mul(HYDRATED_NODE_BYTES);
            if est <= i.budget_bytes || i.ops_total < 2 * MIN_CHUNK_OPS {
                return whole;
            }
        }
        let per_op = if i.prev_chunk_ops > 0 && i.prev_nodes_written > 0 {
            // Measured nodes per op of the previous chunk (at least one node per op).
            ((i.prev_nodes_written as u64).saturating_mul(HYDRATED_NODE_BYTES)
                / i.prev_chunk_ops as u64)
                .max(HYDRATED_NODE_BYTES)
        } else {
            FIRST_CHUNK_NODES_PER_OP * HYDRATED_NODE_BYTES
        };
        let mut ops = ((i.budget_bytes / per_op) as usize).max(MIN_CHUNK_OPS);
        // A remainder smaller than half a minimum chunk is taken whole.
        if remaining.saturating_sub(ops) < MIN_CHUNK_OPS / 2 {
            ops = remaining;
        }
        ChunkDecision {
            ops: ops.min(remaining),
            flush_pack,
        }
    }
}

/// Fixed-size chunks (benchmarks, tests). `0` plans one chunk.
#[derive(Clone, Copy, Debug)]
pub struct FixedChunkPolicy(pub usize);

impl ChunkPolicy for FixedChunkPolicy {
    fn chunk_plan(&self, i: &ChunkInput) -> ChunkDecision {
        let remaining = i.ops_total.saturating_sub(i.ops_done);
        ChunkDecision {
            ops: if self.0 == 0 { remaining } else { self.0 },
            flush_pack: i.pending_index_bytes > PENDING_FLUSH_INDEX_BYTES,
        }
    }
}
