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
// - `MapEntityStore<N, V>`: CoordMapStore-backed materialized store; the
//   value codec (postcard) sits at the record boundary, the seam for a
//   future codec layer.
//
// Key contract: the consumer chooses the store depth M. Keys of exactly
// M-1 Hangul characters (the CoordId string form) map directly onto axes
// 0..M-2 with marker axis M-1 = 0 and are injective by construction. Any
// other key of length 1..=capacity maps onto axes 0..M-2 as big-endian
// base-11172 digits with the byte length on axis M-1: no truncation, no
// padding, no hashing, and structurally injective within the declared
// capacity of (M-1) x log2(11172) ~= 13.45(M-1) bits. Keys beyond the
// capacity are rejected with KeyError::TooLong. Do not mix the two
// formats for the same key.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::cell::Cell2;
use crate::map::CoordMapStore;
use crate::origin::Origin;
use async_trait::async_trait;
use tagma_core::{Coord, CoordPath, CoordSpaceN};
use tagma_map::CoordMap;

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

/// Maximum store depth. The general key encoding stores the byte length
/// on the marker axis, a Coord (0..11171); at depth 1024 the capacity is
/// about 1720 bytes, far beyond any practical key, so the marker never
/// overflows. Depths above this are rejected at compile time.
pub const MAX_STORE_DEPTH: usize = 1024;

/// Errors from mapping a string key onto a coordinate path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The key is empty; an empty string has no representable path.
    Empty,
    /// The key byte string exceeds the payload capacity of M-1 axes.
    TooLong { len: usize, depth: usize },
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Empty => write!(f, "empty key cannot be mapped to a coordinate path"),
            KeyError::TooLong { len, depth } => write!(
                f,
                "key of {len} bytes exceeds the {depth}-axis payload capacity"
            ),
        }
    }
}

impl std::error::Error for KeyError {}

/// Map a string key onto a `CoordPath<M>` deterministically and
/// injectively. The depth M is the consumer's parameter and must be at
/// least 2: axis M-1 is the marker, axes 0..M-2 carry payload. Canonical
/// keys are exactly M-1 Hangul characters.
///
/// Two formats, separated by the marker axis M-1:
/// - Canonical (M-1 Hangul characters): each character maps directly to
///   a Coord on axes 0..M-2, marker axis 0. Injective by construction.
/// - General (any other string of length 1..=capacity): the marker axis
///   holds the byte length, axes 0..M-2 hold the big-endian base-11172
///   digits of the byte string. No truncation, no padding, no hashing.
///   Structurally injective within the declared capacity of about
///   13.45(M-1) bits; keys beyond it are rejected with
///   [`KeyError::TooLong`].
pub fn str_to_coordpath<const M: usize>(key: &str) -> Result<CoordPath<M>, KeyError> {
    const {
        assert!(
            M >= 2,
            "depth must be at least 2: the marker axis and one payload axis"
        );
        assert!(
            M <= MAX_STORE_DEPTH,
            "depth exceeds the compile-time capacity bound"
        );
    }
    let chars: Vec<char> = key.chars().collect();
    // Canonical path: exactly M-1 Hangul characters, marker axis 0.
    if chars.len() == M - 1 && chars.iter().all(|c| Coord::from_char(*c).is_some()) {
        let mut coords = [Coord::new(0).unwrap(); M];
        for (i, &ch) in chars.iter().enumerate() {
            coords[i] = Coord::from_char(ch).unwrap();
        }
        return Ok(CoordPath::new(coords));
    }
    encode_general::<M>(key)
}

/// Encode an arbitrary byte string onto axes 0..M-2 as big-endian
/// base-11172 digits, with the byte length on the marker axis M-1.
fn encode_general<const M: usize>(key: &str) -> Result<CoordPath<M>, KeyError> {
    let bytes = key.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return Err(KeyError::Empty);
    }
    // Repeated division by 11172 extracts base-11172 digits, least
    // significant first. At most M-1 digits fit; anything left over
    // after M-1 divisions exceeds the capacity.
    let mut value: Vec<u8> = bytes.to_vec();
    let mut digits = [0u16; M];
    for i in 0..(M - 1) {
        let (q, r) = divmod_11172(&value);
        digits[M - 2 - i] = r;
        value = q;
    }
    if value.iter().any(|&b| b != 0) {
        return Err(KeyError::TooLong { len, depth: M - 1 });
    }
    let mut coords = [Coord::new(0).unwrap(); M];
    for (i, coord) in coords.iter_mut().enumerate().take(M - 1) {
        *coord = Coord::new(digits[i]).unwrap();
    }
    // The marker axis holds the byte length. The depth bound above keeps
    // this below N_VALID, so the conversion never truncates; try_from is
    // defense in depth against future refactors.
    let len_coord = u16::try_from(len).map_err(|_| KeyError::TooLong { len, depth: M - 1 })?;
    coords[M - 1] = Coord::new(len_coord).ok_or(KeyError::TooLong { len, depth: M - 1 })?;
    Ok(CoordPath::new(coords))
}

/// Divide a big-endian byte vector by 11172; returns (quotient, remainder).
fn divmod_11172(value: &[u8]) -> (Vec<u8>, u16) {
    let mut rem: u32 = 0;
    let mut quotient = Vec::with_capacity(value.len());
    let mut started = false;
    for &b in value {
        let cur = rem * 256 + b as u32;
        let q = cur / 11172;
        rem = cur % 11172;
        if q != 0 || started {
            started = true;
            quotient.push(q as u8);
        }
    }
    if quotient.is_empty() {
        quotient.push(0);
    }
    (quotient, rem as u16)
}

/// Map a key to its path, panicking on a contract violation. Only
/// insert paths need this: probing (get/remove/contains) treats an
/// unrepresentable key as absent.
fn map_key_or_panic<const N: usize>(key: &str) -> CoordPath<N> {
    str_to_coordpath::<N>(key).unwrap_or_else(|e| panic!("entity store key error: {e}"))
}

/// EntityStore backed by CoordSpaceN instead of HashMap.
///
/// String keys map through [`str_to_coordpath`] with the store depth as
/// the consumer parameter: canonical M-1 Hangul keys directly, any other
/// key through the injective length-prefix encoding. This is the bridge
/// between the current string-keyed storage interface and
/// CoordPath-native storage.
pub struct CoordEntityStore<const N: usize, V> {
    inner: Cell2<CoordSpaceN<N, V>>,
}

impl<const N: usize, V> CoordEntityStore<N, V>
where
    V: Clone + 'static,
{
    pub fn new() -> Self {
        const {
            assert!(
                N >= 2,
                "depth must be at least 2: the marker axis and one payload axis"
            );
            assert!(
                N <= MAX_STORE_DEPTH,
                "depth exceeds the compile-time capacity bound"
            );
        }
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
        let Ok(path) = str_to_coordpath::<N>(key) else {
            return None;
        };
        self.inner.borrow().at_path(&path).cloned()
    }

    pub(crate) async fn insert_entry(&self, key: String, value: V) -> Option<V> {
        let path = map_key_or_panic::<N>(&key);
        self.inner.borrow_mut().place_path(&path, value)
    }

    pub(crate) async fn remove_entry(&self, key: &str) -> Option<V> {
        let Ok(path) = str_to_coordpath::<N>(key) else {
            return None;
        };
        self.inner.borrow_mut().vacate_path(&path)
    }

    pub(crate) async fn contains_entry(&self, key: &str) -> bool {
        let Ok(path) = str_to_coordpath::<N>(key) else {
            return false;
        };
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
            let path = map_key_or_panic::<N>(&key);
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

// ── MapEntityStore: CoordMapStore-backed EntityStore ─────────────────────────

/// EntityStore over chton's materialized CoordMap.
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
/// tagma-map CoordMap contract.
///
/// The record boundary is the same codec seam as the FileIo surface: the
/// value bytes are opaque to the map, so a future cipher layer would sit
/// here without changing the trait surface.
pub struct MapEntityStore<const N: usize, V> {
    inner: Cell2<CoordMapStore<N>>,
    marker: PhantomData<V>,
}

impl<const N: usize, V> MapEntityStore<N, V>
where
    V: Clone + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Create a fresh store over `origin`. The origin must be empty;
    /// `load` opens an existing store.
    pub fn new(origin: Box<dyn Origin>, record_slot_size: u64) -> Self {
        const {
            assert!(
                N >= 2,
                "depth must be at least 2: the marker axis and one payload axis"
            );
            assert!(
                N <= MAX_STORE_DEPTH,
                "depth exceeds the compile-time capacity bound"
            );
        }
        Self {
            inner: Cell2::new(CoordMapStore::new(origin, record_slot_size)),
            marker: PhantomData,
        }
    }

    /// Open a store over `origin`: load the header when present, otherwise
    /// create a fresh store with `default_record_slot_size`.
    pub fn load(origin: Box<dyn Origin>, default_record_slot_size: u64) -> Result<Self, String> {
        let map =
            CoordMapStore::load(origin, default_record_slot_size).map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Cell2::new(map),
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
        let Ok(path) = str_to_coordpath::<N>(key) else {
            return None;
        };
        let result = {
            let map = self.inner.borrow();
            map.get_path(&path)
        };
        match result {
            Ok(value) => value.map(decode_value),
            Err(e) => panic!("map entity store get failed: {e}"),
        }
    }

    pub(crate) async fn insert_entry(&self, key: String, value: V) -> Option<V> {
        let path = map_key_or_panic::<N>(&key);
        let bytes = encode_value(&value);
        let result = {
            let mut map = self.inner.borrow_mut();
            map.put_path(&path, &bytes)
        };
        match result {
            Ok(prev) => prev.map(decode_value),
            Err(e) => panic!("map entity store insert failed: {e}"),
        }
    }

    pub(crate) async fn remove_entry(&self, key: &str) -> Option<V> {
        let Ok(path) = str_to_coordpath::<N>(key) else {
            return None;
        };
        let result = {
            let mut map = self.inner.borrow_mut();
            map.remove_path(&path)
        };
        match result {
            Ok(prev) => prev.map(decode_value),
            Err(e) => panic!("map entity store remove failed: {e}"),
        }
    }

    pub(crate) async fn contains_entry(&self, key: &str) -> bool {
        let Ok(path) = str_to_coordpath::<N>(key) else {
            return false;
        };
        let result = {
            let map = self.inner.borrow();
            map.get_path(&path)
        };
        match result {
            Ok(value) => value.is_some(),
            Err(e) => panic!("map entity store get failed: {e}"),
        }
    }

    pub(crate) async fn entry_count(&self) -> usize {
        self.inner.borrow().len()
    }

    pub(crate) async fn all_values(&self) -> Vec<V> {
        let result = {
            let map = self.inner.borrow();
            map.iter()
        };
        let entries = match result {
            Ok(entries) => entries,
            Err(e) => panic!("map entity store iter failed: {e}"),
        };
        entries
            .into_iter()
            .map(|(_, value)| decode_value(value))
            .collect()
    }

    pub(crate) async fn clear_entries(&self) {
        let result = {
            let mut map = self.inner.borrow_mut();
            map.clear_checked()
        };
        if let Err(e) = result {
            panic!("map entity store clear failed: {e}");
        }
    }

    pub(crate) async fn replace_entries(&self, entries: Vec<(String, V)>) {
        // Encode before borrowing so a codec panic never holds the guard.
        let encoded: Vec<(CoordPath<N>, Vec<u8>)> = entries
            .into_iter()
            .map(|(key, value)| (map_key_or_panic::<N>(&key), encode_value(&value)))
            .collect();
        let result = {
            let mut map = self.inner.borrow_mut();
            (|| -> Result<(), String> {
                map.clear_checked().map_err(|e| e.to_string())?;
                for (path, bytes) in encoded {
                    map.put_path(&path, &bytes).map_err(|e| e.to_string())?;
                }
                Ok(())
            })()
        };
        if let Err(e) = result {
            panic!("map entity store replace failed: {e}");
        }
    }

    /// Proximity query over the materialized store: entries within
    /// L-infinity `radius` of `center`, decoded to `V`. The center is
    /// interpreted as a `CoordCube<D, R>`; `D * R` must equal `N`.
    ///
    /// Panics on IO errors after releasing the interior borrow, the
    /// same contract as the rest of the surface.
    pub fn proximity<const D: usize, const R: usize>(
        &self,
        center: &CoordPath<N>,
        radius: usize,
    ) -> Vec<(CoordPath<N>, V)> {
        use tagma_core::CoordCube;
        use tagma_geo::spatial::SpatialOps;
        let cube = CoordCube::<N, D, R>::from_path(*center);
        let mut results = Vec::new();
        for path in cube.proximity(radius) {
            let result = {
                let map = self.inner.borrow();
                map.get_path(&path)
            };
            match result {
                Ok(Some(bytes)) => results.push((path, decode_value(bytes))),
                Ok(None) => {}
                Err(e) => panic!("map entity store proximity get failed: {e}"),
            }
        }
        results
    }
}

/// Decode a stored value, panicking at the trait boundary. The
/// `EntityStore` surface is infallible, so IO and codec errors panic with
/// a descriptive message, mirroring the tagma-map CoordMap contract.
fn decode_value<V>(bytes: Vec<u8>) -> V
where
    V: serde::de::DeserializeOwned,
{
    postcard::from_bytes(&bytes).unwrap_or_else(|e| panic!("map entity store decode failed: {e}"))
}

/// Encode a value for storage.
fn encode_value<V>(value: &V) -> Vec<u8>
where
    V: serde::Serialize,
{
    postcard::to_allocvec(value).unwrap_or_else(|e| panic!("map entity store encode failed: {e}"))
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[async_trait]
impl<const N: usize, V> EntityStore<V> for MapEntityStore<N, V>
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
impl<const N: usize, V> EntityStore<V> for MapEntityStore<N, V>
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
