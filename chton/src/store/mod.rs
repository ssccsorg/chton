// ── chton store: generic record store surface ─────────────────────────
//
// The store behavior layer: an async, generic (domain-free) key-value
// store with memory and materialized implementations. Moved from nexus
// as part of the behavior-layer ownership: chton is the single owner of
// store behavior; nexus consumes it and keeps only semantics.
//
// Implementations:
// - `MemoryEntityStore<V>`: HashMap-backed memory store.
// - `CoordEntityStore<N, V>`: CoordSpaceN-backed memory store with
//   spatial query methods (axis/prefix filtered iteration).
// - `KvEntityStore<N, V>`: CoordKVStore-backed materialized store; the
//   value codec (postcard) sits at the record boundary, the seam for a
//   future codec layer.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::kv::CoordKVStore;
use crate::origin::Origin;
use async_trait::async_trait;
use tagma_core::{Coord, CoordPath, CoordSpaceN};
use tagma_kv::CoordKV;

use crate::cell::Cell2;

// ── EntityStore trait ────────────────────────────────────────────────────

/// EntityStore: replaceable key-value store for FIH records.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[async_trait]
pub trait EntityStore<V>: Send + Sync
where
    V: Clone + 'static,
{
    async fn get(&self, key: &str) -> Option<V>;
    async fn insert(&self, key: String, value: V) -> Option<V>;
    async fn remove(&self, key: &str) -> Option<V>;
    async fn contains_key(&self, key: &str) -> bool;
    async fn len(&self) -> usize;
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
    async fn values(&self) -> Vec<V>;
    async fn clear(&self);
    async fn replace_from(&self, entries: Vec<(String, V)>);
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
pub trait EntityStore<V>
where
    V: Clone + 'static,
{
    async fn get(&self, key: &str) -> Option<V>;
    async fn insert(&self, key: String, value: V) -> Option<V>;
    async fn remove(&self, key: &str) -> Option<V>;
    async fn contains_key(&self, key: &str) -> bool;
    async fn len(&self) -> usize;
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
    async fn values(&self) -> Vec<V>;
    async fn clear(&self);
    async fn replace_from(&self, entries: Vec<(String, V)>);
}

// ── CoordEntityStore: CoordSpaceN-backed EntityStore ──────────────────

/// Map a string key to a CoordPath<N> deterministically without hashing.
///
/// Two formats:
/// - N-character Hangul: each character is a direct Coord (Phase 2+, CoordId format)
/// - Other keys: the tagma-kv ByteWise mapping, one Coord per UTF-8 byte.
///   Injective for keys of at most N bytes; shorter keys zero-pad and
///   longer keys truncate at the first N bytes.
pub fn str_to_coordpath<const N: usize>(key: &str) -> CoordPath<N> {
    let chars: Vec<char> = key.chars().collect();
    // Fast path: N-character Hangul key → direct Coord mapping
    if chars.len() == N && chars.iter().all(|c| Coord::from_char(*c).is_some()) {
        let mut coords = [Coord::new(0).unwrap(); N];
        for (i, &ch) in chars.iter().enumerate() {
            coords[i] = Coord::from_char(ch).unwrap();
        }
        return CoordPath::new(coords);
    }
    // ByteWise fallback: each UTF-8 byte is one Coord. The generated
    // vector is at most N coords for keys of at most N bytes; longer keys
    // truncate and shorter keys keep the zero padding below.
    let generated = tagma_kv::string_to_coord_path(key).unwrap_or_default();
    let mut coords = [Coord::new(0).unwrap(); N];
    for (i, coord) in coords.iter_mut().enumerate() {
        if let Some(c) = generated.get(i) {
            *coord = *c;
        }
    }
    CoordPath::new(coords)
}

/// EntityStore backed by CoordSpaceN instead of HashMap.
///
/// String keys are mapped to CoordPath<N> deterministically.
/// This is the bridge between the current FihHash-hex-keyed storage
/// interface and the future CoordPath-native storage.
pub struct CoordEntityStore<const N: usize, V> {
    inner: Cell2<CoordSpaceN<N, V>>,
}

impl<const N: usize, V> CoordEntityStore<N, V>
where
    V: Clone + 'static,
{
    pub fn new() -> Self {
        Self {
            inner: Cell2::new(CoordSpaceN::new()),
        }
    }

    /// Iterate over values matching a predicate, cloning only on match.
    /// Avoids the `values()` → Vec → filter pipeline.
    pub async fn iter_filtered<F>(&self, mut predicate: F) -> Vec<V>
    where
        V: Send,
        F: FnMut(&V) -> bool + Send,
    {
        let space = self.inner.borrow();
        let mut results = Vec::with_capacity(space.len().min(128));
        for (_path, v) in space.iter_tree() {
            if predicate(v) {
                results.push(v.clone());
            }
        }
        results
    }

    /// Filter during tree traversal using path coordinates.
    /// For each entry, checks if `path[axis] == value` for all specified
    /// (axis, value) pairs BEFORE cloning. Avoids string comparison
    /// when the filter corresponds to known axis indices.
    ///
    /// Falls back to full scan (path coord check is still faster than
    /// string compare), but when axes 0..k are fully specified, uses
    /// `iter_prefix` for O(subtree) traversal.
    pub async fn axis_filtered(&self, axis_checks: &[(usize, u16)]) -> Vec<V>
    where
        V: Send,
    {
        let space = self.inner.borrow();

        // If axis_checks cover axes 0..k contiguously from the start,
        // use iter_prefix for the subtree.
        let contiguous_prefix = {
            let mut prefix_len = 0;
            for (i, &(axis, _val)) in axis_checks.iter().enumerate() {
                if axis == i {
                    prefix_len = i + 1;
                } else {
                    break;
                }
            }
            if prefix_len > 0 && prefix_len == axis_checks.len() {
                Some(prefix_len)
            } else {
                None
            }
        };

        if let Some(prefix_len) = contiguous_prefix {
            // Build prefix from the first prefix_len axis values
            let mut prefix_coords = Vec::with_capacity(prefix_len);
            for (_, val) in axis_checks.iter().take(prefix_len) {
                if let Some(c) = tagma_core::Coord::new(*val) {
                    prefix_coords.push(c);
                } else {
                    return Vec::new();
                }
            }
            if let Some(iter) = space.iter_prefix(&prefix_coords) {
                let mut results = Vec::new();
                for (_path, v) in iter {
                    results.push(v.clone());
                }
                return results;
            }
            return Vec::new();
        }

        // Non-contiguous: full scan with path coord check
        let mut results = Vec::new();
        'outer: for (path, v) in space.iter_tree() {
            for &(axis, val) in axis_checks {
                if axis >= N || path.coords()[axis].index() != val {
                    continue 'outer;
                }
            }
            results.push(v.clone());
        }
        results
    }

    /// Iterate over values under a CoordPath prefix, cloning only matching entries.
    /// This is the axis-aware fast path — skips entire subtrees that don't match.
    /// Returns `None` if the prefix path doesn't exist.
    pub async fn iter_prefix_filtered<F>(
        &self,
        prefix: &[tagma_core::Coord],
        mut predicate: F,
    ) -> Option<Vec<V>>
    where
        V: Send,
        F: FnMut(&V) -> bool + Send,
    {
        let space = self.inner.borrow();
        let iter = space.iter_prefix(prefix)?;
        let mut results = Vec::new();
        for (_path, v) in iter {
            if predicate(v) {
                results.push(v.clone());
            }
        }
        Some(results)
    }
}

impl<const N: usize, V> Default for CoordEntityStore<N, V>
where
    V: Clone + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[async_trait]
impl<const N: usize, V> EntityStore<V> for CoordEntityStore<N, V>
where
    V: Clone + Send + 'static,
{
    async fn get(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow().at_path(&path).cloned()
    }

    async fn insert(&self, key: String, value: V) -> Option<V> {
        let path = str_to_coordpath::<N>(&key);
        self.inner.borrow_mut().place_path(&path, value)
    }

    async fn remove(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow_mut().vacate_path(&path)
    }

    async fn contains_key(&self, key: &str) -> bool {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow().at_path(&path).is_some()
    }

    async fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    async fn values(&self) -> Vec<V> {
        let space = self.inner.borrow();
        let mut values = Vec::with_capacity(space.len());
        for (_path, v) in space.iter_tree() {
            values.push(v.clone());
        }
        values
    }

    async fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    async fn replace_from(&self, entries: Vec<(String, V)>) {
        let mut space = self.inner.borrow_mut();
        space.clear();
        for (key, value) in entries {
            let path = str_to_coordpath::<N>(&key);
            space.place_path(&path, value);
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
impl<const N: usize, V> EntityStore<V> for CoordEntityStore<N, V>
where
    V: Clone + 'static,
{
    async fn get(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow().at_path(&path).cloned()
    }

    async fn insert(&self, key: String, value: V) -> Option<V> {
        let path = str_to_coordpath::<N>(&key);
        self.inner.borrow_mut().place_path(&path, value)
    }

    async fn remove(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow_mut().vacate_path(&path)
    }

    async fn contains_key(&self, key: &str) -> bool {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow().at_path(&path).is_some()
    }

    async fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    async fn values(&self) -> Vec<V> {
        let space = self.inner.borrow();
        let mut values = Vec::with_capacity(space.len());
        for (_path, v) in space.iter_tree() {
            values.push(v.clone());
        }
        values
    }

    async fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    async fn replace_from(&self, entries: Vec<(String, V)>) {
        let mut space = self.inner.borrow_mut();
        space.clear();
        for (key, value) in entries {
            let path = str_to_coordpath::<N>(&key);
            space.place_path(&path, value);
        }
    }
}

// ── MemoryEntityStore ────────────────────────────────────────────────────

/// In-memory EntityStore using Cell2 (Mutex on native, RefCell on wasm).
pub struct MemoryEntityStore<V> {
    inner: Cell2<HashMap<String, V>>,
}

impl<V> MemoryEntityStore<V>
where
    V: Clone + 'static,
{
    pub fn new() -> Self {
        Self {
            inner: Cell2::new(HashMap::new()),
        }
    }
}

impl<V> Default for MemoryEntityStore<V>
where
    V: Clone + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[async_trait]
impl<V> EntityStore<V> for MemoryEntityStore<V>
where
    V: Clone + Send + 'static,
{
    async fn get(&self, key: &str) -> Option<V> {
        self.inner.borrow().get(key).cloned()
    }

    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.inner.borrow_mut().insert(key, value)
    }

    async fn remove(&self, key: &str) -> Option<V> {
        self.inner.borrow_mut().remove(key)
    }

    async fn contains_key(&self, key: &str) -> bool {
        self.inner.borrow().contains_key(key)
    }

    async fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    async fn values(&self) -> Vec<V> {
        self.inner.borrow().values().cloned().collect()
    }

    async fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    async fn replace_from(&self, entries: Vec<(String, V)>) {
        let mut map = self.inner.borrow_mut();
        map.clear();
        map.extend(entries);
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
impl<V> EntityStore<V> for MemoryEntityStore<V>
where
    V: Clone + 'static,
{
    async fn get(&self, key: &str) -> Option<V> {
        self.inner.borrow().get(key).cloned()
    }

    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.inner.borrow_mut().insert(key, value)
    }

    async fn remove(&self, key: &str) -> Option<V> {
        self.inner.borrow_mut().remove(key)
    }

    async fn contains_key(&self, key: &str) -> bool {
        self.inner.borrow().contains_key(key)
    }

    async fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    async fn values(&self) -> Vec<V> {
        self.inner.borrow().values().cloned().collect()
    }

    async fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    async fn replace_from(&self, entries: Vec<(String, V)>) {
        let mut map = self.inner.borrow_mut();
        map.clear();
        map.extend(entries);
    }
}

// ── KvEntityStore: CoordKVStore-backed EntityStore ─────────────────────────

/// EntityStore over chton's materialized CoordKV.
///
/// The string key maps to a `CoordPath<N>` with the same deterministic
/// mapping as [`CoordEntityStore`]; the value is postcard-encoded into the
/// fixed-size record slot. The store is durable when the underlying origin
/// is flushed: `flush` persists the strategy header and the origin, and
/// `is_buffered` reports whether non-durable state exists.
///
/// The record boundary is the same codec seam as the FileIo surface: the
/// value bytes are opaque to the kv, so a future cipher layer would sit
/// here without changing the trait surface.
pub struct KvEntityStore<const N: usize, V> {
    inner: Cell2<CoordKVStore<N>>,
    marker: PhantomData<V>,
}

impl<const N: usize, V> KvEntityStore<N, V> {
    /// Create a fresh store over `origin`. The origin must be empty;
    /// `load` opens an existing store.
    pub fn new(origin: Box<dyn Origin>, record_slot_size: u64) -> Self {
        Self {
            inner: Cell2::new(CoordKVStore::new(origin, record_slot_size)),
            marker: PhantomData,
        }
    }

    /// Open a store over `origin`: load the header when present, otherwise
    /// create a fresh store with `default_record_slot_size`.
    pub fn load(origin: Box<dyn Origin>, default_record_slot_size: u64) -> Result<Self, String> {
        let kv = CoordKVStore::load(origin, default_record_slot_size).map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Cell2::new(kv),
            marker: PhantomData,
        })
    }

    /// Whether the store holds buffered state that is not yet durable.
    pub fn is_buffered(&self) -> bool {
        self.inner.borrow().is_buffered()
    }

    /// Persist buffered state to the medium.
    pub fn flush(&self) -> Result<(), String> {
        self.inner.borrow_mut().flush().map_err(|e| e.to_string())
    }
}

/// Decode a stored value, panicking at the trait boundary. The
/// `EntityStore` surface is infallible, so IO and codec errors panic with
/// a descriptive message, mirroring the tagma-kv CoordKV contract.
fn decode_value<V>(bytes: Vec<u8>) -> V
where
    V: serde::de::DeserializeOwned,
{
    postcard::from_bytes(&bytes).unwrap_or_else(|e| panic!("kv entity store decode failed: {e}"))
}

/// Encode a value for storage.
fn encode_value<V>(value: &V) -> Vec<u8>
where
    V: serde::Serialize,
{
    postcard::to_allocvec(value).unwrap_or_else(|e| panic!("kv entity store encode failed: {e}"))
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[async_trait]
impl<const N: usize, V> EntityStore<V> for KvEntityStore<N, V>
where
    V: Clone + Send + Sync + 'static + serde::Serialize + serde::de::DeserializeOwned,
{
    async fn get(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        let value = self
            .inner
            .borrow()
            .get_path(&path)
            .unwrap_or_else(|e| panic!("kv entity store get failed: {e}"))?;
        Some(decode_value(value))
    }

    async fn insert(&self, key: String, value: V) -> Option<V> {
        let path = str_to_coordpath::<N>(&key);
        let bytes = encode_value(&value);
        let prev = self
            .inner
            .borrow_mut()
            .put_path(&path, &bytes)
            .unwrap_or_else(|e| panic!("kv entity store insert failed: {e}"));
        prev.map(decode_value)
    }

    async fn remove(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        let prev = self
            .inner
            .borrow_mut()
            .remove_path(&path)
            .unwrap_or_else(|e| panic!("kv entity store remove failed: {e}"));
        prev.map(decode_value)
    }

    async fn contains_key(&self, key: &str) -> bool {
        let path = str_to_coordpath::<N>(key);
        self.inner
            .borrow()
            .get_path(&path)
            .unwrap_or_else(|e| panic!("kv entity store get failed: {e}"))
            .is_some()
    }

    async fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    async fn values(&self) -> Vec<V> {
        let entries = self
            .inner
            .borrow()
            .iter()
            .unwrap_or_else(|e| panic!("kv entity store iter failed: {e}"));
        entries
            .into_iter()
            .map(|(_, value)| decode_value(value))
            .collect()
    }

    async fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    async fn replace_from(&self, entries: Vec<(String, V)>) {
        let mut kv = self.inner.borrow_mut();
        kv.clear();
        for (key, value) in entries {
            let path = str_to_coordpath::<N>(&key);
            let bytes = encode_value(&value);
            kv.put_path(&path, &bytes)
                .unwrap_or_else(|e| panic!("kv entity store insert failed: {e}"));
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
impl<const N: usize, V> EntityStore<V> for KvEntityStore<N, V>
where
    V: Clone + 'static + serde::Serialize + serde::de::DeserializeOwned,
{
    async fn get(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        let value = self
            .inner
            .borrow()
            .get_path(&path)
            .unwrap_or_else(|e| panic!("kv entity store get failed: {e}"))?;
        Some(decode_value(value))
    }

    async fn insert(&self, key: String, value: V) -> Option<V> {
        let path = str_to_coordpath::<N>(&key);
        let bytes = encode_value(&value);
        let prev = self
            .inner
            .borrow_mut()
            .put_path(&path, &bytes)
            .unwrap_or_else(|e| panic!("kv entity store insert failed: {e}"));
        prev.map(decode_value)
    }

    async fn remove(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        let prev = self
            .inner
            .borrow_mut()
            .remove_path(&path)
            .unwrap_or_else(|e| panic!("kv entity store remove failed: {e}"));
        prev.map(decode_value)
    }

    async fn contains_key(&self, key: &str) -> bool {
        let path = str_to_coordpath::<N>(key);
        self.inner
            .borrow()
            .get_path(&path)
            .unwrap_or_else(|e| panic!("kv entity store get failed: {e}"))
            .is_some()
    }

    async fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    async fn values(&self) -> Vec<V> {
        let entries = self
            .inner
            .borrow()
            .iter()
            .unwrap_or_else(|e| panic!("kv entity store iter failed: {e}"));
        entries
            .into_iter()
            .map(|(_, value)| decode_value(value))
            .collect()
    }

    async fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    async fn replace_from(&self, entries: Vec<(String, V)>) {
        let mut kv = self.inner.borrow_mut();
        kv.clear();
        for (key, value) in entries {
            let path = str_to_coordpath::<N>(&key);
            let bytes = encode_value(&value);
            kv.put_path(&path, &bytes)
                .unwrap_or_else(|e| panic!("kv entity store insert failed: {e}"));
        }
    }
}
