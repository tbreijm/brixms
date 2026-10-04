//! Structurally shared ordered maps and sets. Mutations copy only an AVL path.
use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::ops::Index;
use std::sync::Arc;

type Link<K, V> = Option<Arc<Node<K, V>>>;
#[derive(Clone)]
struct Node<K, V> {
    key: K,
    value: V,
    left: Link<K, V>,
    right: Link<K, V>,
    height: usize,
    size: usize,
}
fn height<K, V>(n: &Link<K, V>) -> usize {
    n.as_ref().map_or(0, |n| n.height)
}
fn size<K, V>(n: &Link<K, V>) -> usize {
    n.as_ref().map_or(0, |n| n.size)
}
impl<K, V> Node<K, V> {
    fn refresh(&mut self) {
        self.height = 1 + height(&self.left).max(height(&self.right));
        self.size = 1 + size(&self.left) + size(&self.right);
    }
}
fn rotate_left<K: Clone, V: Clone>(mut root: Arc<Node<K, V>>) -> Arc<Node<K, V>> {
    let mut right = Arc::make_mut(&mut root)
        .right
        .take()
        .expect("AVL right child");
    Arc::make_mut(&mut root).right = Arc::make_mut(&mut right).left.take();
    Arc::make_mut(&mut root).refresh();
    Arc::make_mut(&mut right).left = Some(root);
    Arc::make_mut(&mut right).refresh();
    right
}
fn rotate_right<K: Clone, V: Clone>(mut root: Arc<Node<K, V>>) -> Arc<Node<K, V>> {
    let mut left = Arc::make_mut(&mut root)
        .left
        .take()
        .expect("AVL left child");
    Arc::make_mut(&mut root).left = Arc::make_mut(&mut left).right.take();
    Arc::make_mut(&mut root).refresh();
    Arc::make_mut(&mut left).right = Some(root);
    Arc::make_mut(&mut left).refresh();
    left
}
fn balance<K: Clone, V: Clone>(mut root: Arc<Node<K, V>>) -> Arc<Node<K, V>> {
    Arc::make_mut(&mut root).refresh();
    if height(&root.left) > height(&root.right) + 1 {
        let left = root.left.as_ref().unwrap();
        if height(&left.right) > height(&left.left) {
            let left = Arc::make_mut(&mut root).left.take().unwrap();
            Arc::make_mut(&mut root).left = Some(rotate_left(left));
        }
        rotate_right(root)
    } else if height(&root.right) > height(&root.left) + 1 {
        let right = root.right.as_ref().unwrap();
        if height(&right.left) > height(&right.right) {
            let right = Arc::make_mut(&mut root).right.take().unwrap();
            Arc::make_mut(&mut root).right = Some(rotate_right(right));
        }
        rotate_left(root)
    } else {
        root
    }
}
fn insert<K: Ord + Clone, V: Clone>(root: Link<K, V>, key: K, value: V) -> (Link<K, V>, Option<V>) {
    let Some(mut root) = root else {
        return (
            Some(Arc::new(Node {
                key,
                value,
                left: None,
                right: None,
                height: 1,
                size: 1,
            })),
            None,
        );
    };
    let old;
    match key.cmp(&root.key) {
        Ordering::Less => {
            let node = Arc::make_mut(&mut root);
            let result = insert(node.left.take(), key, value);
            node.left = result.0;
            old = result.1;
        }
        Ordering::Greater => {
            let node = Arc::make_mut(&mut root);
            let result = insert(node.right.take(), key, value);
            node.right = result.0;
            old = result.1;
        }
        Ordering::Equal => {
            old = Some(std::mem::replace(
                &mut Arc::make_mut(&mut root).value,
                value,
            ));
        }
    }
    (Some(balance(root)), old)
}
fn take_min<K: Clone, V: Clone>(mut root: Arc<Node<K, V>>) -> (Link<K, V>, K, V) {
    if root.left.is_none() {
        let node = Arc::make_mut(&mut root);
        return (node.right.take(), node.key.clone(), node.value.clone());
    }
    let left = Arc::make_mut(&mut root).left.take().unwrap();
    let (left, key, value) = take_min(left);
    Arc::make_mut(&mut root).left = left;
    (Some(balance(root)), key, value)
}
fn remove<K: Ord + Clone + Borrow<Q>, V: Clone, Q: Ord + ?Sized>(
    root: Link<K, V>,
    key: &Q,
) -> (Link<K, V>, Option<V>) {
    let Some(mut root) = root else {
        return (None, None);
    };
    let old;
    match key.cmp(root.key.borrow()) {
        Ordering::Less => {
            let node = Arc::make_mut(&mut root);
            let result = remove(node.left.take(), key);
            node.left = result.0;
            old = result.1;
        }
        Ordering::Greater => {
            let node = Arc::make_mut(&mut root);
            let result = remove(node.right.take(), key);
            node.right = result.0;
            old = result.1;
        }
        Ordering::Equal => {
            let node = Arc::make_mut(&mut root);
            old = Some(node.value.clone());
            if node.left.is_none() {
                return (node.right.take(), old);
            }
            if node.right.is_none() {
                return (node.left.take(), old);
            }
            let (right, key, value) = take_min(node.right.take().unwrap());
            node.right = right;
            node.key = key;
            node.value = value;
        }
    }
    (Some(balance(root)), old)
}

#[derive(Clone)]
pub struct PMap<K, V> {
    root: Link<K, V>,
}
impl<K, V> Default for PMap<K, V> {
    fn default() -> Self {
        Self { root: None }
    }
}
impl<K, V> PMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        size(&self.root)
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
    pub fn clear(&mut self) {
        self.root = None;
    }
    pub fn iter(&self) -> Iter<'_, K, V> {
        let mut it = Iter { stack: Vec::new() };
        it.push_left(self.root.as_deref());
        it
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let mut node = self.root.as_deref();
        while let Some(n) = node {
            match key.cmp(n.key.borrow()) {
                Ordering::Less => node = n.left.as_deref(),
                Ordering::Greater => node = n.right.as_deref(),
                Ordering::Equal => return Some(&n.value),
            }
        }
        None
    }
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }
}
impl<K: Ord + Clone, V: Clone> PMap<K, V> {
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let (root, old) = insert(self.root.take(), key, value);
        self.root = root;
        old
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        if !self.contains_key(key) {
            return None;
        }
        let (root, old) = remove(self.root.take(), key);
        self.root = root;
        old
    }
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        if !self.contains_key(key) {
            return None;
        }
        let mut node = self.root.as_mut();
        while let Some(n) = node {
            let n = Arc::make_mut(n);
            match key.cmp(n.key.borrow()) {
                Ordering::Less => node = n.left.as_mut(),
                Ordering::Greater => node = n.right.as_mut(),
                Ordering::Equal => return Some(&mut n.value),
            }
        }
        None
    }
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        Entry { map: self, key }
    }
}
pub struct Entry<'a, K, V> {
    map: &'a mut PMap<K, V>,
    key: K,
}
impl<'a, K: Ord + Clone, V: Clone> Entry<'a, K, V> {
    pub fn or_insert_with(self, f: impl FnOnce() -> V) -> &'a mut V {
        if !self.map.contains_key(&self.key) {
            self.map.insert(self.key.clone(), f());
        }
        self.map.get_mut(&self.key).unwrap()
    }
    pub fn or_insert(self, value: V) -> &'a mut V {
        self.or_insert_with(|| value)
    }
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        self.or_insert_with(V::default)
    }
}
pub struct Iter<'a, K, V> {
    stack: Vec<&'a Node<K, V>>,
}
impl<'a, K, V> Iter<'a, K, V> {
    fn push_left(&mut self, mut node: Option<&'a Node<K, V>>) {
        while let Some(n) = node {
            self.stack.push(n);
            node = n.left.as_deref();
        }
    }
}
impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        self.push_left(node.right.as_deref());
        Some((&node.key, &node.value))
    }
}
impl<'a, K, V> IntoIterator for &'a PMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for PMap<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}
impl<K: PartialEq, V: PartialEq> PartialEq for PMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}
impl<K: Eq, V: Eq> Eq for PMap<K, V> {}
impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for PMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}
impl<K: Borrow<Q>, V, Q: Ord + ?Sized> Index<&Q> for PMap<K, V> {
    type Output = V;
    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("missing PMap key")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PSet<K> {
    map: PMap<K, ()>,
}
impl<K> Default for PSet<K> {
    fn default() -> Self {
        Self::new()
    }
}
impl<K> PSet<K> {
    pub fn new() -> Self {
        Self { map: PMap::new() }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }
    pub fn contains<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.contains_key(key)
    }
    pub fn clear(&mut self) {
        self.map.clear()
    }
}
impl<K: Ord + Clone> PSet<K> {
    pub fn insert(&mut self, key: K) -> bool {
        self.map.insert(key, ()).is_none()
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.remove(key).is_some()
    }
}
impl<K: Ord + Clone> FromIterator<K> for PSet<K> {
    fn from_iter<T: IntoIterator<Item = K>>(iter: T) -> Self {
        Self {
            map: iter.into_iter().map(|k| (k, ())).collect(),
        }
    }
}
impl<K: fmt::Debug> fmt::Debug for PSet<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}
pub struct SetIter<'a, K>(Iter<'a, K, ()>);
impl<'a, K> Iterator for SetIter<'a, K> {
    type Item = &'a K;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, _)| k)
    }
}
impl<'a, K> IntoIterator for &'a PSet<K> {
    type Item = &'a K;
    type IntoIter = SetIter<'a, K>;
    fn into_iter(self) -> Self::IntoIter {
        SetIter(self.map.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn validate<K: Ord, V>(node: &Link<K, V>) -> (usize, usize) {
        let Some(n) = node else { return (0, 0) };
        let (lh, ls) = validate(&n.left);
        let (rh, rs) = validate(&n.right);
        assert!(lh.abs_diff(rh) <= 1);
        assert_eq!(n.height, 1 + lh.max(rh));
        assert_eq!(n.size, 1 + ls + rs);
        if let Some(l) = &n.left {
            assert!(l.key < n.key)
        }
        if let Some(r) = &n.right {
            assert!(r.key > n.key)
        }
        (n.height, n.size)
    }
    #[test]
    fn differential_updates_preserve_snapshots() {
        let mut map = PMap::new();
        let mut reference = BTreeMap::new();
        let mut seed = 19u64;
        let mut snapshots = Vec::new();
        for i in 0..20_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let key = (seed >> 32) % 997;
            match seed % 4 {
                0 => assert_eq!(map.remove(&key), reference.remove(&key)),
                1 => {
                    *map.entry(key).or_default() += 1;
                    *reference.entry(key).or_default() += 1;
                }
                _ => assert_eq!(map.insert(key, i), reference.insert(key, i)),
            }
            if i % 137 == 0 {
                validate(&map.root);
                assert_eq!(
                    map.iter().collect::<Vec<_>>(),
                    reference.iter().collect::<Vec<_>>()
                );
                snapshots.push((map.clone(), reference.clone()));
            }
        }
        for (map, reference) in snapshots {
            validate(&map.root);
            assert_eq!(
                map.iter().collect::<Vec<_>>(),
                reference.iter().collect::<Vec<_>>()
            );
        }
        for key in reference.keys().copied().collect::<Vec<_>>() {
            map.remove(&key);
            validate(&map.root);
        }
        assert!(map.is_empty());
    }
    #[test]
    fn clone_copies_only_changed_paths_and_nested_maps() {
        #[derive(Debug)]
        struct Counted(Arc<std::sync::atomic::AtomicUsize>);
        impl Clone for Counted {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Self(self.0.clone())
            }
        }
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut map: PMap<_, _> = (0..65_536).map(|k| (k, Counted(count.clone()))).collect();
        count.store(0, std::sync::atomic::Ordering::Relaxed);
        let snapshot = map.clone();
        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 0);
        map.insert(32767, Counted(count.clone()));
        let clones = count.load(std::sync::atomic::Ordering::Relaxed);
        assert!(clones <= 18, "copied {clones} payloads");
        assert_eq!(snapshot.len(), 65_536);
        validate(&map.root);
        let mut nested = PMap::new();
        nested.insert(
            "orders".to_owned(),
            (0..1000).map(|i| (i, i)).collect::<PMap<_, _>>(),
        );
        let old = nested.clone();
        nested.get_mut("orders").unwrap().insert(7, 99);
        assert_eq!(old["orders"][&7], 7);
        assert_eq!(nested["orders"][&7], 99);
    }
}
