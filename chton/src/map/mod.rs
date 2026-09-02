//! Materialized key-value surface: the tagma-map CoordMap contract over a
//! chton origin.
//!
//! The interface (CoordMap, CoordMapKey) is owned by tagma-map; chton
//! provides the materialization backend. Keys are coordinate paths into a
//! `TreeStrategy<N>` tree, values are opaque bytes in fixed-size record
//! slots as `[u64 length][payload]`. The layout is the storage format:
//! there is no separate serialization step.
//!
//! The record boundary is the seam for a future codec layer: a cipher
//! over the payload before write and after read would sit here without
//! changing the trait surface or the slot layout.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;
use core::error::Error;
use core::fmt;

use crate::binding::{BindingError, SpaceStrategy, TreeStrategy};
use crate::origin::{Origin, OriginError};
use tagma_core::CoordPath;
use tagma_map::coord_cube_map::CoordCubeMap;
use tagma_map::coord_gen::CoordKey;
use tagma_map::{CoordMap, CoordMapKey};

/// Byte size of the record length prefix.
const LENGTH_BYTES: u64 = 8;

/// Errors from materialized key-value operations.
#[derive(Debug)]
pub enum MapError {
    Origin(OriginError),
    Binding(BindingError),
    ValueTooLarge { value_len: usize, max_len: usize },
    CorruptRecord { offset: u64, reason: &'static str },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Origin(e) => write!(f, "map origin error: {e}"),
            MapError::Binding(e) => write!(f, "map binding error: {e}"),
            MapError::ValueTooLarge { value_len, max_len } => {
                write!(
                    f,
                    "map value too large: {value_len} bytes, maximum {max_len}"
                )
            }
            MapError::CorruptRecord { offset, reason } => {
                write!(f, "corrupt map record at {offset}: {reason}")
            }
        }
    }
}

impl Error for MapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MapError::Origin(e) => Some(e),
            MapError::Binding(e) => Some(e),
            _ => None,
        }
    }
}

impl From<OriginError> for MapError {
    fn from(e: OriginError) -> Self {
        MapError::Origin(e)
    }
}

impl From<BindingError> for MapError {
    fn from(e: BindingError) -> Self {
        MapError::Binding(e)
    }
}

/// Materialized key-value backend over a chton origin.
///
/// Implements the tagma-map [`CoordMap`] and [`CoordMapKey`] contract over
/// [`TreeStrategy`], the CoordSpace persistence backend. The tagma-map
/// contract is infallible, so IO and corruption errors panic with a
/// descriptive message at the trait boundary; the `_path` methods and
/// lifecycle operations return typed errors.
///
/// The record slot size is fixed at creation. Values are bounded by
/// `record_slot_size - 8`; a larger value is rejected. A reopened store
/// adopts the recorded slot size and restores the entry count by walking
/// the tree.
pub struct CoordMapStore<const N: usize> {
    strategy: TreeStrategy<N>,
    origin: Box<dyn Origin>,
    len: usize,
    /// True when the strategy header is not yet written to the origin.
    dirty: Cell<bool>,
}

impl<const N: usize> fmt::Debug for CoordMapStore<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoordMapStore")
            .field("len", &self.len)
            .field("record_slot_size", &self.record_slot_size())
            .finish_non_exhaustive()
    }
}

impl<const N: usize> CoordMapStore<N> {
    /// Create a fresh store over `origin`. The origin must be empty;
    /// `load` opens an existing store.
    pub fn new(origin: Box<dyn Origin>, record_slot_size: u64) -> Self {
        Self {
            strategy: TreeStrategy::new(record_slot_size),
            origin,
            len: 0,
            dirty: Cell::new(false),
        }
    }

    /// Open a store over `origin`: load the header when present, otherwise
    /// create a fresh store with `default_record_slot_size`.
    pub fn load(origin: Box<dyn Origin>, default_record_slot_size: u64) -> Result<Self, MapError> {
        let strategy = TreeStrategy::<N>::load_or_new(&*origin, default_record_slot_size)?;
        // The record count is persisted in the header, so a reopen reads
        // it directly instead of walking the whole tree (O(nodes x
        // fan-out) for the sparse 11172-wide layout).
        let len = strategy.record_count() as usize;
        Ok(Self {
            strategy,
            origin,
            len,
            dirty: Cell::new(false),
        })
    }

    /// Whether the strategy header holds buffered state that is not yet
    /// durable on the origin.
    pub fn is_buffered(&self) -> bool {
        self.dirty.get()
    }

    /// Persist strategy state and flush the origin to the medium.
    pub fn flush(&mut self) -> Result<(), MapError> {
        self.strategy.set_record_count(self.len as u64);
        self.strategy.flush(&mut *self.origin)?;
        self.origin.flush()?;
        self.dirty.set(false);
        Ok(())
    }

    /// The byte size of one record slot.
    pub fn record_slot_size(&self) -> u64 {
        self.strategy.record_slot_size()
    }

    /// The largest value that fits one record slot.
    pub fn max_value_len(&self) -> usize {
        self.record_slot_size() as usize - LENGTH_BYTES as usize
    }

    /// Iterate all entries in ascending coordinate order: `(key, value)`.
    pub fn iter(&self) -> Result<Vec<(CoordKey<N>, Vec<u8>)>, MapError> {
        let entries = self.strategy.iter(&*self.origin)?;
        let mut out = Vec::with_capacity(entries.len());
        for (path, offset) in entries {
            let value = self.read_record(offset)?;
            out.push((CoordKey::from_coord_path(&path), value));
        }
        Ok(out)
    }

    /// Read the value at `path`. `None` means the key is absent.
    pub fn get_path(&self, path: &CoordPath<N>) -> Result<Option<Vec<u8>>, MapError> {
        let slot = self.strategy.locate(&*self.origin, path)?;
        if slot.record_offset == 0 {
            return Ok(None);
        }
        Ok(Some(self.read_record(slot.record_offset)?))
    }

    /// Write `value` at `path`, replacing any prior value. Returns the
    /// previous value.
    pub fn put_path(
        &mut self,
        path: &CoordPath<N>,
        value: &[u8],
    ) -> Result<Option<Vec<u8>>, MapError> {
        if value.len() > self.max_value_len() {
            return Err(MapError::ValueTooLarge {
                value_len: value.len(),
                max_len: self.max_value_len(),
            });
        }
        let slot = self.strategy.locate_or_create(&mut *self.origin, path)?;
        let prev = if slot.record_offset != 0 {
            Some(self.read_record(slot.record_offset)?)
        } else {
            None
        };
        // In-place overwrite is safe: the value-length check bounds the
        // payload to the fixed record slot.
        let record = if slot.record_offset != 0 {
            slot.record_offset
        } else {
            let record = self.strategy.alloc_record(&mut *self.origin)?;
            self.strategy.write_leaf(&mut *self.origin, &slot, record)?;
            self.len += 1;
            record
        };
        self.origin
            .write(record, &(value.len() as u64).to_le_bytes())?;
        if !value.is_empty() {
            self.origin.write(record + LENGTH_BYTES, value)?;
        }
        self.dirty.set(true);
        Ok(prev)
    }

    /// Remove the value at `path`. Removing an absent key is a no-op.
    /// Returns the previous value.
    pub fn remove_path(&mut self, path: &CoordPath<N>) -> Result<Option<Vec<u8>>, MapError> {
        let slot = self.strategy.locate(&*self.origin, path)?;
        if slot.record_offset == 0 {
            return Ok(None);
        }
        let prev = self.read_record(slot.record_offset)?;
        self.strategy
            .free_record(&mut *self.origin, slot.record_offset)?;
        self.strategy.write_leaf(&mut *self.origin, &slot, 0)?;
        self.len = self.len.saturating_sub(1);
        self.dirty.set(true);
        Ok(Some(prev))
    }

    /// Clear all entries, returning the error instead of panicking.
    /// The infallible [`CoordMap::clear`] delegates here.
    pub fn clear_checked(&mut self) -> Result<(), MapError> {
        self.strategy.reset(&mut *self.origin)?;
        self.len = 0;
        self.dirty.set(false);
        Ok(())
    }

    /// Read the value at a record offset. A short read, a length prefix
    /// beyond the record slot, or a short payload is corruption, not
    /// absence.
    fn read_record(&self, offset: u64) -> Result<Vec<u8>, MapError> {
        let mut header = [0u8; LENGTH_BYTES as usize];
        let n = self.origin.read(offset, &mut header)?;
        if n < LENGTH_BYTES as usize {
            return Err(MapError::CorruptRecord {
                offset,
                reason: "short record header",
            });
        }
        let len = u64::from_le_bytes(header) as usize;
        if len > self.max_value_len() {
            return Err(MapError::CorruptRecord {
                offset,
                reason: "length prefix exceeds record slot",
            });
        }
        let mut value = vec![0u8; len];
        let m = self.origin.read(offset + LENGTH_BYTES, &mut value)?;
        if m < len {
            return Err(MapError::CorruptRecord {
                offset,
                reason: "short record payload",
            });
        }
        Ok(value)
    }
}

impl<const N: usize> CoordMap for CoordMapStore<N> {
    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.clear_checked()
            .unwrap_or_else(|e| panic!("map clear: reset failed: {e}"));
    }

    fn insert(&mut self, key: &str, value: Vec<u8>) -> Option<Vec<u8>> {
        let ck: CoordKey<N> = key.into();
        self.insert_by_coordkey(&ck, value)
    }

    fn get(&self, key: &str) -> Option<Vec<u8>> {
        if key.len() != N {
            return None;
        }
        let ck: CoordKey<N> = key.into();
        self.get_by_coordkey(&ck)
    }

    fn remove(&mut self, key: &str) -> Option<Vec<u8>> {
        if key.len() != N {
            return None;
        }
        let ck: CoordKey<N> = key.into();
        self.remove_by_coordkey(&ck)
    }
}

impl<const N: usize> CoordMapKey<N> for CoordMapStore<N> {
    fn insert_by_coordkey(&mut self, key: &CoordKey<N>, value: Vec<u8>) -> Option<Vec<u8>> {
        let path = key.to_coord_path();
        self.put_path(&path, &value)
            .unwrap_or_else(|e| panic!("map insert failed: {e}"))
    }

    fn get_by_coordkey(&self, key: &CoordKey<N>) -> Option<Vec<u8>> {
        let path = key.to_coord_path();
        self.get_path(&path)
            .unwrap_or_else(|e| panic!("map get failed: {e}"))
    }

    fn remove_by_coordkey(&mut self, key: &CoordKey<N>) -> Option<Vec<u8>> {
        let path = key.to_coord_path();
        self.remove_path(&path)
            .unwrap_or_else(|e| panic!("map remove failed: {e}"))
    }
}

impl<const N: usize> CoordCubeMap<N> for CoordMapStore<N> {
    fn proximity<const D: usize, const R: usize>(
        &self,
        center: &CoordPath<N>,
        radius: usize,
    ) -> Vec<(CoordPath<N>, Vec<u8>)> {
        use tagma_core::CoordCube;
        use tagma_geo::spatial::SpatialOps;
        let cube = CoordCube::<N, D, R>::from_path(*center);
        let capacity = (2 * radius + 1).pow(N as u32);
        let mut results = Vec::with_capacity(capacity);
        for path in cube.proximity(radius) {
            if let Some(val) = self
                .get_path(&path)
                .unwrap_or_else(|e| panic!("map proximity failed: {e}"))
            {
                results.push((path, val));
            }
        }
        results
    }

    fn bounding_box_range(&self, ranges: &[(u16, u16); N]) -> Vec<(CoordPath<N>, Vec<u8>)> {
        use tagma_geo::BoundingBoxIter;
        let iter = BoundingBoxIter::<N>::new(*ranges);
        let capacity = iter.count_paths();
        let mut results = Vec::with_capacity(capacity);
        for path in iter {
            if let Some(val) = self
                .get_path(&path)
                .unwrap_or_else(|e| panic!("map bounding box failed: {e}"))
            {
                results.push((path, val));
            }
        }
        results
    }
}
