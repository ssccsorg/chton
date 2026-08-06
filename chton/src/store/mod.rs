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
//
// Key contract: canonical keys are exactly N Hangul characters (the
// CoordId string form) and map injectively. Any other key maps through a
// SHA-256 fingerprint, so collisions are negligible and key length is
// unbounded. Do not mix the two formats for the same key.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::cell::Cell2;
use crate::kv::CoordKVStore;
use crate::origin::Origin;
use async_trait::async_trait;
use sha2::Digest;
use tagma_core::{Coord, CoordPath, CoordSpaceN};
use tagma_kv::CoordKV;

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

/// Map a string key to a CoordPath<N> deterministically.
///
/// Two formats, matching the CoordId string convention:
/// - N-character Hangul: each character maps directly to a Coord. This
///   path is injective; it is the canonical key form.
/// - Any other key: SHA-256 fingerprint split across the N coords (mod
///   11172 each). Collisions are negligible (256-bit digest over about
///   13.4-bit coords), key length is unbounded, and there is no
///   truncation or zero-padding, so keys that previously collided under
///   a byte-wise mapping ("ab" vs "ab\0", keys longer than N bytes, byte
///   keys vs Hangul keys sharing leading coords) now map to distinct
///   paths.
pub fn str_to_coordpath<const N: usize>(key: &str) -> CoordPath<N> {
    let chars: Vec<char> = key.chars().collect();
    // Canonical path: N-character Hangul key → direct Coord mapping.
    if chars.len() == N && chars.iter().all(|c| Coord::from_char(*c).is_some()) {
        let mut coords = [Coord::new(0).unwrap(); N];
        for (i, &ch) in chars.iter().enumerate() {
            coords[i] = Coord::from_char(ch).unwrap();
        }
        return CoordPath::new(coords);
    }
    // Hash fallback for arbitrary keys.
    let digest = sha2::Sha256::digest(key.as_bytes());
    let mut coords = [Coord::new(0).unwrap(); N];
    for (i, coord) in coords.iter_mut().enumerate() {
        let idx = u16::from_le_bytes([
            digest.get(i * 2).copied().unwrap_or(0),
            digest.get(i * 2 + 1).copied().unwrap_or(0),
        ]) % 11172;
        *coord = Coord::new(idx).unwrap();
    }
    CoordPath::new(coords)
}

/// EntityStore backed by CoordSpaceN instead of HashMap.
///
/// String keys map through [`str_to_coordpath`]: canonical N-Hangul keys
/// directly, any other key through a SHA-256 fingerprint. This is the
/// bridge between the current string-keyed storage interface and
/// CoordPath-native storage.
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
    /// Precondition: `axis_checks` must be sorted by axis. Only a check
    /// set that covers axes 0..k contiguously from the start takes the
    /// O(subtree) `iter_prefix` path; any other shape silently falls back
    /// to a full scan with path-coord checks.
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

    // ── EntityStore behavior (single source, platform-agnostic) ────────

    pub(crate) async fn get_entry(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow().at_path(&path).cloned()
    }

    pub(crate) async fn insert_entry(&self, key: String, value: V) -> Option<V> {
        let path = str_to_coordpath::<N>(&key);
        self.inner.borrow_mut().place_path(&path, value)
    }

    pub(crate) async fn remove_entry(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow_mut().vacate_path(&path)
    }

    pub(crate) async fn contains_entry(&self, key: &str) -> bool {
        let path = str_to_coordpath::<N>(key);
        self.inner.borrow().at_path(&path).is_some()
    }

    pub(crate) async fn entry_count(&self) -> usize {
        self.inner.borrow().len()
    }

    pub(crate) async fn all_values(&self) -> Vec<V> {
        let space = self.inner.borrow();
        let mut values = Vec::with_capacity(space.len());
        for (_path, v) in space.iter_tree() {
            values.push(v.clone());
        }
        values
    }

    pub(crate) async fn clear_entries(&self) {
        self.inner.borrow_mut().clear();
    }

    pub(crate) async fn replace_entries(&self, entries: Vec<(String, V)>) {
        let mut space = self.inner.borrow_mut();
        space.clear();
        for (key, value) in entries {
            let path = str_to_coordpath::<N>(&key);
            space.place_path(&path, value);
        }
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
        self.get_entry(key).await
    }
    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.insert_entry(key, value).await
    }
    async fn remove(&self, key: &str) -> Option<V> {
        self.remove_entry(key).await
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.contains_entry(key).await
    }
    async fn len(&self) -> usize {
        self.entry_count().await
    }
    async fn values(&self) -> Vec<V> {
        self.all_values().await
    }
    async fn clear(&self) {
        self.clear_entries().await
    }
    async fn replace_from(&self, entries: Vec<(String, V)>) {
        self.replace_entries(entries).await
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
impl<const N: usize, V> EntityStore<V> for CoordEntityStore<N, V>
where
    V: Clone + 'static,
{
    async fn get(&self, key: &str) -> Option<V> {
        self.get_entry(key).await
    }
    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.insert_entry(key, value).await
    }
    async fn remove(&self, key: &str) -> Option<V> {
        self.remove_entry(key).await
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.contains_entry(key).await
    }
    async fn len(&self) -> usize {
        self.entry_count().await
    }
    async fn values(&self) -> Vec<V> {
        self.all_values().await
    }
    async fn clear(&self) {
        self.clear_entries().await
    }
    async fn replace_from(&self, entries: Vec<(String, V)>) {
        self.replace_entries(entries).await
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

    // ── EntityStore behavior (single source, platform-agnostic) ────────

    pub(crate) async fn get_entry(&self, key: &str) -> Option<V> {
        self.inner.borrow().get(key).cloned()
    }

    pub(crate) async fn insert_entry(&self, key: String, value: V) -> Option<V> {
        self.inner.borrow_mut().insert(key, value)
    }

    pub(crate) async fn remove_entry(&self, key: &str) -> Option<V> {
        self.inner.borrow_mut().remove(key)
    }

    pub(crate) async fn contains_entry(&self, key: &str) -> bool {
        self.inner.borrow().contains_key(key)
    }

    pub(crate) async fn entry_count(&self) -> usize {
        self.inner.borrow().len()
    }

    pub(crate) async fn all_values(&self) -> Vec<V> {
        self.inner.borrow().values().cloned().collect()
    }

    pub(crate) async fn clear_entries(&self) {
        self.inner.borrow_mut().clear();
    }

    pub(crate) async fn replace_entries(&self, entries: Vec<(String, V)>) {
        let mut map = self.inner.borrow_mut();
        map.clear();
        map.extend(entries);
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
        self.get_entry(key).await
    }
    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.insert_entry(key, value).await
    }
    async fn remove(&self, key: &str) -> Option<V> {
        self.remove_entry(key).await
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.contains_entry(key).await
    }
    async fn len(&self) -> usize {
        self.entry_count().await
    }
    async fn values(&self) -> Vec<V> {
        self.all_values().await
    }
    async fn clear(&self) {
        self.clear_entries().await
    }
    async fn replace_from(&self, entries: Vec<(String, V)>) {
        self.replace_entries(entries).await
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
impl<V> EntityStore<V> for MemoryEntityStore<V>
where
    V: Clone + 'static,
{
    async fn get(&self, key: &str) -> Option<V> {
        self.get_entry(key).await
    }
    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.insert_entry(key, value).await
    }
    async fn remove(&self, key: &str) -> Option<V> {
        self.remove_entry(key).await
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.contains_entry(key).await
    }
    async fn len(&self) -> usize {
        self.entry_count().await
    }
    async fn values(&self) -> Vec<V> {
        self.all_values().await
    }
    async fn clear(&self) {
        self.clear_entries().await
    }
    async fn replace_from(&self, entries: Vec<(String, V)>) {
        self.replace_entries(entries).await
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
/// The trait surface is infallible, so IO and codec errors panic with a
/// descriptive message once the interior borrow is released: a failed
/// operation never leaves the backing mutex poisoned. This mirrors the
/// tagma-kv CoordKV contract.
///
/// The record boundary is the same codec seam as the FileIo surface: the
/// value bytes are opaque to the kv, so a future cipher layer would sit
/// here without changing the trait surface.
pub struct KvEntityStore<const N: usize, V> {
    inner: Cell2<CoordKVStore<N>>,
    marker: PhantomData<V>,
}

impl<const N: usize, V> KvEntityStore<N, V>
where
    V: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
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

    // ── EntityStore behavior (single source, platform-agnostic) ────────

    pub(crate) async fn get_entry(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        let result = {
            let kv = self.inner.borrow();
            kv.get_path(&path)
        };
        match result {
            Ok(value) => value.map(decode_value),
            Err(e) => panic!("kv entity store get failed: {e}"),
        }
    }

    pub(crate) async fn insert_entry(&self, key: String, value: V) -> Option<V> {
        let path = str_to_coordpath::<N>(&key);
        let bytes = encode_value(&value);
        let result = {
            let mut kv = self.inner.borrow_mut();
            kv.put_path(&path, &bytes)
        };
        match result {
            Ok(prev) => prev.map(decode_value),
            Err(e) => panic!("kv entity store insert failed: {e}"),
        }
    }

    pub(crate) async fn remove_entry(&self, key: &str) -> Option<V> {
        let path = str_to_coordpath::<N>(key);
        let result = {
            let mut kv = self.inner.borrow_mut();
            kv.remove_path(&path)
        };
        match result {
            Ok(prev) => prev.map(decode_value),
            Err(e) => panic!("kv entity store remove failed: {e}"),
        }
    }

    pub(crate) async fn contains_entry(&self, key: &str) -> bool {
        let path = str_to_coordpath::<N>(key);
        let result = {
            let kv = self.inner.borrow();
            kv.get_path(&path)
        };
        match result {
            Ok(value) => value.is_some(),
            Err(e) => panic!("kv entity store get failed: {e}"),
        }
    }

    pub(crate) async fn entry_count(&self) -> usize {
        self.inner.borrow().len()
    }

    pub(crate) async fn all_values(&self) -> Vec<V> {
        let result = {
            let kv = self.inner.borrow();
            kv.iter()
        };
        let entries = match result {
            Ok(entries) => entries,
            Err(e) => panic!("kv entity store iter failed: {e}"),
        };
        entries
            .into_iter()
            .map(|(_, value)| decode_value(value))
            .collect()
    }

    pub(crate) async fn clear_entries(&self) {
        let result = {
            let mut kv = self.inner.borrow_mut();
            kv.clear_checked()
        };
        if let Err(e) = result {
            panic!("kv entity store clear failed: {e}");
        }
    }

    pub(crate) async fn replace_entries(&self, entries: Vec<(String, V)>) {
        // Encode before borrowing so a codec panic never holds the guard.
        let encoded: Vec<(CoordPath<N>, Vec<u8>)> = entries
            .into_iter()
            .map(|(key, value)| (str_to_coordpath::<N>(&key), encode_value(&value)))
            .collect();
        let result = {
            let mut kv = self.inner.borrow_mut();
            (|| -> Result<(), String> {
                kv.clear_checked().map_err(|e| e.to_string())?;
                for (path, bytes) in encoded {
                    kv.put_path(&path, &bytes).map_err(|e| e.to_string())?;
                }
                Ok(())
            })()
        };
        if let Err(e) = result {
            panic!("kv entity store replace failed: {e}");
        }
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
        self.get_entry(key).await
    }
    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.insert_entry(key, value).await
    }
    async fn remove(&self, key: &str) -> Option<V> {
        self.remove_entry(key).await
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.contains_entry(key).await
    }
    async fn len(&self) -> usize {
        self.entry_count().await
    }
    async fn values(&self) -> Vec<V> {
        self.all_values().await
    }
    async fn clear(&self) {
        self.clear_entries().await
    }
    async fn replace_from(&self, entries: Vec<(String, V)>) {
        self.replace_entries(entries).await
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[async_trait(?Send)]
impl<const N: usize, V> EntityStore<V> for KvEntityStore<N, V>
where
    V: Clone + 'static + serde::Serialize + serde::de::DeserializeOwned,
{
    async fn get(&self, key: &str) -> Option<V> {
        self.get_entry(key).await
    }
    async fn insert(&self, key: String, value: V) -> Option<V> {
        self.insert_entry(key, value).await
    }
    async fn remove(&self, key: &str) -> Option<V> {
        self.remove_entry(key).await
    }
    async fn contains_key(&self, key: &str) -> bool {
        self.contains_entry(key).await
    }
    async fn len(&self) -> usize {
        self.entry_count().await
    }
    async fn values(&self) -> Vec<V> {
        self.all_values().await
    }
    async fn clear(&self) {
        self.clear_entries().await
    }
    async fn replace_from(&self, entries: Vec<(String, V)>) {
        self.replace_entries(entries).await
    }
}
