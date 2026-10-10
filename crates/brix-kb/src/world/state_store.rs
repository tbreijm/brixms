//! Store-aware persistent collections for network state (R0 Phase 2, M0).
//!
//! [`StateStore`] is a cheaply clonable handle over a file- or memory-backed
//! [`NodeStore`] with a bounded LRU read cache. [`SMap`] and [`SSet`] wrap soc-core's
//! `TrieMap` and expose **only** fallible, store-aware operations: a missing or corrupt
//! node is always a typed [`StateError`], never silently treated as empty (the plain
//! `TrieMap::get`/`iter`/`remove` treat unhydrated `Lazy` nodes as absent, and `insert`
//! panics on them; none of those are reachable through this API).

#![deny(unsafe_code)]

use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, RwLock};

use brix_canon::{Digest, Domain};
use soc_core::store::{
    DecodedNodeCache, FileNodeStore, MemoryNodeStore, NodeStore, StorageError, TrieMap,
};

use super::policy_trace::{PolicyTrace, Recorder, TraceOp, TraceScope};
pub use super::state_codec::SubRoot;
use super::state_codec::{St, StateCodec, StateCodecError};

/// Error from a store-aware state operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// A trie node was missing or failed verification / decoding.
    Storage(StorageError),
    /// A persisted value failed its canonical codec checks.
    Codec(StateCodecError),
    /// I/O failure (flush, open).
    Io(String),
    /// A root passed to `dehydrate` is not present in the store.
    RootNotPersisted(Digest),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(e) => write!(f, "state store: {e}"),
            Self::Codec(e) => write!(f, "state store: {e}"),
            Self::Io(e) => write!(f, "state store io: {e}"),
            Self::RootNotPersisted(d) => {
                write!(f, "state store: root {} not persisted", d.to_hex())
            }
        }
    }
}

impl std::error::Error for StateError {}

impl From<StorageError> for StateError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<StateCodecError> for StateError {
    fn from(e: StateCodecError) -> Self {
        Self::Codec(e)
    }
}

/// Default decoded-node cache budget for a [`StateStore`] (64 MiB).
pub const DEFAULT_NODE_CACHE_BYTES: usize = 128 * 1024 * 1024;

enum Backend {
    Memory(MemoryNodeStore),
    File(FileNodeStore),
}

struct Inner {
    backend: RwLock<Backend>,
    cache: DecodedNodeCache,
    /// Opt-in node-touch trace (R0 M7); off unless a sink was installed.
    recorder: Arc<Recorder>,
}

/// Shared handle to the persisted node store plus its read cache.
#[derive(Clone)]
pub struct StateStore {
    inner: Arc<Inner>,
}

impl fmt::Debug for StateStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StateStore")
    }
}

impl StateStore {
    fn from_backend(backend: Backend, node_cache_bytes: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend: RwLock::new(backend),
                cache: DecodedNodeCache::new(node_cache_bytes),
                recorder: Arc::default(),
            }),
        }
    }

    /// Fresh in-memory store.
    pub fn memory(node_cache_bytes: usize) -> Self {
        Self::from_backend(Backend::Memory(MemoryNodeStore::new()), node_cache_bytes)
    }

    /// In-memory store seeded from an existing [`MemoryNodeStore`].
    pub fn from_memory(store: MemoryNodeStore, node_cache_bytes: usize) -> Self {
        Self::from_backend(Backend::Memory(store), node_cache_bytes)
    }

    /// Open (or create) a file-backed store rooted at `dir`.
    pub fn open_file(dir: impl AsRef<Path>, node_cache_bytes: usize) -> io::Result<Self> {
        Ok(Self::from_backend(
            Backend::File(FileNodeStore::new(dir)?),
            node_cache_bytes,
        ))
    }

    /// Wrap an existing file store.
    pub fn from_file(store: FileNodeStore, node_cache_bytes: usize) -> Self {
        Self::from_backend(Backend::File(store), node_cache_bytes)
    }

    /// Make pending writes durable (no-op for memory stores).
    pub fn flush(&self) -> Result<(), StateError> {
        let backend = self.inner.backend.read().expect("state store lock");
        if let Backend::File(f) = &*backend {
            f.flush().map_err(|e| StateError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// Whether the store holds a node with this digest.
    pub fn has_node(&self, digest: &Digest) -> bool {
        NodeStore::contains(self, digest)
    }

    /// Physical file-store IO counters (`None` for memory stores).
    pub fn io_stats(&self) -> Option<soc_core::store::StoreIoStats> {
        match &*self.inner.backend.read().expect("state store lock") {
            Backend::File(f) => Some(f.io_stats()),
            Backend::Memory(_) => None,
        }
    }

    /// Bytes currently held by the read cache.
    pub fn cache_used_bytes(&self) -> usize {
        self.inner.cache.used_bytes()
    }

    /// Start recording node touches into a fresh in-memory [`PolicyTrace`] holding at most
    /// `cap` records (see `policy_trace`). Shared by every clone of this store. Recording
    /// never changes a result; with no trace installed (the default) the cost is one relaxed
    /// atomic load per node resolution.
    pub fn enable_policy_trace(&self, cap: usize) -> PolicyTrace {
        let trace = PolicyTrace::new(cap);
        self.inner.recorder.enable(trace.clone());
        trace
    }

    /// Stop recording (the trace handle keeps what it already holds).
    pub fn disable_policy_trace(&self) {
        self.inner.recorder.disable();
    }

    /// Label the node touches made while the returned guard lives (innermost wins).
    pub fn trace_scope(&self, label: &'static str) -> TraceScope {
        TraceScope::new(&self.inner.recorder, label)
    }
}

impl NodeStore for StateStore {
    fn get_node(&self, digest: &Digest) -> Option<Vec<u8>> {
        let backend = self.inner.backend.read().expect("state store lock");
        match &*backend {
            Backend::Memory(m) => m.get_node(digest),
            Backend::File(f) => f.get_node(digest),
        }
    }

    fn decoded_cache(&self) -> Option<&DecodedNodeCache> {
        Some(&self.inner.cache)
    }

    #[inline]
    fn note_touch(&self, digest: &Digest, hit: bool, bytes: usize) {
        if self.inner.recorder.is_on() {
            self.inner
                .recorder
                .record(TraceOp::NodeRead, digest, hit, bytes);
        }
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        if self.inner.recorder.is_on() {
            self.inner
                .recorder
                .record(TraceOp::NodeWrite, &digest, false, bytes.len());
        }
        let mut backend = self.inner.backend.write().expect("state store lock");
        match &mut *backend {
            Backend::Memory(m) => m.put_node(digest, bytes),
            Backend::File(f) => f.put_node(digest, bytes),
        }
    }

    fn put_new_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        if self.inner.recorder.is_on() {
            self.inner
                .recorder
                .record(TraceOp::NodeWrite, &digest, false, bytes.len());
        }
        let mut backend = self.inner.backend.write().expect("state store lock");
        match &mut *backend {
            Backend::Memory(m) => m.put_node(digest, bytes),
            Backend::File(f) => f.put_new_node(digest, bytes),
        }
    }

    fn contains(&self, digest: &Digest) -> bool {
        let backend = self.inner.backend.read().expect("state store lock");
        match &*backend {
            Backend::Memory(m) => m.contains(digest),
            Backend::File(f) => f.contains(digest),
        }
    }
}

fn empty_root() -> Digest {
    Digest::of(Domain::Value, b"brix:trie:empty:v1")
}

/// Resume token for [`SMap::page`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageCursor<K> {
    pub hash: [u8; 32],
    pub key: K,
}

/// A persisted ordered-by-hash map keyed by `K` with values `V`.
pub struct SMap<K, V> {
    trie: TrieMap<St<K>, St<V>>,
    _p: PhantomData<(K, V)>,
}

impl<K: Clone, V: Clone> Clone for SMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            trie: self.trie.clone(),
            _p: PhantomData,
        }
    }
}

impl<
        K: StateCodec + Ord + Clone + 'static + Send + Sync,
        V: StateCodec + Clone + 'static + Send + Sync,
    > Default for SMap<K, V>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
        K: StateCodec + Ord + Clone + 'static + Send + Sync,
        V: StateCodec + Clone + 'static + Send + Sync,
    > SMap<K, V>
{
    pub fn new() -> Self {
        Self {
            trie: TrieMap::new(),
            _p: PhantomData,
        }
    }

    /// Open a map by root digest and entry count; nodes load lazily from the store.
    pub fn from_root(root: Digest, len: u64) -> Self {
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        Self {
            trie: TrieMap::from_root_digest(root, len),
            _p: PhantomData,
        }
    }

    pub fn from_sub_root(sub: SubRoot) -> Self {
        Self::from_root(sub.root, sub.len)
    }

    pub fn get(&self, key: &K, store: &StateStore) -> Result<Option<V>, StateError> {
        Ok(self
            .trie
            .get_with_store(&St(key.clone()), store)?
            .map(|v| v.0))
    }

    pub fn contains(&self, key: &K, store: &StateStore) -> Result<bool, StateError> {
        Ok(self.get(key, store)?.is_some())
    }

    /// Insert or overwrite, returning the new map (the receiver is unchanged).
    pub fn upsert(&self, key: K, value: V, store: &StateStore) -> Result<Self, StateError> {
        let (trie, _) = self.trie.insert_with_store(St(key), St(value), store)?;
        Ok(Self {
            trie,
            _p: PhantomData,
        })
    }

    /// Remove `key` if present, returning the new map.
    pub fn remove(&self, key: &K, store: &StateStore) -> Result<Self, StateError> {
        let (trie, _) = self.trie.remove_with_store(&St(key.clone()), store)?;
        Ok(Self {
            trie,
            _p: PhantomData,
        })
    }

    /// Up to `limit` entries after `cursor` in deterministic trie order, plus the next cursor.
    #[allow(clippy::type_complexity)]
    pub fn page(
        &self,
        cursor: Option<&PageCursor<K>>,
        limit: usize,
        store: &StateStore,
    ) -> Result<(Vec<(K, V)>, Option<PageCursor<K>>), StateError> {
        let wrapped = cursor.map(|c| (c.hash, St(c.key.clone())));
        let (rows, next) =
            self.trie
                .iter_page_with_store(wrapped.as_ref().map(|(h, k)| (h, k)), limit, store)?;
        Ok((
            rows.into_iter().map(|(k, v)| (k.0, v.0)).collect(),
            next.map(|(hash, k)| PageCursor { hash, key: k.0 }),
        ))
    }

    pub fn len(&self) -> usize {
        self.trie.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trie.len() == 0
    }

    pub fn root(&self) -> Digest {
        self.trie.root_digest()
    }

    pub fn sub_root(&self) -> SubRoot {
        SubRoot {
            root: self.root(),
            len: self.len() as u64,
        }
    }

    /// Write all not-yet-persisted nodes; returns the number of nodes written. Call
    /// [`StateStore::flush`] afterwards for durability.
    pub fn persist(&self, store: &StateStore) -> usize {
        let mut s = store.clone();
        self.trie.persist_to_store(&mut s)
    }

    /// Drop all in-memory nodes, keeping only `(root, len)`. The root must already be
    /// persisted (checked), otherwise reads would fail later.
    pub fn dehydrate(&self, store: &StateStore) -> Result<Self, StateError> {
        let root = self.root();
        if root != empty_root() && !store.has_node(&root) {
            return Err(StateError::RootNotPersisted(root));
        }
        Ok(Self::from_root(root, self.len() as u64))
    }
}

impl<K: StateCodec, V: StateCodec> fmt::Debug for SMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SMap")
            .field("root", &self.trie.root_digest().to_hex())
            .field("len", &self.trie.len())
            .finish()
    }
}

impl<K: StateCodec> fmt::Debug for SSet<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SSet")
            .field("root", &self.map.trie.root_digest().to_hex())
            .field("len", &self.map.trie.len())
            .finish()
    }
}

/// A persisted set of `K`.
pub struct SSet<K> {
    map: SMap<K, ()>,
}

impl<K: Clone> Clone for SSet<K> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<K: StateCodec + Ord + Clone + 'static + Send + Sync> Default for SSet<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: StateCodec + Ord + Clone + 'static + Send + Sync> SSet<K> {
    pub fn new() -> Self {
        Self { map: SMap::new() }
    }

    pub fn from_root(root: Digest, len: u64) -> Self {
        Self {
            map: SMap::from_root(root, len),
        }
    }

    pub fn from_sub_root(sub: SubRoot) -> Self {
        Self::from_root(sub.root, sub.len)
    }

    pub fn contains(&self, key: &K, store: &StateStore) -> Result<bool, StateError> {
        self.map.contains(key, store)
    }

    pub fn insert(&self, key: K, store: &StateStore) -> Result<Self, StateError> {
        Ok(Self {
            map: self.map.upsert(key, (), store)?,
        })
    }

    pub fn remove(&self, key: &K, store: &StateStore) -> Result<Self, StateError> {
        Ok(Self {
            map: self.map.remove(key, store)?,
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn page(
        &self,
        cursor: Option<&PageCursor<K>>,
        limit: usize,
        store: &StateStore,
    ) -> Result<(Vec<K>, Option<PageCursor<K>>), StateError> {
        let (rows, next) = self.map.page(cursor, limit, store)?;
        Ok((rows.into_iter().map(|(k, ())| k).collect(), next))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn root(&self) -> Digest {
        self.map.root()
    }

    pub fn sub_root(&self) -> SubRoot {
        self.map.sub_root()
    }

    pub fn persist(&self, store: &StateStore) -> usize {
        self.map.persist(store)
    }

    pub fn dehydrate(&self, store: &StateStore) -> Result<Self, StateError> {
        Ok(Self {
            map: self.map.dehydrate(store)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::network::{DerivationId, IntermediateTuple, Value};
    use super::super::state_codec::ValueList;
    use super::super::types::WorldKey;
    use super::*;
    use proptest::prelude::*;

    fn tup(i: i64) -> IntermediateTuple {
        let mut t = IntermediateTuple::new();
        t.insert("x", Value::Int(i));
        t.insert("s", Value::Str(format!("v{i}")));
        t
    }

    fn did(i: u8) -> Digest {
        super::super::compact_deriv::CompactDeriv::from_derivation(&DerivationId::Base {
            relation: "r".into(),
            key: WorldKey(vec![i]),
        })
        .digest()
    }

    fn build(store: &StateStore, order: &[i64]) -> SMap<ValueList, IntermediateTuple> {
        let mut m = SMap::new();
        for &i in order {
            m = m
                .upsert(ValueList(vec![Value::Int(i)]), tup(i), store)
                .unwrap();
        }
        m
    }

    #[test]
    fn root_independent_of_insertion_order() {
        let store = StateStore::memory(1 << 20);
        let a = build(&store, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let b = build(&store, &[10, 3, 7, 1, 9, 2, 8, 4, 6, 5]);
        assert_eq!(a.root(), b.root());
        assert_eq!(a.len(), 10);
    }

    proptest! {
        #[test]
        fn root_order_independent_prop(mut xs in proptest::collection::vec(any::<i64>(), 0..40), seed in any::<u64>()) {
            xs.sort();
            xs.dedup();
            let store = StateStore::memory(1 << 20);
            let a = build(&store, &xs);
            let mut ys = xs.clone();
            // deterministic shuffle
            let mut s = seed | 1;
            for i in (1..ys.len()).rev() {
                s ^= s << 13; s ^= s >> 7; s ^= s << 17;
                ys.swap(i, (s % (i as u64 + 1)) as usize);
            }
            let b = build(&store, &ys);
            prop_assert_eq!(a.root(), b.root());
            // remove-all returns to the empty root
            let mut c = b;
            for x in &ys { c = c.remove(&ValueList(vec![Value::Int(*x)]), &store).unwrap(); }
            prop_assert!(c.is_empty());
            prop_assert_eq!(c.root(), SMap::<ValueList, IntermediateTuple>::new().root());
        }
    }

    #[test]
    fn int_and_str_keys_do_not_collide() {
        let store = StateStore::memory(1 << 20);
        let m: SMap<ValueList, u64> = SMap::new()
            .upsert(ValueList(vec![Value::Int(1)]), 1, &store)
            .unwrap()
            .upsert(ValueList(vec![Value::Str("1".into())]), 2, &store)
            .unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(
            m.get(&ValueList(vec![Value::Int(1)]), &store).unwrap(),
            Some(1)
        );
        assert_eq!(
            m.get(&ValueList(vec![Value::Str("1".into())]), &store)
                .unwrap(),
            Some(2)
        );
    }

    #[test]
    fn dehydrate_preserves_root_and_reads() {
        let store = StateStore::memory(1 << 20);
        let m = build(&store, &(0..200).collect::<Vec<_>>());
        // Not persisted yet: dehydrate must refuse.
        assert!(matches!(
            m.dehydrate(&store),
            Err(StateError::RootNotPersisted(_))
        ));
        assert!(m.persist(&store) > 0);
        store.flush().unwrap();
        let d = m.dehydrate(&store).unwrap();
        assert_eq!(d.root(), m.root());
        assert_eq!(d.len(), m.len());
        for i in [0i64, 17, 199] {
            assert_eq!(
                d.get(&ValueList(vec![Value::Int(i)]), &store).unwrap(),
                Some(tup(i))
            );
        }
        assert_eq!(
            d.get(&ValueList(vec![Value::Int(500)]), &store).unwrap(),
            None
        );
        // Writes on the dehydrated map still reach the same root as the hydrated one.
        let k = ValueList(vec![Value::Int(1000)]);
        let a = d.upsert(k.clone(), tup(1000), &store).unwrap();
        let b = m.upsert(k, tup(1000), &store).unwrap();
        assert_eq!(a.root(), b.root());
        // Removal through lazy nodes.
        let r = d.remove(&ValueList(vec![Value::Int(5)]), &store).unwrap();
        assert_eq!(r.len(), 199);
        assert_eq!(
            r.get(&ValueList(vec![Value::Int(5)]), &store).unwrap(),
            None
        );
        // Pagination covers everything exactly once.
        let mut seen = 0;
        let mut cur = None;
        loop {
            let (rows, next) = d.page(cur.as_ref(), 33, &store).unwrap();
            seen += rows.len();
            match next {
                Some(c) => cur = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, 200);
    }

    #[test]
    fn missing_node_is_a_typed_error_not_empty() {
        let store = StateStore::memory(1 << 20);
        let m = build(&store, &(0..50).collect::<Vec<_>>());
        m.persist(&store);
        let other = StateStore::memory(1 << 20); // does not hold the nodes
        let d: SMap<ValueList, IntermediateTuple> = SMap::from_root(m.root(), m.len() as u64);
        let k = ValueList(vec![Value::Int(3)]);
        assert!(matches!(
            d.get(&k, &other),
            Err(StateError::Storage(StorageError::MissingNode(_)))
        ));
        assert!(d.upsert(k.clone(), tup(3), &other).is_err());
        assert!(d.remove(&k, &other).is_err());
        assert!(d.page(None, 10, &other).is_err());
    }

    #[test]
    fn flipped_byte_is_a_typed_error() {
        let mut raw = MemoryNodeStore::new();
        let m: SMap<ValueList, IntermediateTuple> = {
            let scratch = StateStore::memory(0);
            let m = build(&scratch, &(0..30).collect::<Vec<_>>());
            let mut s = scratch.clone();
            m.trie.persist_to_store(&mut s);
            // copy nodes out
            let backend = scratch.inner.backend.read().unwrap();
            if let Backend::Memory(mem) = &*backend {
                raw = mem.clone();
            }
            m
        };
        let root = m.root();
        let mut bytes = raw.get_node(&root).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        raw.put_node(root, bytes);
        let store = StateStore::from_memory(raw, 1 << 20);
        let d: SMap<ValueList, IntermediateTuple> = SMap::from_root(root, 30);
        let r = d.get(&ValueList(vec![Value::Int(3)]), &store);
        assert!(
            matches!(
                r,
                Err(StateError::Storage(StorageError::CorruptedNode(_)))
                    | Err(StateError::Storage(StorageError::MissingNode(_)))
            ),
            "got {r:?}"
        );
    }

    #[test]
    fn sset_and_subroot_nesting() {
        let store = StateStore::memory(1 << 20);
        let mut set = SSet::new();
        for i in 0..20u8 {
            set = set.insert(did(i), &store).unwrap();
        }
        set.persist(&store);
        let sub = set.sub_root();
        assert_eq!(sub.len, 20);
        let outer: SMap<ValueList, SubRoot> = SMap::new()
            .upsert(ValueList(vec![Value::Str("g".into())]), sub, &store)
            .unwrap();
        outer.persist(&store);
        let outer = outer.dehydrate(&store).unwrap();
        let got = outer
            .get(&ValueList(vec![Value::Str("g".into())]), &store)
            .unwrap()
            .unwrap();
        let reopened: SSet<Digest> = SSet::from_sub_root(got);
        assert!(reopened.contains(&did(7), &store).unwrap());
        assert!(!reopened.contains(&did(99), &store).unwrap());
        let shrunk = reopened.remove(&did(7), &store).unwrap();
        assert_eq!(shrunk.len(), 19);
        assert!(!shrunk.contains(&did(7), &store).unwrap());
    }

    #[test]
    fn lru_cache_is_bounded() {
        let store = StateStore::memory(2048);
        let m = build(&store, &(0..300).collect::<Vec<_>>());
        m.persist(&store);
        let d = m.dehydrate(&store).unwrap();
        for i in 0..300 {
            d.get(&ValueList(vec![Value::Int(i)]), &store).unwrap();
        }
        assert!(store.cache_used_bytes() <= 2048);
    }

    #[test]
    fn second_get_on_dehydrated_map_does_zero_decodes() {
        let store = StateStore::memory(1 << 22);
        let m = build(&store, &(0..200).collect::<Vec<_>>());
        m.persist(&store);
        let d = m.dehydrate(&store).unwrap();
        let k = ValueList(vec![Value::Int(77)]);
        assert_eq!(d.get(&k, &store).unwrap(), Some(tup(77)));
        let after_first = store.inner.cache.decode_count();
        assert!(after_first > 0);
        assert_eq!(d.get(&k, &store).unwrap(), Some(tup(77)));
        assert_eq!(store.inner.cache.decode_count(), after_first);
    }

    #[test]
    fn corrupt_node_is_never_cached() {
        let mut raw = MemoryNodeStore::new();
        let scratch = StateStore::memory(0);
        let m = build(&scratch, &(0..30).collect::<Vec<_>>());
        let mut s = scratch.clone();
        m.trie.persist_to_store(&mut s);
        if let Backend::Memory(mem) = &*scratch.inner.backend.read().unwrap() {
            raw = mem.clone();
        }
        let root = m.root();
        let mut bytes = raw.get_node(&root).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        raw.put_node(root, bytes);
        let store = StateStore::from_memory(raw, 1 << 20);
        let d: SMap<ValueList, IntermediateTuple> = SMap::from_root(root, 30);
        let k = ValueList(vec![Value::Int(3)]);
        for _ in 0..2 {
            assert!(d.get(&k, &store).is_err());
        }
        assert!(!store.inner.cache.contains_digest(&root));
        assert_eq!(store.cache_used_bytes(), 0);
        assert_eq!(store.inner.cache.decode_count(), 0);
    }

    #[test]
    fn debug_prints_root_and_len() {
        let m: SMap<ValueList, u64> = SMap::new();
        let s = format!("{m:?}");
        assert!(s.contains("root") && s.contains("len: 0"), "{s}");
        let set: SSet<Digest> = SSet::new();
        assert!(format!("{set:?}").contains("len: 0"));
    }

    #[test]
    fn file_backed_roundtrip_across_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "brix-state-store-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let root;
        {
            let store = StateStore::open_file(&dir, 1 << 16).unwrap();
            let m = build(&store, &(0..100).collect::<Vec<_>>());
            m.persist(&store);
            store.flush().unwrap();
            root = m.root();
        }
        {
            let store = StateStore::open_file(&dir, 1 << 16).unwrap();
            let d: SMap<ValueList, IntermediateTuple> = SMap::from_root(root, 100);
            assert_eq!(
                d.get(&ValueList(vec![Value::Int(42)]), &store).unwrap(),
                Some(tup(42))
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
