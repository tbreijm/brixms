//! Persistent key→value store with structural sharing (ADR-0002 §9.2
//! "State", ADR-0046, P1):
//!
//! > Persistent HAMT-style maps with structural sharing.
//!
//! This module provides two implementations of the [`PersistentMap`] trait:
//! 1. [`TrieMap`] (also aliased as [`HamtMap`]): A 16-way branching, path-compressed,
//!    persistent radix trie (HAMT) with node-level structural sharing, incremental Merkle
//!    root hashing via `brix-canon`, deterministic collision buckets, and O(log n)
//!    copy-on-write paths.
//! 2. [`ArcMap`]: The v1 snapshot implementation (`Arc<BTreeMap>`), retained for
//!    backwards compatibility and as a deliberate O(n) negative control in scale tests.
//!
//! **Ring-0 TCB Whitelist & Safe Rust.** This module depends strictly on `brix-canon`
//! and `brix-semantic` only (no external collection crates like `im` or `rpds`).
//! All structures are implemented in 100% safe Rust under the workspace `unsafe_code = "deny"`
//! lint policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use brix_canon::{CanonDecode, CanonError, CanonReader, CanonWriter, Canonical, Digest, Domain};

/// A persistent (immutable, versioned) key→value map. `insert` takes `&self`
/// and returns a *new* map value — the receiver is untouched, so every
/// snapshot a caller still holds remains valid after another caller derives
/// a new version from it.
pub trait PersistentMap<K, V>: Clone {
    /// The empty map.
    fn new() -> Self;
    /// Look up `key`.
    fn get(&self, key: &K) -> Option<&V>;
    /// Return a new map with `key` bound to `value`; `self` is unchanged.
    fn insert(&self, key: K, value: V) -> Self;
    /// Return a new map with `key` removed; `self` is unchanged.
    fn remove(&self, key: &K) -> Self;
    /// Number of bindings.
    fn len(&self) -> usize;
    /// Whether the map has no bindings.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// v1 Scaffolding: ArcMap (retained for compatibility & negative controls)
// ---------------------------------------------------------------------------

/// The **v1** [`PersistentMap`]: an `Arc`-shared immutable `BTreeMap`
/// snapshot. Clones the whole map on `insert`/`remove` (O(n)).
#[derive(Debug)]
pub struct ArcMap<K, V>(Arc<BTreeMap<K, V>>);

impl<K, V> Clone for ArcMap<K, V> {
    /// O(1): bumps the `Arc` refcount, does not touch the map's contents.
    fn clone(&self) -> Self {
        ArcMap(Arc::clone(&self.0))
    }
}

impl<K, V> ArcMap<K, V> {
    /// Whether two snapshots share the same underlying allocation (`Arc`
    /// pointer equality).
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<K: Ord + Clone, V: Clone> PersistentMap<K, V> for ArcMap<K, V> {
    fn new() -> Self {
        ArcMap(Arc::new(BTreeMap::new()))
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.0.get(key)
    }

    fn insert(&self, key: K, value: V) -> Self {
        let mut next = (*self.0).clone();
        next.insert(key, value);
        ArcMap(Arc::new(next))
    }

    fn remove(&self, key: &K) -> Self {
        let mut next = (*self.0).clone();
        next.remove(key);
        ArcMap(Arc::new(next))
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

// ---------------------------------------------------------------------------
// Key Hashers (Injectable for collision verification, ADR-0046 §3.3)
// ---------------------------------------------------------------------------

/// Hasher for trie keys.
pub trait KeyHasher: Clone {
    /// Hash a canonical key into a 32-byte (256-bit) digest.
    fn hash_key<K: Canonical>(&self, key: &K) -> [u8; 32];
}

/// The standard canonical key hasher, deriving a 256-bit BLAKE3 digest in [`Domain::Value`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CanonHasher;

impl KeyHasher for CanonHasher {
    fn hash_key<K: Canonical>(&self, key: &K) -> [u8; 32] {
        *key.canon_digest(Domain::Value).as_bytes()
    }
}

/// Injectable test hasher that folds key hashes modulo `modulus` into the first 8 bytes
/// and zeroes out the rest. Used to deterministically induce and test hash collisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuloHasher {
    pub modulus: u64,
}

impl ModuloHasher {
    pub const fn new(modulus: u64) -> Self {
        Self { modulus }
    }
}

impl Default for ModuloHasher {
    fn default() -> Self {
        Self { modulus: 1 }
    }
}

impl KeyHasher for ModuloHasher {
    fn hash_key<K: Canonical>(&self, key: &K) -> [u8; 32] {
        let full = key.canon_digest(Domain::Value);
        let mut out = [0u8; 32];
        let val = u64::from_be_bytes(full.as_bytes()[0..8].try_into().unwrap());
        let rem = if self.modulus > 0 {
            val % self.modulus
        } else {
            0
        };
        out[0..8].copy_from_slice(&rem.to_be_bytes());
        out
    }
}

// ---------------------------------------------------------------------------
// Structural Cost Statistics (P1 structural bounds, ADR-0046 §3.3, §6)
// ---------------------------------------------------------------------------

/// Physical structural operation counters observed during a trie operation.
/// Every counter represents a real, discrete event executed by the tree walk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrieOpStats {
    /// Number of tree nodes allocated (`Arc<Node>` instances created along the path).
    pub nodes_allocated: usize,
    /// Number of tree nodes inspected during traversal.
    pub nodes_visited: usize,
    /// Number of Merkle node hashes computed.
    pub nodes_hashed: usize,
    /// Number of full key comparisons performed.
    pub key_comparisons: usize,
}

// ---------------------------------------------------------------------------
// Trie Nodes
// ---------------------------------------------------------------------------

#[inline]
pub(crate) fn get_nibble(hash: &[u8; 32], index: usize) -> u8 {
    let byte = hash[index / 2];
    if index.is_multiple_of(2) {
        (byte >> 4) & 0x0f
    } else {
        byte & 0x0f
    }
}

pub(crate) fn common_prefix_len(h1: &[u8; 32], h2: &[u8; 32], start: usize) -> usize {
    let mut len = 0;
    while start + len < 64 {
        if get_nibble(h1, start + len) == get_nibble(h2, start + len) {
            len += 1;
        } else {
            break;
        }
    }
    len
}

/// A leaf node holding a single key-value binding.
#[derive(Clone, Debug)]
pub struct LeafNode<K, V> {
    pub key: K,
    pub value: Arc<V>,
    pub hash: [u8; 32],
    pub digest: Digest,
}

impl<K: Canonical, V: Canonical> LeafNode<K, V> {
    pub fn new(key: K, value: Arc<V>, hash: [u8; 32]) -> Self {
        let digest = Self::compute_digest(&key, &value, &hash);
        Self {
            key,
            value,
            hash,
            digest,
        }
    }

    fn compute_digest(key: &K, value: &V, hash: &[u8; 32]) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag("brix:trie:leaf:v1");
        w.write_bytes(hash);
        key.canon_write(&mut w);
        value.canon_write(&mut w);
        w.digest(Domain::Value)
    }
}

/// A collision node holding two or more distinct keys that share the exact same 32-byte hash.
/// Entries are sorted strictly by `key.cmp(...)` (canonical key order).
#[derive(Clone, Debug)]
pub struct CollisionNode<K, V> {
    pub hash: [u8; 32],
    pub entries: Vec<(K, Arc<V>)>,
    pub digest: Digest,
}

impl<K: Canonical + Ord, V: Canonical> CollisionNode<K, V> {
    pub fn new(hash: [u8; 32], mut entries: Vec<(K, Arc<V>)>) -> Self {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let digest = Self::compute_digest(&hash, &entries);
        Self {
            hash,
            entries,
            digest,
        }
    }

    fn compute_digest(hash: &[u8; 32], entries: &[(K, Arc<V>)]) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag("brix:trie:collision:v1");
        w.write_bytes(hash);
        w.write_uint(entries.len() as u64);
        for (k, v) in entries {
            k.canon_write(&mut w);
            v.canon_write(&mut w);
        }
        w.digest(Domain::Value)
    }
}

/// A 16-way branching internal node with path compression.
///
/// Invariant: `children.len() >= 2` (never 0 or 1). If deletions drop children to 1,
/// the branch contracts into that child (merging prefixes if the child is another branch).
#[derive(Clone, Debug)]
pub struct BranchNode<K, V> {
    pub prefix: Vec<u8>,
    pub bitmap: u16,
    pub children: Vec<Arc<Node<K, V>>>,
    pub digest: Digest,
}

impl<K, V> BranchNode<K, V> {
    pub fn new(prefix: Vec<u8>, bitmap: u16, children: Vec<Arc<Node<K, V>>>) -> Self {
        debug_assert!(
            children.len() >= 2,
            "branch node must have at least 2 children"
        );
        debug_assert_eq!(bitmap.count_ones() as usize, children.len());
        let digest = Self::compute_digest(&prefix, bitmap, &children);
        Self {
            prefix,
            bitmap,
            children,
            digest,
        }
    }

    fn compute_digest(prefix: &[u8], bitmap: u16, children: &[Arc<Node<K, V>>]) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag("brix:trie:branch:v1");
        w.write_uint(prefix.len() as u64);
        for &nib in prefix {
            w.write_uint(nib as u64);
        }
        w.write_uint(bitmap as u64);
        for child in children {
            w.write_bytes(child.digest().as_bytes());
        }
        w.digest(Domain::Value)
    }

    #[inline]
    pub fn child_index(&self, nibble: u8) -> Option<usize> {
        let mask = 1u16 << nibble;
        if (self.bitmap & mask) == 0 {
            None
        } else {
            let lower_mask = mask - 1;
            Some((self.bitmap & lower_mask).count_ones() as usize)
        }
    }

    #[inline]
    pub fn get_child(&self, nibble: u8) -> Option<&Arc<Node<K, V>>> {
        self.child_index(nibble).map(|idx| &self.children[idx])
    }
}

/// An enum representing any node in the persistent trie.
#[derive(Clone, Debug)]
pub enum Node<K, V> {
    Leaf(LeafNode<K, V>),
    Collision(CollisionNode<K, V>),
    Branch(BranchNode<K, V>),
    Lazy(Digest),
}

impl<K, V> Node<K, V> {
    pub fn digest(&self) -> Digest {
        match self {
            Node::Leaf(l) => l.digest,
            Node::Collision(c) => c.digest,
            Node::Branch(b) => b.digest,
            Node::Lazy(d) => *d,
        }
    }
}

// ---------------------------------------------------------------------------
// Trie Traversal Algorithms (Safe Path Copying + Contraction)
// ---------------------------------------------------------------------------

fn get_recursive<'a, K: Canonical + Ord, V: Canonical>(
    node: &'a Node<K, V>,
    key: &K,
    hash: &[u8; 32],
    depth: usize,
    stats: &mut TrieOpStats,
) -> Option<&'a V> {
    stats.nodes_visited += 1;
    match node {
        Node::Leaf(leaf) => {
            if leaf.hash == *hash {
                stats.key_comparisons += 1;
                if leaf.key == *key {
                    return Some(&*leaf.value);
                }
            }
            None
        }
        Node::Collision(coll) => {
            if coll.hash == *hash {
                for (k, v) in &coll.entries {
                    stats.key_comparisons += 1;
                    if k == key {
                        return Some(&**v);
                    }
                }
            }
            None
        }
        Node::Branch(branch) => {
            if depth + branch.prefix.len() > 64 {
                return None;
            }
            for (i, &nib) in branch.prefix.iter().enumerate() {
                if get_nibble(hash, depth + i) != nib {
                    return None;
                }
            }
            let branch_point = depth + branch.prefix.len();
            if branch_point >= 64 {
                return None;
            }
            let nibble = get_nibble(hash, branch_point);
            match branch.get_child(nibble) {
                Some(child) => get_recursive(child, key, hash, branch_point + 1, stats),
                None => None,
            }
        }
        Node::Lazy(_) => None,
    }
}

fn insert_recursive<K: Canonical + Ord + Clone, V: Canonical + Clone>(
    node: &Arc<Node<K, V>>,
    key: K,
    value: Arc<V>,
    hash: [u8; 32],
    depth: usize,
    stats: &mut TrieOpStats,
) -> (Arc<Node<K, V>>, bool) {
    stats.nodes_visited += 1;
    match &**node {
        Node::Leaf(leaf) => {
            if leaf.hash == hash {
                stats.key_comparisons += 1;
                if leaf.key == key {
                    // Key overwrite
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    (Arc::new(Node::Leaf(LeafNode::new(key, value, hash))), false)
                } else {
                    // Hash collision! Form collision bucket
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let entries = vec![(leaf.key.clone(), leaf.value.clone()), (key, value)];
                    (
                        Arc::new(Node::Collision(CollisionNode::new(hash, entries))),
                        true,
                    )
                }
            } else {
                let lcp = common_prefix_len(&leaf.hash, &hash, depth);
                let branch_point = depth + lcp;
                debug_assert!(branch_point < 64);
                let nibble_existing = get_nibble(&leaf.hash, branch_point);
                let nibble_new = get_nibble(&hash, branch_point);
                debug_assert_ne!(nibble_existing, nibble_new);

                stats.nodes_allocated += 2;
                stats.nodes_hashed += 2;
                let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));
                let prefix: Vec<u8> = (depth..branch_point)
                    .map(|i| get_nibble(&hash, i))
                    .collect();
                let bitmap = (1u16 << nibble_existing) | (1u16 << nibble_new);
                let children = if nibble_existing < nibble_new {
                    vec![node.clone(), new_leaf]
                } else {
                    vec![new_leaf, node.clone()]
                };
                (
                    Arc::new(Node::Branch(BranchNode::new(prefix, bitmap, children))),
                    true,
                )
            }
        }
        Node::Collision(coll) => {
            if coll.hash == hash {
                let mut entries = coll.entries.clone();
                let mut found = false;
                for entry in &mut entries {
                    stats.key_comparisons += 1;
                    if entry.0 == key {
                        entry.1 = value.clone();
                        found = true;
                        break;
                    }
                }
                if !found {
                    entries.push((key, value));
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                }
                stats.nodes_allocated += 1;
                stats.nodes_hashed += 1;
                (
                    Arc::new(Node::Collision(CollisionNode::new(hash, entries))),
                    !found,
                )
            } else {
                let lcp = common_prefix_len(&coll.hash, &hash, depth);
                let branch_point = depth + lcp;
                debug_assert!(branch_point < 64);
                let nibble_existing = get_nibble(&coll.hash, branch_point);
                let nibble_new = get_nibble(&hash, branch_point);
                debug_assert_ne!(nibble_existing, nibble_new);

                stats.nodes_allocated += 2;
                stats.nodes_hashed += 2;
                let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));
                let prefix: Vec<u8> = (depth..branch_point)
                    .map(|i| get_nibble(&hash, i))
                    .collect();
                let bitmap = (1u16 << nibble_existing) | (1u16 << nibble_new);
                let children = if nibble_existing < nibble_new {
                    vec![node.clone(), new_leaf]
                } else {
                    vec![new_leaf, node.clone()]
                };
                (
                    Arc::new(Node::Branch(BranchNode::new(prefix, bitmap, children))),
                    true,
                )
            }
        }
        Node::Branch(branch) => {
            let mut match_len = 0;
            while match_len < branch.prefix.len() {
                if get_nibble(&hash, depth + match_len) != branch.prefix[match_len] {
                    break;
                }
                match_len += 1;
            }

            if match_len < branch.prefix.len() {
                // Diverged in common prefix! Split this branch node
                let branch_nibble = branch.prefix[match_len];
                let new_nibble = get_nibble(&hash, depth + match_len);

                let lower_prefix = branch.prefix[match_len + 1..].to_vec();
                stats.nodes_allocated += 3;
                stats.nodes_hashed += 3;
                let lower_branch = Arc::new(Node::Branch(BranchNode::new(
                    lower_prefix,
                    branch.bitmap,
                    branch.children.clone(),
                )));
                let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));

                let upper_prefix = branch.prefix[..match_len].to_vec();
                let upper_bitmap = (1u16 << branch_nibble) | (1u16 << new_nibble);
                let upper_children = if branch_nibble < new_nibble {
                    vec![lower_branch, new_leaf]
                } else {
                    vec![new_leaf, lower_branch]
                };
                (
                    Arc::new(Node::Branch(BranchNode::new(
                        upper_prefix,
                        upper_bitmap,
                        upper_children,
                    ))),
                    true,
                )
            } else {
                let branch_point = depth + branch.prefix.len();
                debug_assert!(branch_point < 64);
                let nibble = get_nibble(&hash, branch_point);

                match branch.child_index(nibble) {
                    None => {
                        let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));
                        stats.nodes_allocated += 2;
                        stats.nodes_hashed += 2;
                        let new_bitmap = branch.bitmap | (1u16 << nibble);
                        let insert_pos =
                            (branch.bitmap & ((1u16 << nibble) - 1)).count_ones() as usize;
                        let mut new_children = branch.children.clone();
                        new_children.insert(insert_pos, new_leaf);
                        (
                            Arc::new(Node::Branch(BranchNode::new(
                                branch.prefix.clone(),
                                new_bitmap,
                                new_children,
                            ))),
                            true,
                        )
                    }
                    Some(idx) => {
                        let (new_child, added) = insert_recursive(
                            &branch.children[idx],
                            key,
                            value,
                            hash,
                            branch_point + 1,
                            stats,
                        );
                        stats.nodes_allocated += 1;
                        stats.nodes_hashed += 1;
                        let mut new_children = branch.children.clone();
                        new_children[idx] = new_child;
                        (
                            Arc::new(Node::Branch(BranchNode::new(
                                branch.prefix.clone(),
                                branch.bitmap,
                                new_children,
                            ))),
                            added,
                        )
                    }
                }
            }
        }
        Node::Lazy(_) => panic!("cannot insert into unhydrated lazy node; use insert_with_store"),
    }
}

fn remove_recursive<K: Canonical + Ord + Clone, V: Canonical + Clone>(
    node: &Arc<Node<K, V>>,
    key: &K,
    hash: &[u8; 32],
    depth: usize,
    stats: &mut TrieOpStats,
) -> (Option<Arc<Node<K, V>>>, bool) {
    stats.nodes_visited += 1;
    match &**node {
        Node::Leaf(leaf) => {
            if leaf.hash == *hash {
                stats.key_comparisons += 1;
                if leaf.key == *key {
                    return (None, true);
                }
            }
            (Some(node.clone()), false)
        }
        Node::Collision(coll) => {
            if coll.hash == *hash {
                let mut entries = coll.entries.clone();
                let mut removed = false;
                for i in 0..entries.len() {
                    stats.key_comparisons += 1;
                    if entries[i].0 == *key {
                        entries.remove(i);
                        removed = true;
                        break;
                    }
                }
                if !removed {
                    return (Some(node.clone()), false);
                }
                if entries.is_empty() {
                    return (None, true);
                } else if entries.len() == 1 {
                    // Contract collision bucket to single Leaf
                    let (k, v) = entries.pop().unwrap();
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let leaf = LeafNode::new(k, v, coll.hash);
                    return (Some(Arc::new(Node::Leaf(leaf))), true);
                } else {
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let coll = CollisionNode::new(coll.hash, entries);
                    return (Some(Arc::new(Node::Collision(coll))), true);
                }
            }
            (Some(node.clone()), false)
        }
        Node::Branch(branch) => {
            for (i, &nib) in branch.prefix.iter().enumerate() {
                if get_nibble(hash, depth + i) != nib {
                    return (Some(node.clone()), false);
                }
            }
            let branch_point = depth + branch.prefix.len();
            if branch_point >= 64 {
                return (Some(node.clone()), false);
            }
            let nibble = get_nibble(hash, branch_point);
            let idx = match branch.child_index(nibble) {
                Some(idx) => idx,
                None => return (Some(node.clone()), false),
            };

            let (opt_child, removed) =
                remove_recursive(&branch.children[idx], key, hash, branch_point + 1, stats);

            if !removed {
                return (Some(node.clone()), false);
            }

            match opt_child {
                Some(new_child) => {
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let mut new_children = branch.children.clone();
                    new_children[idx] = new_child;
                    (
                        Some(Arc::new(Node::Branch(BranchNode::new(
                            branch.prefix.clone(),
                            branch.bitmap,
                            new_children,
                        )))),
                        true,
                    )
                }
                None => {
                    let new_bitmap = branch.bitmap & !(1u16 << nibble);
                    let mut new_children = branch.children.clone();
                    new_children.remove(idx);

                    if new_children.len() >= 2 {
                        stats.nodes_allocated += 1;
                        stats.nodes_hashed += 1;
                        (
                            Some(Arc::new(Node::Branch(BranchNode::new(
                                branch.prefix.clone(),
                                new_bitmap,
                                new_children,
                            )))),
                            true,
                        )
                    } else {
                        // Contraction rule: collapse single child
                        let single_child = new_children.pop().unwrap();
                        let remaining_nibble = new_bitmap.trailing_zeros() as u8;

                        match &*single_child {
                            Node::Leaf(_) | Node::Collision(_) | Node::Lazy(_) => {
                                (Some(single_child), true)
                            }
                            Node::Branch(child_branch) => {
                                // Merge branch prefixes!
                                let mut merged_prefix = branch.prefix.clone();
                                merged_prefix.push(remaining_nibble);
                                merged_prefix.extend_from_slice(&child_branch.prefix);
                                stats.nodes_allocated += 1;
                                stats.nodes_hashed += 1;
                                let merged = BranchNode::new(
                                    merged_prefix,
                                    child_branch.bitmap,
                                    child_branch.children.clone(),
                                );
                                (Some(Arc::new(Node::Branch(merged))), true)
                            }
                        }
                    }
                }
            }
        }
        Node::Lazy(_) => (Some(node.clone()), false),
    }
}

fn collect_entries<'a, K, V>(node: &'a Node<K, V>, out: &mut Vec<(&'a K, &'a V)>) {
    match node {
        Node::Leaf(leaf) => {
            out.push((&leaf.key, &*leaf.value));
        }
        Node::Collision(coll) => {
            for (k, v) in &coll.entries {
                out.push((k, &**v));
            }
        }
        Node::Branch(branch) => {
            for child in &branch.children {
                collect_entries(child, out);
            }
        }
        Node::Lazy(_) => {}
    }
}

// ---------------------------------------------------------------------------
// TrieMap (PersistentMap implementation)
// ---------------------------------------------------------------------------

/// A persistent, structural-sharing, path-copying radix trie (HAMT) backing [`PersistentMap`].
///
/// Properties:
/// - **O(log n) Updates**: `insert` and `remove` copy only the path of nodes to the target leaf.
/// - **Canonical Incremental Root Digest**: Computes a Merkle root digest over canonical node
///   encodings via `brix-canon`. Subtrees are hashed once and shared.
/// - **Canonical Root Invariant**: Equal sets of key-value bindings produce byte-for-byte identical
///   root digests regardless of insertion order, deletion history, or batching.
/// - **Safe Rust**: 100% safe Rust under `unsafe_code = "deny"`.
#[derive(Clone, Debug)]
pub struct TrieMap<K, V, H = CanonHasher> {
    root: Option<Arc<Node<K, V>>>,
    len: usize,
    hasher: H,
}

/// An alias for [`TrieMap`] indicating its role as the ratified HAMT persistent map.
pub type HamtMap<K, V, H = CanonHasher> = TrieMap<K, V, H>;

impl<K, V> Default for TrieMap<K, V, CanonHasher> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> TrieMap<K, V, CanonHasher> {
    /// Create a fresh, empty trie map using the default [`CanonHasher`].
    pub fn new() -> Self {
        Self {
            root: None,
            len: 0,
            hasher: CanonHasher,
        }
    }
}

impl<K, V, H: KeyHasher> TrieMap<K, V, H> {
    /// Create an empty map with a custom key hasher (e.g. [`ModuloHasher`] for testing).
    pub fn with_hasher(hasher: H) -> Self {
        Self {
            root: None,
            len: 0,
            hasher,
        }
    }

    /// Number of bindings in the map.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the map has no bindings.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Canonical root digest of the map.
    ///
    /// For an empty map, returns the fixed canonical empty digest `Digest::of(Domain::Value, b"brix:trie:empty:v1")`.
    /// For a non-empty map, returns the Merkle digest of the root node.
    pub fn root_digest(&self) -> Digest {
        match &self.root {
            None => Digest::of(Domain::Value, b"brix:trie:empty:v1"),
            Some(node) => node.digest(),
        }
    }

    /// Whether two snapshots share the exact same root allocation (`Arc` pointer equality).
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl<K: Canonical + Ord + Clone, V: Canonical + Clone, H: KeyHasher> TrieMap<K, V, H> {
    /// Look up `key` in the map.
    pub fn get(&self, key: &K) -> Option<&V> {
        let mut stats = TrieOpStats::default();
        self.get_with_stats(key, &mut stats)
    }

    /// Look up `key` while observing structural operation statistics.
    pub fn get_with_stats(&self, key: &K, stats: &mut TrieOpStats) -> Option<&V> {
        let root = self.root.as_ref()?;
        let hash = self.hasher.hash_key(key);
        get_recursive(root, key, &hash, 0, stats)
    }

    /// Whether `key` is present in the map.
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Return a new map with `key` bound to `value`; `self` is unchanged.
    pub fn insert(&self, key: K, value: V) -> Self {
        let (next, _stats) = self.insert_with_stats(key, value);
        next
    }

    /// Return a new map with `key` bound to `value`, accompanied by the exact structural operation statistics.
    pub fn insert_with_stats(&self, key: K, value: V) -> (Self, TrieOpStats) {
        let mut stats = TrieOpStats::default();
        let hash = self.hasher.hash_key(&key);
        let val_arc = Arc::new(value);

        match &self.root {
            None => {
                stats.nodes_allocated += 1;
                stats.nodes_hashed += 1;
                let leaf = LeafNode::new(key, val_arc, hash);
                let next = Self {
                    root: Some(Arc::new(Node::Leaf(leaf))),
                    len: 1,
                    hasher: self.hasher.clone(),
                };
                (next, stats)
            }
            Some(root) => {
                let (new_root, added) = insert_recursive(root, key, val_arc, hash, 0, &mut stats);
                let next = Self {
                    root: Some(new_root),
                    len: if added { self.len + 1 } else { self.len },
                    hasher: self.hasher.clone(),
                };
                (next, stats)
            }
        }
    }

    /// Return a new map with `key` removed; `self` is unchanged.
    pub fn remove(&self, key: &K) -> Self {
        let (next, _stats) = self.remove_with_stats(key);
        next
    }

    /// Return a new map with `key` removed, accompanied by the exact structural operation statistics.
    pub fn remove_with_stats(&self, key: &K) -> (Self, TrieOpStats) {
        let mut stats = TrieOpStats::default();
        let root = match &self.root {
            None => return (self.clone(), stats),
            Some(r) => r,
        };
        let hash = self.hasher.hash_key(key);
        let (new_root, removed) = remove_recursive(root, key, &hash, 0, &mut stats);
        if !removed {
            return (self.clone(), stats);
        }
        let next = Self {
            root: new_root,
            len: self.len - 1,
            hasher: self.hasher.clone(),
        };
        (next, stats)
    }

    /// Iterate over key-value pairs in deterministic depth-first hash/bucket order.
    pub fn iter(&self) -> Vec<(&K, &V)> {
        let mut out = Vec::with_capacity(self.len);
        if let Some(root) = &self.root {
            collect_entries(root, &mut out);
        }
        out
    }

    /// Persist all unwritten nodes in this tree to `store`.
    /// Returns the number of new nodes written.
    pub fn persist_to_store<S: NodeStore>(&self, store: &mut S) -> usize {
        let mut count = 0;
        if let Some(root) = &self.root {
            persist_node_recursive(root, store, &mut count);
        }
        count
    }
}

impl<K: Canonical + Ord + Clone, V: Canonical + Clone, H: KeyHasher + Default> PersistentMap<K, V>
    for TrieMap<K, V, H>
{
    fn new() -> Self {
        Self::with_hasher(H::default())
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.get(key)
    }

    fn insert(&self, key: K, value: V) -> Self {
        self.insert(key, value)
    }

    fn remove(&self, key: &K) -> Self {
        self.remove(key)
    }

    fn len(&self) -> usize {
        self.len
    }
}

impl<K: Canonical + Ord + Clone, V: Canonical + Clone, H: KeyHasher> Canonical
    for TrieMap<K, V, H>
{
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.root_digest().as_bytes());
    }
}

impl<K: Canonical + Ord + Clone, V: Canonical + Clone, H: KeyHasher> PartialEq
    for TrieMap<K, V, H>
{
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            true
        } else if self.len != other.len {
            false
        } else {
            self.root_digest() == other.root_digest()
        }
    }
}

impl<K: Canonical + Ord + Clone, V: Canonical + Clone, H: KeyHasher> Eq for TrieMap<K, V, H> {}

impl<K: Canonical + Ord + Clone, V: Canonical + Clone, H: KeyHasher + Default> FromIterator<(K, V)>
    for TrieMap<K, V, H>
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map = map.insert(k, v);
        }
        map
    }
}

impl<
        K: Canonical + Ord + Clone + CanonDecode,
        V: Canonical + Clone + CanonDecode,
        H: KeyHasher,
    > TrieMap<K, V, H>
{
    /// Construct a persistent trie from its Merkle root digest and binding count.
    /// The root is lazy and will be hydrated from store on demand.
    pub fn from_root_digest(digest: Digest, len: usize) -> Self
    where
        H: Default,
    {
        let empty_d = Digest::of(brix_canon::Domain::Value, b"brix:trie:empty:v1");
        Self {
            root: if digest == empty_d {
                None
            } else {
                Some(Arc::new(Node::Lazy(digest)))
            },
            len,
            hasher: H::default(),
        }
    }

    /// Look up `key`, resolving lazy nodes from `store` on demand.
    /// Returns `Err(StorageError)` if a required node is missing or corrupted.
    pub fn get_with_store<S: NodeStore>(
        &self,
        key: &K,
        store: &S,
    ) -> Result<Option<V>, StorageError> {
        let Some(root) = self.root.as_ref() else {
            return Ok(None);
        };
        let hash = self.hasher.hash_key(key);
        let mut current = root.clone();
        let mut depth = 0;

        loop {
            current = resolve_node(&current, store)?;

            match &*current {
                Node::Leaf(leaf) => {
                    if leaf.hash == hash && leaf.key == *key {
                        return Ok(Some((*leaf.value).clone()));
                    }
                    return Ok(None);
                }
                Node::Collision(coll) => {
                    if coll.hash == hash {
                        for (k, v) in &coll.entries {
                            if k == key {
                                return Ok(Some((**v).clone()));
                            }
                        }
                    }
                    return Ok(None);
                }
                Node::Branch(branch) => {
                    for (i, &nib) in branch.prefix.iter().enumerate() {
                        if get_nibble(&hash, depth + i) != nib {
                            return Ok(None);
                        }
                    }
                    let branch_point = depth + branch.prefix.len();
                    if branch_point >= 64 {
                        return Ok(None);
                    }
                    let nibble = get_nibble(&hash, branch_point);
                    let Some(child_idx) = branch.child_index(nibble) else {
                        return Ok(None);
                    };
                    current = branch.children[child_idx].clone();
                    depth = branch_point + 1;
                }
                Node::Lazy(_) => unreachable!(),
            }
        }
    }

    /// Insert `key -> value` into the map, resolving lazy nodes along the insertion path from `store`.
    pub fn insert_with_store<S: NodeStore>(
        &self,
        key: K,
        value: V,
        store: &S,
    ) -> Result<(Self, TrieOpStats), StorageError> {
        let mut stats = TrieOpStats::default();
        let hash = self.hasher.hash_key(&key);
        let val_arc = Arc::new(value);

        match &self.root {
            None => {
                stats.nodes_allocated += 1;
                stats.nodes_hashed += 1;
                let leaf = LeafNode::new(key, val_arc, hash);
                let next = Self {
                    root: Some(Arc::new(Node::Leaf(leaf))),
                    len: 1,
                    hasher: self.hasher.clone(),
                };
                Ok((next, stats))
            }
            Some(root) => {
                let (new_root, added) =
                    insert_recursive_store(root, key, val_arc, hash, 0, &mut stats, store)?;
                let next = Self {
                    root: Some(new_root),
                    len: if added { self.len + 1 } else { self.len },
                    hasher: self.hasher.clone(),
                };
                Ok((next, stats))
            }
        }
    }

    /// Remove `key` from the map, resolving lazy nodes along the removal path from `store`.
    pub fn remove_with_store<S: NodeStore>(
        &self,
        key: &K,
        store: &S,
    ) -> Result<(Self, TrieOpStats), StorageError> {
        let mut stats = TrieOpStats::default();
        let Some(root) = &self.root else {
            return Ok((self.clone(), stats));
        };

        let hash = self.hasher.hash_key(key);
        let (opt_root, removed) = remove_recursive_store(root, key, &hash, 0, &mut stats, store)?;

        if !removed {
            return Ok((self.clone(), stats));
        }

        let next = Self {
            root: opt_root,
            len: self.len.saturating_sub(1),
            hasher: self.hasher.clone(),
        };
        Ok((next, stats))
    }

    /// Paginate entries in trie traversal order, resolving lazy subtrees from `store` only as needed.
    pub fn iter_page_with_store<S: NodeStore>(
        &self,
        cursor: Option<(&[u8; 32], &K)>,
        limit: usize,
        store: &S,
    ) -> Result<(Vec<(K, V)>, Option<([u8; 32], K)>), StorageError> {
        let mut out = Vec::with_capacity(limit.min(1024));
        if limit == 0 {
            return Ok((out, None));
        }
        let Some(root) = &self.root else {
            return Ok((out, None));
        };
        let mut done = false;
        iter_page_recursive(
            root,
            cursor,
            limit.saturating_add(1),
            &mut out,
            &mut done,
            store,
            0,
        )?;
        let next_cursor = if out.len() > limit {
            out.pop();
            out.last()
                .map(|(k, _)| (self.hasher.hash_key(k), k.clone()))
        } else {
            None
        };
        Ok((out, next_cursor))
    }
}

// ---------------------------------------------------------------------------
// Durable Node Storage & Serialization (P1 durability seam, ADR-0046 §3.3)
// ---------------------------------------------------------------------------

/// Content-addressed storage for persistent trie nodes.
pub trait NodeStore {
    /// Retrieve raw serialized bytes of a node by its Merkle [`Digest`].
    fn get_node(&self, digest: &Digest) -> Option<Vec<u8>>;
    /// Store raw serialized bytes for a node under its Merkle [`Digest`].
    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>);
    /// Whether `digest` exists in the store.
    fn contains(&self, digest: &Digest) -> bool {
        self.get_node(digest).is_some()
    }
}

/// In-memory content-addressed node store.
#[derive(Default, Clone, Debug)]
pub struct MemoryNodeStore {
    nodes: BTreeMap<Digest, Vec<u8>>,
}

impl MemoryNodeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl NodeStore for MemoryNodeStore {
    fn get_node(&self, digest: &Digest) -> Option<Vec<u8>> {
        self.nodes.get(digest).cloned()
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        self.nodes.insert(digest, bytes);
    }
}

/// Serialize a trie node to its canonical durable byte format.
pub fn encode_node<K: Canonical, V: Canonical>(node: &Node<K, V>) -> Vec<u8> {
    let mut w = CanonWriter::new();
    match node {
        Node::Leaf(leaf) => {
            w.write_uint(1); // 1 = Leaf
            w.write_bytes(&leaf.hash);
            w.write_bytes(&leaf.key.canon_bytes());
            w.write_bytes(&leaf.value.canon_bytes());
        }
        Node::Collision(coll) => {
            w.write_uint(2); // 2 = Collision
            w.write_bytes(&coll.hash);
            w.write_uint(coll.entries.len() as u64);
            for (k, v) in &coll.entries {
                w.write_bytes(&k.canon_bytes());
                w.write_bytes(&v.canon_bytes());
            }
        }
        Node::Branch(branch) => {
            w.write_uint(3); // 3 = Branch
            w.write_uint(branch.prefix.len() as u64);
            for &nib in &branch.prefix {
                w.write_uint(nib as u64);
            }
            w.write_uint(branch.bitmap as u64);
            w.write_uint(branch.children.len() as u64);
            for child in &branch.children {
                w.write_bytes(child.digest().as_bytes());
            }
        }
        Node::Lazy(d) => {
            w.write_uint(4); // 4 = Lazy stub
            w.write_bytes(d.as_bytes());
        }
    }
    w.finish()
}

/// Deserialize a trie node from its canonical durable byte format.
/// Storage errors when resolving lazy trie nodes from a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    MissingNode(Digest),
    CorruptedNode(Digest),
    DecodeError(CanonError),
    Io(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingNode(d) => write!(f, "missing node: {}", d.to_hex()),
            Self::CorruptedNode(d) => write!(f, "corrupted node: {}", d.to_hex()),
            Self::DecodeError(e) => write!(f, "decode error: {e:?}"),
            Self::Io(s) => write!(f, "store io error: {s}"),
        }
    }
}

impl std::error::Error for StorageError {}

/// Deserialize a trie node from its canonical durable byte format.
pub fn decode_node<K: CanonDecode + Canonical + Ord, V: CanonDecode + Canonical>(
    bytes: &[u8],
) -> Result<Node<K, V>, CanonError> {
    let mut r = CanonReader::new(bytes);
    let tag = r.read_uint()?;
    match tag {
        1 => {
            let hash_bytes = r.read_bytes()?;
            if hash_bytes.len() != 32 {
                return Err(CanonError::BadLength);
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(hash_bytes);
            let key_bytes = r.read_bytes()?;
            let key = K::from_canon_bytes(key_bytes)?;
            let val_bytes = r.read_bytes()?;
            let value = V::from_canon_bytes(val_bytes)?;
            if !r.is_empty() {
                return Err(CanonError::BadLength);
            }
            Ok(Node::Leaf(LeafNode::new(key, Arc::new(value), hash)))
        }
        2 => {
            let hash_bytes = r.read_bytes()?;
            if hash_bytes.len() != 32 {
                return Err(CanonError::BadLength);
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(hash_bytes);
            let count = r.read_uint()?;
            if count > 65536 {
                return Err(CanonError::BadLength);
            }
            let mut entries = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let k_bytes = r.read_bytes()?;
                let k = K::from_canon_bytes(k_bytes)?;
                let v_bytes = r.read_bytes()?;
                let v = V::from_canon_bytes(v_bytes)?;
                entries.push((k, Arc::new(v)));
            }
            if !r.is_empty() {
                return Err(CanonError::BadLength);
            }
            Ok(Node::Collision(CollisionNode::new(hash, entries)))
        }
        3 => {
            let prefix_len = r.read_uint()?;
            if prefix_len > 64 {
                return Err(CanonError::BadLength);
            }
            let mut prefix = Vec::with_capacity(prefix_len as usize);
            for _ in 0..prefix_len {
                let nib = r.read_uint()?;
                if nib > 15 {
                    return Err(CanonError::BadLength);
                }
                prefix.push(nib as u8);
            }
            let bitmap = r.read_uint()? as u16;
            let child_count = r.read_uint()?;
            if child_count > 16 || child_count as u32 != bitmap.count_ones() {
                return Err(CanonError::BadLength);
            }
            let mut children = Vec::with_capacity(child_count as usize);
            for _ in 0..child_count {
                let d_bytes = r.read_bytes()?;
                if d_bytes.len() != 32 {
                    return Err(CanonError::BadLength);
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(d_bytes);
                children.push(Arc::new(Node::Lazy(Digest::from_bytes(arr))));
            }
            if !r.is_empty() {
                return Err(CanonError::BadLength);
            }
            Ok(Node::Branch(BranchNode::new(prefix, bitmap, children)))
        }
        4 => {
            let d_bytes = r.read_bytes()?;
            if d_bytes.len() != 32 {
                return Err(CanonError::BadLength);
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(d_bytes);
            if !r.is_empty() {
                return Err(CanonError::BadLength);
            }
            Ok(Node::Lazy(Digest::from_bytes(arr)))
        }
        _ => Err(CanonError::BadLength),
    }
}

/// Deserialize and verify that the decoded node matches the expected cryptographic Merkle digest.
pub fn decode_node_verified<K: CanonDecode + Canonical + Ord, V: CanonDecode + Canonical>(
    bytes: &[u8],
    expected_digest: &Digest,
) -> Result<Node<K, V>, CanonError> {
    let node = decode_node::<K, V>(bytes)?;
    if node.digest() != *expected_digest {
        return Err(CanonError::BadLength);
    }
    Ok(node)
}

/// A persistent on-disk content-addressed node store.
///
/// Nodes are stored immutably under `<root_dir>/objects/{xx}/{yy...}.bin`.
/// Writes are atomic (write to temp file, fsync, then atomic rename).
#[derive(Clone, Debug)]
pub struct FileNodeStore {
    objects_dir: PathBuf,
    write_failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pending_files: Arc<Mutex<BTreeSet<PathBuf>>>,
    io: Arc<StoreIoCounters>,
}

/// Actual filesystem work, shared by cloned handles to one store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreIoStats {
    pub reads: u64,
    pub bytes_read: u64,
    pub writes: u64,
    pub bytes_written: u64,
    pub files_synced: u64,
    pub directories_synced: u64,
}

#[derive(Debug, Default)]
struct StoreIoCounters {
    reads: AtomicU64,
    bytes_read: AtomicU64,
    writes: AtomicU64,
    bytes_written: AtomicU64,
    files_synced: AtomicU64,
    directories_synced: AtomicU64,
}

impl FileNodeStore {
    pub fn new(root_dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let objects_dir = root_dir.as_ref().join("objects");
        fs::create_dir_all(&objects_dir)?;
        Ok(Self {
            objects_dir,
            write_failed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_files: Arc::new(Mutex::new(BTreeSet::new())),
            io: Arc::new(StoreIoCounters::default()),
        })
    }

    pub fn io_stats(&self) -> StoreIoStats {
        StoreIoStats {
            reads: self.io.reads.load(Ordering::Relaxed),
            bytes_read: self.io.bytes_read.load(Ordering::Relaxed),
            writes: self.io.writes.load(Ordering::Relaxed),
            bytes_written: self.io.bytes_written.load(Ordering::Relaxed),
            files_synced: self.io.files_synced.load(Ordering::Relaxed),
            directories_synced: self.io.directories_synced.load(Ordering::Relaxed),
        }
    }

    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    fn object_path(&self, digest: &Digest) -> (PathBuf, PathBuf) {
        let hex = digest.to_hex();
        let prefix = &hex[..2];
        let rest = &hex[2..];
        let dir = self.objects_dir.join(prefix);
        let path = dir.join(format!("{rest}.bin"));
        (dir, path)
    }

    pub fn node_count(&self) -> usize {
        let mut count = 0;
        if let Ok(entries) = fs::read_dir(&self.objects_dir) {
            for entry in entries.flatten() {
                if let Ok(sub) = fs::read_dir(entry.path()) {
                    for f in sub.flatten() {
                        if f.path().extension().and_then(|s| s.to_str()) == Some("bin") {
                            count += 1;
                        }
                    }
                }
            }
        }
        count
    }

    pub fn total_bytes(&self) -> u64 {
        let mut total = 0;
        if let Ok(entries) = fs::read_dir(&self.objects_dir) {
            for entry in entries.flatten() {
                if let Ok(sub) = fs::read_dir(entry.path()) {
                    for f in sub.flatten() {
                        if let Ok(meta) = f.metadata() {
                            total += meta.len();
                        }
                    }
                }
            }
        }
        total
    }

    pub fn flush(&self) -> std::io::Result<()> {
        if self.write_failed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "node write failed in FileNodeStore",
            ));
        }
        let mut pending = self
            .pending_files
            .lock()
            .map_err(|_| std::io::Error::other("poisoned object write set"))?;
        if pending.is_empty() {
            return Ok(());
        }
        let mut directories = BTreeSet::new();
        // Object contents must reach disk before the directory entries and HEAD.
        // Keep pending entries on failure so a later flush cannot mask it.
        for path in pending.iter() {
            fs::File::open(path)?.sync_all()?;
            self.io.files_synced.fetch_add(1, Ordering::Relaxed);
            if let Some(parent) = path.parent() {
                directories.insert(parent.to_path_buf());
            }
        }
        for directory in directories {
            fs::File::open(directory)?.sync_all()?;
            self.io.directories_synced.fetch_add(1, Ordering::Relaxed);
        }
        fs::File::open(&self.objects_dir)?.sync_all()?;
        self.io.directories_synced.fetch_add(1, Ordering::Relaxed);
        pending.clear();
        Ok(())
    }
}

impl NodeStore for FileNodeStore {
    fn get_node(&self, digest: &Digest) -> Option<Vec<u8>> {
        let (_dir, path) = self.object_path(digest);
        self.io.reads.fetch_add(1, Ordering::Relaxed);
        let bytes = fs::read(path).ok()?;
        self.io
            .bytes_read
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Some(bytes)
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        let (dir, path) = self.object_path(&digest);
        if path.exists() {
            return;
        }
        if fs::create_dir_all(&dir).is_err() {
            self.write_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        let tmp_path = dir.join(format!(".tmp-{}.tmp", digest.to_hex()));
        let write_res = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut f = fs::File::create(&tmp_path)?;
            f.write_all(&bytes)?;
            drop(f);
            fs::rename(&tmp_path, &path)?;
            self.pending_files
                .lock()
                .map_err(|_| std::io::Error::other("poisoned object write set"))?
                .insert(path.clone());
            self.io.writes.fetch_add(1, Ordering::Relaxed);
            self.io
                .bytes_written
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            Ok(())
        })();
        if write_res.is_err() {
            let _ = fs::remove_file(&tmp_path);
            self.write_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

fn resolve_node<K: CanonDecode + Canonical + Ord, V: CanonDecode + Canonical, S: NodeStore>(
    node: &Arc<Node<K, V>>,
    store: &S,
) -> Result<Arc<Node<K, V>>, StorageError> {
    match &**node {
        Node::Lazy(d) => {
            let bytes = store
                .get_node(d)
                .ok_or_else(|| StorageError::MissingNode(*d))?;
            let decoded = decode_node_verified::<K, V>(&bytes, d)
                .map_err(|_| StorageError::CorruptedNode(*d))?;
            Ok(Arc::new(decoded))
        }
        _ => Ok(node.clone()),
    }
}

fn insert_recursive_store<
    K: Canonical + Ord + Clone + CanonDecode,
    V: Canonical + Clone + CanonDecode,
    S: NodeStore,
>(
    node: &Arc<Node<K, V>>,
    key: K,
    value: Arc<V>,
    hash: [u8; 32],
    depth: usize,
    stats: &mut TrieOpStats,
    store: &S,
) -> Result<(Arc<Node<K, V>>, bool), StorageError> {
    let resolved = resolve_node(node, store)?;
    stats.nodes_visited += 1;
    match &*resolved {
        Node::Leaf(leaf) => {
            if leaf.hash == hash {
                stats.key_comparisons += 1;
                if leaf.key == key {
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    Ok((Arc::new(Node::Leaf(LeafNode::new(key, value, hash))), false))
                } else {
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let entries = vec![(leaf.key.clone(), leaf.value.clone()), (key, value)];
                    Ok((
                        Arc::new(Node::Collision(CollisionNode::new(hash, entries))),
                        true,
                    ))
                }
            } else {
                let lcp = common_prefix_len(&leaf.hash, &hash, depth);
                let branch_point = depth + lcp;
                debug_assert!(branch_point < 64);
                let nibble_existing = get_nibble(&leaf.hash, branch_point);
                let nibble_new = get_nibble(&hash, branch_point);
                debug_assert_ne!(nibble_existing, nibble_new);

                stats.nodes_allocated += 2;
                stats.nodes_hashed += 2;
                let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));
                let prefix: Vec<u8> = (depth..branch_point)
                    .map(|i| get_nibble(&hash, i))
                    .collect();
                let bitmap = (1u16 << nibble_existing) | (1u16 << nibble_new);
                let children = if nibble_existing < nibble_new {
                    vec![resolved.clone(), new_leaf]
                } else {
                    vec![new_leaf, resolved.clone()]
                };
                Ok((
                    Arc::new(Node::Branch(BranchNode::new(prefix, bitmap, children))),
                    true,
                ))
            }
        }
        Node::Collision(coll) => {
            if coll.hash == hash {
                let mut entries = coll.entries.clone();
                let mut found = false;
                for entry in &mut entries {
                    stats.key_comparisons += 1;
                    if entry.0 == key {
                        entry.1 = value.clone();
                        found = true;
                        break;
                    }
                }
                if !found {
                    entries.push((key, value));
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                }
                stats.nodes_allocated += 1;
                stats.nodes_hashed += 1;
                Ok((
                    Arc::new(Node::Collision(CollisionNode::new(hash, entries))),
                    !found,
                ))
            } else {
                let lcp = common_prefix_len(&coll.hash, &hash, depth);
                let branch_point = depth + lcp;
                debug_assert!(branch_point < 64);
                let nibble_existing = get_nibble(&coll.hash, branch_point);
                let nibble_new = get_nibble(&hash, branch_point);
                debug_assert_ne!(nibble_existing, nibble_new);

                stats.nodes_allocated += 2;
                stats.nodes_hashed += 2;
                let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));
                let prefix: Vec<u8> = (depth..branch_point)
                    .map(|i| get_nibble(&hash, i))
                    .collect();
                let bitmap = (1u16 << nibble_existing) | (1u16 << nibble_new);
                let children = if nibble_existing < nibble_new {
                    vec![resolved.clone(), new_leaf]
                } else {
                    vec![new_leaf, resolved.clone()]
                };
                Ok((
                    Arc::new(Node::Branch(BranchNode::new(prefix, bitmap, children))),
                    true,
                ))
            }
        }
        Node::Branch(branch) => {
            let mut match_len = 0;
            while match_len < branch.prefix.len() {
                if get_nibble(&hash, depth + match_len) != branch.prefix[match_len] {
                    break;
                }
                match_len += 1;
            }

            if match_len < branch.prefix.len() {
                let branch_nibble = branch.prefix[match_len];
                let new_nibble = get_nibble(&hash, depth + match_len);

                let lower_prefix = branch.prefix[match_len + 1..].to_vec();
                stats.nodes_allocated += 3;
                stats.nodes_hashed += 3;
                let lower_branch = Arc::new(Node::Branch(BranchNode::new(
                    lower_prefix,
                    branch.bitmap,
                    branch.children.clone(),
                )));
                let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));

                let upper_prefix = branch.prefix[..match_len].to_vec();
                let upper_bitmap = (1u16 << branch_nibble) | (1u16 << new_nibble);
                let upper_children = if branch_nibble < new_nibble {
                    vec![lower_branch, new_leaf]
                } else {
                    vec![new_leaf, lower_branch]
                };
                Ok((
                    Arc::new(Node::Branch(BranchNode::new(
                        upper_prefix,
                        upper_bitmap,
                        upper_children,
                    ))),
                    true,
                ))
            } else {
                let branch_point = depth + branch.prefix.len();
                debug_assert!(branch_point < 64);
                let nibble = get_nibble(&hash, branch_point);

                match branch.child_index(nibble) {
                    None => {
                        let new_leaf = Arc::new(Node::Leaf(LeafNode::new(key, value, hash)));
                        stats.nodes_allocated += 2;
                        stats.nodes_hashed += 2;
                        let new_bitmap = branch.bitmap | (1u16 << nibble);
                        let insert_pos =
                            (branch.bitmap & ((1u16 << nibble) - 1)).count_ones() as usize;
                        let mut new_children = branch.children.clone();
                        new_children.insert(insert_pos, new_leaf);
                        Ok((
                            Arc::new(Node::Branch(BranchNode::new(
                                branch.prefix.clone(),
                                new_bitmap,
                                new_children,
                            ))),
                            true,
                        ))
                    }
                    Some(idx) => {
                        let (new_child, added) = insert_recursive_store(
                            &branch.children[idx],
                            key,
                            value,
                            hash,
                            branch_point + 1,
                            stats,
                            store,
                        )?;
                        stats.nodes_allocated += 1;
                        stats.nodes_hashed += 1;
                        let mut new_children = branch.children.clone();
                        new_children[idx] = new_child;
                        Ok((
                            Arc::new(Node::Branch(BranchNode::new(
                                branch.prefix.clone(),
                                branch.bitmap,
                                new_children,
                            ))),
                            added,
                        ))
                    }
                }
            }
        }
        Node::Lazy(_) => unreachable!("already resolved"),
    }
}

fn remove_recursive_store<
    K: Canonical + Ord + Clone + CanonDecode,
    V: Canonical + Clone + CanonDecode,
    S: NodeStore,
>(
    node: &Arc<Node<K, V>>,
    key: &K,
    hash: &[u8; 32],
    depth: usize,
    stats: &mut TrieOpStats,
    store: &S,
) -> Result<(Option<Arc<Node<K, V>>>, bool), StorageError> {
    let resolved = resolve_node(node, store)?;
    stats.nodes_visited += 1;
    match &*resolved {
        Node::Leaf(leaf) => {
            if leaf.hash == *hash {
                stats.key_comparisons += 1;
                if leaf.key == *key {
                    return Ok((None, true));
                }
            }
            Ok((Some(resolved), false))
        }
        Node::Collision(coll) => {
            if coll.hash == *hash {
                let mut new_entries = Vec::with_capacity(coll.entries.len());
                let mut removed = false;
                for i in 0..entries_len(&coll.entries) {
                    stats.key_comparisons += 1;
                    if coll.entries[i].0 == *key {
                        for (idx, entry) in coll.entries.iter().enumerate() {
                            if idx != i {
                                new_entries.push(entry.clone());
                            }
                        }
                        removed = true;
                        break;
                    }
                }
                if !removed {
                    return Ok((Some(resolved), false));
                }
                if new_entries.is_empty() {
                    return Ok((None, true));
                } else if new_entries.len() == 1 {
                    let (k, v) = new_entries.pop().unwrap();
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let leaf = LeafNode::new(k, v, coll.hash);
                    return Ok((Some(Arc::new(Node::Leaf(leaf))), true));
                } else {
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let coll = CollisionNode::new(coll.hash, new_entries);
                    return Ok((Some(Arc::new(Node::Collision(coll))), true));
                }
            }
            Ok((Some(resolved), false))
        }
        Node::Branch(branch) => {
            for (i, &nib) in branch.prefix.iter().enumerate() {
                if get_nibble(hash, depth + i) != nib {
                    return Ok((Some(resolved), false));
                }
            }
            let branch_point = depth + branch.prefix.len();
            if branch_point >= 64 {
                return Ok((Some(resolved), false));
            }
            let nibble = get_nibble(hash, branch_point);
            let idx = match branch.child_index(nibble) {
                Some(idx) => idx,
                None => return Ok((Some(resolved), false)),
            };

            let (opt_child, removed) = remove_recursive_store(
                &branch.children[idx],
                key,
                hash,
                branch_point + 1,
                stats,
                store,
            )?;

            if !removed {
                return Ok((Some(resolved), false));
            }

            match opt_child {
                Some(new_child) => {
                    stats.nodes_allocated += 1;
                    stats.nodes_hashed += 1;
                    let mut new_children = branch.children.clone();
                    new_children[idx] = new_child;
                    Ok((
                        Some(Arc::new(Node::Branch(BranchNode::new(
                            branch.prefix.clone(),
                            branch.bitmap,
                            new_children,
                        )))),
                        true,
                    ))
                }
                None => {
                    let new_bitmap = branch.bitmap & !(1u16 << nibble);
                    let mut new_children = branch.children.clone();
                    new_children.remove(idx);

                    if new_children.len() >= 2 {
                        stats.nodes_allocated += 1;
                        stats.nodes_hashed += 1;
                        Ok((
                            Some(Arc::new(Node::Branch(BranchNode::new(
                                branch.prefix.clone(),
                                new_bitmap,
                                new_children,
                            )))),
                            true,
                        ))
                    } else {
                        let single_child = new_children.pop().unwrap();
                        let remaining_nibble = new_bitmap.trailing_zeros() as u8;

                        let resolved_single = resolve_node(&single_child, store)?;
                        match &*resolved_single {
                            Node::Leaf(_) | Node::Collision(_) | Node::Lazy(_) => {
                                Ok((Some(resolved_single), true))
                            }
                            Node::Branch(child_branch) => {
                                let mut merged_prefix = branch.prefix.clone();
                                merged_prefix.push(remaining_nibble);
                                merged_prefix.extend_from_slice(&child_branch.prefix);
                                stats.nodes_allocated += 1;
                                stats.nodes_hashed += 1;
                                let merged = BranchNode::new(
                                    merged_prefix,
                                    child_branch.bitmap,
                                    child_branch.children.clone(),
                                );
                                Ok((Some(Arc::new(Node::Branch(merged))), true))
                            }
                        }
                    }
                }
            }
        }
        Node::Lazy(_) => unreachable!("already resolved"),
    }
}

fn entries_len<T>(v: &[T]) -> usize {
    v.len()
}

fn iter_page_recursive<
    K: Canonical + Ord + Clone + CanonDecode,
    V: Canonical + Clone + CanonDecode,
    S: NodeStore,
>(
    node: &Arc<Node<K, V>>,
    cursor: Option<(&[u8; 32], &K)>,
    limit: usize,
    out: &mut Vec<(K, V)>,
    done: &mut bool,
    store: &S,
    depth: usize,
) -> Result<(), StorageError> {
    if *done {
        return Ok(());
    }
    let resolved = resolve_node(node, store)?;
    match &*resolved {
        Node::Leaf(leaf) => {
            if let Some((cur_hash, cur_key)) = cursor {
                if leaf.hash < *cur_hash || (leaf.hash == *cur_hash && leaf.key <= *cur_key) {
                    return Ok(());
                }
            }
            out.push((leaf.key.clone(), (*leaf.value).clone()));
            if out.len() >= limit {
                *done = true;
            }
            Ok(())
        }
        Node::Collision(coll) => {
            for (k, v) in &coll.entries {
                if let Some((cur_hash, cur_key)) = cursor {
                    if coll.hash < *cur_hash || (coll.hash == *cur_hash && k <= cur_key) {
                        continue;
                    }
                }
                out.push((k.clone(), (**v).clone()));
                if out.len() >= limit {
                    *done = true;
                    return Ok(());
                }
            }
            Ok(())
        }
        Node::Branch(branch) => {
            let mut child_cursor = cursor;
            if let Some((hash, _)) = cursor {
                for (offset, nibble) in branch.prefix.iter().enumerate() {
                    match nibble.cmp(&get_nibble(hash, depth + offset)) {
                        std::cmp::Ordering::Less => return Ok(()),
                        std::cmp::Ordering::Greater => {
                            child_cursor = None;
                            break;
                        }
                        std::cmp::Ordering::Equal => {}
                    }
                }
            }
            let branch_depth = depth + branch.prefix.len();
            let mut children = branch.children.iter();
            for nibble in 0..16u8 {
                if branch.bitmap & (1u16 << nibble) == 0 {
                    continue;
                }
                let child = children.next().expect("validated branch bitmap");
                let next_cursor = match child_cursor {
                    Some((hash, _)) if nibble < get_nibble(hash, branch_depth) => continue,
                    Some((hash, _)) if nibble > get_nibble(hash, branch_depth) => None,
                    other => other,
                };
                iter_page_recursive(
                    child,
                    next_cursor,
                    limit,
                    out,
                    done,
                    store,
                    branch_depth + 1,
                )?;
                if *done {
                    return Ok(());
                }
            }
            Ok(())
        }
        Node::Lazy(_) => Ok(()),
    }
}

fn persist_node_recursive<K: Canonical, V: Canonical, S: NodeStore>(
    node: &Arc<Node<K, V>>,
    store: &mut S,
    count: &mut usize,
) {
    if let Node::Lazy(_) = &**node {
        return;
    }
    let digest = node.digest();
    if store.contains(&digest) {
        return;
    }
    if let Node::Branch(branch) = &**node {
        for child in &branch.children {
            persist_node_recursive(child, store, count);
        }
    }
    let bytes = encode_node(node);
    store.put_node(digest, bytes);
    *count += 1;
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_map_insert_returns_new_snapshot_old_unchanged() {
        let m0: ArcMap<u32, &str> = ArcMap::new();
        let m0 = m0.insert(1, "a");
        let m1 = m0.insert(2, "b");

        assert_eq!(m0.get(&1), Some(&"a"));
        assert_eq!(
            m0.get(&2),
            None,
            "old snapshot must not observe an update made on a derived map"
        );
        assert_eq!(m1.get(&1), Some(&"a"));
        assert_eq!(m1.get(&2), Some(&"b"));
        assert_eq!(m0.len(), 1);
        assert_eq!(m1.len(), 2);
    }

    #[test]
    fn arc_map_remove_returns_new_snapshot_old_unchanged() {
        let m0: ArcMap<u32, &str> = ArcMap::new().insert(1, "a").insert(2, "b");
        let m1 = m0.remove(&1);

        assert_eq!(m0.get(&1), Some(&"a"));
        assert_eq!(m0.len(), 2);
        assert_eq!(m1.get(&1), None);
        assert_eq!(m1.get(&2), Some(&"b"));
        assert_eq!(m1.len(), 1);
    }

    #[test]
    fn trie_map_insert_and_get() {
        let m: TrieMap<u64, &str> = TrieMap::new();
        let m = m.insert(10, "alpha").insert(20, "beta").insert(30, "gamma");

        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&10), Some(&"alpha"));
        assert_eq!(m.get(&20), Some(&"beta"));
        assert_eq!(m.get(&30), Some(&"gamma"));
        assert_eq!(m.get(&40), None);
    }

    #[test]
    fn trie_map_persistence_old_snapshot_unchanged() {
        let m0: TrieMap<u64, u64> = TrieMap::new().insert(1, 100);
        let m1 = m0.insert(2, 200);

        assert_eq!(m0.len(), 1);
        assert_eq!(m0.get(&1), Some(&100));
        assert_eq!(m0.get(&2), None);

        assert_eq!(m1.len(), 2);
        assert_eq!(m1.get(&1), Some(&100));
        assert_eq!(m1.get(&2), Some(&200));
    }

    #[test]
    fn trie_map_overwrite_preserves_size() {
        let m0: TrieMap<u64, &str> = TrieMap::new().insert(1, "old");
        let m1 = m0.insert(1, "new");

        assert_eq!(m0.get(&1), Some(&"old"));
        assert_eq!(m1.get(&1), Some(&"new"));
        assert_eq!(m1.len(), 1);
    }

    #[test]
    fn trie_map_remove_and_contraction() {
        let m0: TrieMap<u64, &str> = TrieMap::new().insert(1, "a").insert(2, "b").insert(3, "c");
        assert_eq!(m0.len(), 3);

        let m1 = m0.remove(&2);
        assert_eq!(m1.len(), 2);
        assert_eq!(m1.get(&1), Some(&"a"));
        assert_eq!(m1.get(&2), None);
        assert_eq!(m1.get(&3), Some(&"c"));

        // Removing non-existent key is a no-op
        let m2 = m1.remove(&999);
        assert_eq!(m2.len(), 2);
        assert_eq!(m2.root_digest(), m1.root_digest());
    }

    #[test]
    fn trie_map_canonical_root_invariant_across_insertion_histories() {
        // Map 1: insert 1, 2, 3, 4
        let m1: TrieMap<u64, u64> = TrieMap::new()
            .insert(1, 10)
            .insert(2, 20)
            .insert(3, 30)
            .insert(4, 40);

        // Map 2: insert 4, 3, 2, 1 (reverse order)
        let m2: TrieMap<u64, u64> = TrieMap::new()
            .insert(4, 40)
            .insert(3, 30)
            .insert(2, 20)
            .insert(1, 10);

        // Map 3: insert 2, 4, 1, 3 (interleaved)
        let m3: TrieMap<u64, u64> = TrieMap::new()
            .insert(2, 20)
            .insert(4, 40)
            .insert(1, 10)
            .insert(3, 30);

        // Map 4: insert 1, 2, 3, 4, 999, then remove 999
        let m4: TrieMap<u64, u64> = TrieMap::new()
            .insert(1, 10)
            .insert(2, 20)
            .insert(3, 30)
            .insert(4, 40)
            .insert(999, 9990)
            .remove(&999);

        assert_eq!(
            m1.root_digest(),
            m2.root_digest(),
            "reverse order produces identical root"
        );
        assert_eq!(
            m1.root_digest(),
            m3.root_digest(),
            "interleaved order produces identical root"
        );
        assert_eq!(
            m1.root_digest(),
            m4.root_digest(),
            "insert then remove contracts to identical root"
        );
        assert_eq!(m1, m2);
        assert_eq!(m1, m3);
        assert_eq!(m1, m4);
    }

    #[test]
    fn trie_map_collision_buckets_with_injected_hasher() {
        // ModuloHasher with modulus 1 maps every single key to the EXACT same hash [0; 32].
        // This forces every entry into a collision bucket, testing full key comparisons and bucket sorting.
        let hasher = ModuloHasher::new(1);
        let m: TrieMap<u64, &str, ModuloHasher> = TrieMap::with_hasher(hasher)
            .insert(30, "thirty")
            .insert(10, "ten")
            .insert(20, "twenty");

        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&10), Some(&"ten"));
        assert_eq!(m.get(&20), Some(&"twenty"));
        assert_eq!(m.get(&30), Some(&"thirty"));
        assert_eq!(m.get(&40), None);

        // Overwrite inside collision bucket
        let m = m.insert(20, "TWENTY");
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&20), Some(&"TWENTY"));

        // Remove from collision bucket down to 1 entry (which contracts to Leaf)
        let m = m.remove(&10).remove(&30);
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&20), Some(&"TWENTY"));

        // Remove the final entry down to empty
        let m = m.remove(&20);
        assert!(m.is_empty());
        assert_eq!(m.root_digest(), TrieMap::<u64, &str>::new().root_digest());
    }

    #[test]
    fn trie_map_durable_node_persistence() {
        let mut store = MemoryNodeStore::new();
        let m: TrieMap<u64, u64> = TrieMap::new().insert(1, 100).insert(2, 200).insert(3, 300);

        let written1 = m.persist_to_store(&mut store);
        assert!(written1 > 0);
        assert_eq!(store.len(), written1);

        // Re-persisting the same tree writes 0 additional nodes (content-addressed sharing)
        let written2 = m.persist_to_store(&mut store);
        assert_eq!(written2, 0);

        // Derived map with 1 new key writes only the newly allocated path nodes
        let m_derived = m.insert(4, 400);
        let written3 = m_derived.persist_to_store(&mut store);
        assert!(
            (1..=4).contains(&written3),
            "derived insert writes only changed path nodes: {written3}"
        );
    }

    #[test]
    fn trie_map_decode_and_lazy_hydration() {
        let mut store = MemoryNodeStore::new();
        let m: TrieMap<u64, u64> = TrieMap::new()
            .insert(1, 100)
            .insert(2, 200)
            .insert(3, 300)
            .insert(4, 400);

        let root_digest = m.root_digest();
        let written = m.persist_to_store(&mut store);
        assert!(written > 0);

        // Cold reconstruct from root digest only
        let cold: TrieMap<u64, u64> = TrieMap::from_root_digest(root_digest, m.len());
        assert_eq!(cold.len(), 4);
        assert_eq!(cold.root_digest(), root_digest);

        // Verify lazy reads
        assert_eq!(cold.get_with_store(&1, &store).unwrap(), Some(100));
        assert_eq!(cold.get_with_store(&2, &store).unwrap(), Some(200));
        assert_eq!(cold.get_with_store(&3, &store).unwrap(), Some(300));
        assert_eq!(cold.get_with_store(&4, &store).unwrap(), Some(400));
        assert_eq!(cold.get_with_store(&99, &store).unwrap(), None);

        // Verify lazy insert
        let (cold_updated, _) = cold.insert_with_store(5, 500, &store).unwrap();
        assert_eq!(cold_updated.len(), 5);
        assert_eq!(cold_updated.get_with_store(&5, &store).unwrap(), Some(500));
        assert_eq!(cold_updated.get_with_store(&1, &store).unwrap(), Some(100));

        // Verify lazy remove
        let (cold_removed, _) = cold.remove_with_store(&2, &store).unwrap();
        assert_eq!(cold_removed.len(), 3);
        assert_eq!(cold_removed.get_with_store(&2, &store).unwrap(), None);
        assert_eq!(cold_removed.get_with_store(&1, &store).unwrap(), Some(100));

        // Verify paged iteration
        let (page1, cursor1) = cold.iter_page_with_store(None, 2, &store).unwrap();
        assert_eq!(page1.len(), 2);
        assert!(cursor1.is_some());

        let cur1_ref = cursor1.as_ref().map(|(h, k)| (h, k));
        let (page2, cursor2) = cold.iter_page_with_store(cur1_ref, 2, &store).unwrap();
        assert_eq!(page2.len(), 2);
        assert!(
            cursor2.is_none(),
            "a full terminal page must not advertise another page"
        );
        let mut all = page1;
        all.extend(page2);
        all.sort();
        assert_eq!(all, vec![(1, 100), (2, 200), (3, 300), (4, 400)]);
    }

    #[test]
    fn file_node_store_roundtrip_and_deduplication() {
        let temp_dir = std::env::temp_dir().join(format!("soc_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);

        let mut store = FileNodeStore::new(&temp_dir).expect("create FileNodeStore");
        let m: TrieMap<u64, String> = TrieMap::new()
            .insert(10, "alpha".to_string())
            .insert(20, "beta".to_string())
            .insert(30, "gamma".to_string());

        let root_digest = m.root_digest();
        let written = m.persist_to_store(&mut store);
        assert!(written > 0);
        assert_eq!(store.node_count(), written);
        assert!(store.total_bytes() > 0);

        // Deduplication: persisting again writes 0 new files
        let written_again = m.persist_to_store(&mut store);
        assert_eq!(written_again, 0);

        // Re-read cold from disk
        let cold: TrieMap<u64, String> = TrieMap::from_root_digest(root_digest, m.len());
        assert_eq!(
            cold.get_with_store(&10, &store).unwrap(),
            Some("alpha".to_string())
        );
        assert_eq!(
            cold.get_with_store(&20, &store).unwrap(),
            Some("beta".to_string())
        );
        assert_eq!(
            cold.get_with_store(&30, &store).unwrap(),
            Some("gamma".to_string())
        );
        assert_eq!(cold.get_with_store(&40, &store).unwrap(), None);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
