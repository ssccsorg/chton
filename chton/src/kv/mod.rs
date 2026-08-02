//! Materialized key-value surface: the tagma-kv CoordKV contract over a
//! chton origin.
//!
//! The interface (CoordKV, CoordKVKey) is owned by tagma-kv; chton
//! provides the materialization backend. Keys are coordinate paths into a
//! `TreeStrategy<N>` tree, values are opaque bytes in fixed-size record
//! slots as `[u64 length][payload]`. The layout is the storage format:
//! there is no separate serialization step.
//!
//! The record boundary is the seam for a future codec layer: a cipher
//! over the payload before write and after read would sit here without
//! changing the trait surface or the slot layout.

use std::error::Error;
use std::fmt;

use crate::binding::{BindingError, SpaceStrategy, TreeStrategy};
use crate::origin::{Origin, OriginError};
use tagma_core::CoordPath;
use tagma_kv::coord_cube_kv::CoordCubeKV;
use tagma_kv::coord_gen::CoordKey;
use tagma_kv::{CoordKV, CoordKVKey};

/// Byte size of the record length prefix.
const LENGTH_BYTES: u64 = 8;

/// Errors from materialized key-value operations.
#[derive(Debug)]
pub enum KvError {
    Origin(OriginError),
    Binding(BindingError),
    ValueTooLarge { value_len: usize, max_len: usize },
    CorruptRecord { offset: u64, reason: &'static str },
}

impl fmt::Display for KvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KvError::Origin(e) => write!(f, "kv origin error: {e}"),
            KvError::Binding(e) => write!(f, "kv binding error: {e}"),
            KvError::ValueTooLarge { value_len, max_len } => {
                write!(
                    f,
                    "kv value too large: {value_len} bytes, maximum {max_len}"
                )
            }
            KvError::CorruptRecord { offset, reason } => {
                write!(f, "corrupt kv record at {offset}: {reason}")
            }
        }
    }
}

impl Error for KvError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            KvError::Origin(e) => Some(e),
            KvError::Binding(e) => Some(e),
            _ => None,
        }
    }
}

impl From<OriginError> for KvError {
    fn from(e: OriginError) -> Self {
        KvError::Origin(e)
    }
}

impl From<BindingError> for KvError {
    fn from(e: BindingError) -> Self {
        KvError::Binding(e)
    }
}

/// Materialized key-value backend over a chton origin.
///
/// Implements the tagma-kv [`CoordKV`] and [`CoordKVKey`] contract over
/// [`TreeStrategy`], the CoordSpace persistence backend. The tagma-kv
/// contract is infallible, so IO and corruption errors panic with a
/// descriptive message at the trait boundary; the `_path` methods and
/// lifecycle operations return typed errors.
///
/// The record slot size is fixed at creation. Values are bounded by
/// `record_slot_size - 8`; a larger value is rejected. A reopened store
/// adopts the recorded slot size and restores the entry count by walking
/// the tree.
pub struct MaterialKv<const N: usize> {
    strategy: TreeStrategy<N>,
    origin: Box<dyn Origin>,
    len: usize,
}

impl<const N: usize> fmt::Debug for MaterialKv<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterialKv")
            .field("len", &self.len)
            .field("record_slot_size", &self.record_slot_size())
            .finish_non_exhaustive()
    }
}

impl<const N: usize> MaterialKv<N> {
    /// Create a fresh store over `origin`. The origin must be empty;
    /// `load` opens an existing store.
    pub fn new(origin: Box<dyn Origin>, record_slot_size: u64) -> Self {
        Self {
            strategy: TreeStrategy::new(record_slot_size),
            origin,
            len: 0,
        }
    }

    /// Open a store over `origin`: load the header when present, otherwise
    /// create a fresh store with `default_record_slot_size`.
    pub fn load(origin: Box<dyn Origin>, default_record_slot_size: u64) -> Result<Self, KvError> {
        let strategy = TreeStrategy::<N>::load_or_new(&*origin, default_record_slot_size)?;
        let len = strategy.count_records(&*origin)? as usize;
        Ok(Self {
            strategy,
            origin,
            len,
        })
    }

    /// Persist strategy state and flush the origin to the medium.
    pub fn flush(&mut self) -> Result<(), KvError> {
        self.strategy.flush(&mut *self.origin)?;
        self.origin.flush()?;
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

    /// Read the value at `path`. `None` means the key is absent.
    pub fn get_path(&self, path: &CoordPath<N>) -> Result<Option<Vec<u8>>, KvError> {
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
    ) -> Result<Option<Vec<u8>>, KvError> {
        if value.len() > self.max_value_len() {
            return Err(KvError::ValueTooLarge {
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
            self.strategy
                .write_leaf(&mut *self.origin, slot.leaf_slot_offset, record)?;
            self.len += 1;
            record
        };
        self.origin
            .write(record, &(value.len() as u64).to_le_bytes())?;
        if !value.is_empty() {
            self.origin.write(record + LENGTH_BYTES, value)?;
        }
        Ok(prev)
    }

    /// Remove the value at `path`. Removing an absent key is a no-op.
    /// Returns the previous value.
    pub fn remove_path(&mut self, path: &CoordPath<N>) -> Result<Option<Vec<u8>>, KvError> {
        let slot = self.strategy.locate(&*self.origin, path)?;
        if slot.record_offset == 0 {
            return Ok(None);
        }
        let prev = self.read_record(slot.record_offset)?;
        self.strategy
            .free_record(&mut *self.origin, slot.record_offset)?;
        self.strategy
            .write_leaf(&mut *self.origin, slot.leaf_slot_offset, 0)?;
        self.len = self.len.saturating_sub(1);
        Ok(Some(prev))
    }

    /// Read the value at a record offset. A short read, a length prefix
    /// beyond the record slot, or a short payload is corruption, not
    /// absence.
    fn read_record(&self, offset: u64) -> Result<Vec<u8>, KvError> {
        let mut header = [0u8; LENGTH_BYTES as usize];
        let n = self.origin.read(offset, &mut header)?;
        if n < LENGTH_BYTES as usize {
            return Err(KvError::CorruptRecord {
                offset,
                reason: "short record header",
            });
        }
        let len = u64::from_le_bytes(header) as usize;
        if len > self.max_value_len() {
            return Err(KvError::CorruptRecord {
                offset,
                reason: "length prefix exceeds record slot",
            });
        }
        let mut value = vec![0u8; len];
        let m = self.origin.read(offset + LENGTH_BYTES, &mut value)?;
        if m < len {
            return Err(KvError::CorruptRecord {
                offset,
                reason: "short record payload",
            });
        }
        Ok(value)
    }
}

impl<const N: usize> CoordKV for MaterialKv<N> {
    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.strategy
            .reset(&mut *self.origin)
            .unwrap_or_else(|e| panic!("kv clear: reset failed: {e}"));
        self.len = 0;
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

impl<const N: usize> CoordKVKey<N> for MaterialKv<N> {
    fn insert_by_coordkey(&mut self, key: &CoordKey<N>, value: Vec<u8>) -> Option<Vec<u8>> {
        let path = key.to_coord_path();
        self.put_path(&path, &value)
            .unwrap_or_else(|e| panic!("kv insert failed: {e}"))
    }

    fn get_by_coordkey(&self, key: &CoordKey<N>) -> Option<Vec<u8>> {
        let path = key.to_coord_path();
        self.get_path(&path)
            .unwrap_or_else(|e| panic!("kv get failed: {e}"))
    }

    fn remove_by_coordkey(&mut self, key: &CoordKey<N>) -> Option<Vec<u8>> {
        let path = key.to_coord_path();
        self.remove_path(&path)
            .unwrap_or_else(|e| panic!("kv remove failed: {e}"))
    }
}

impl<const N: usize> CoordCubeKV<N> for MaterialKv<N> {
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
                .unwrap_or_else(|e| panic!("kv proximity failed: {e}"))
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
                .unwrap_or_else(|e| panic!("kv bounding box failed: {e}"))
            {
                results.push((path, val));
            }
        }
        results
    }
}
