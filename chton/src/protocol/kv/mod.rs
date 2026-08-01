//! Key-value protocol: the first materialization protocol.
//!
//! The surface is a familiar key-value interface; the implementation is
//! native over the coordinate space. Keys are coordinate paths, values are
//! opaque bytes. The key-value surface is the on-ramp; the coordinate
//! structure is the engine.

use std::error::Error;
use std::fmt;

use crate::binding::CoordRegion;
use crate::origin::{Origin, OriginError};
use tagma_core::CoordPath;

/// Errors from key-value operations.
#[derive(Debug)]
pub enum KvError {
    Origin(OriginError),
    ValueTooLarge { value_len: usize, max_len: usize },
}

impl fmt::Display for KvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KvError::Origin(e) => write!(f, "kv origin error: {e}"),
            KvError::ValueTooLarge { value_len, max_len } => {
                write!(
                    f,
                    "kv value too large: {value_len} bytes, maximum {max_len}"
                )
            }
        }
    }
}

impl Error for KvError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            KvError::Origin(e) => Some(e),
            _ => None,
        }
    }
}

impl From<OriginError> for KvError {
    fn from(e: OriginError) -> Self {
        KvError::Origin(e)
    }
}

/// Key-value protocol surface.
pub trait KvStore<const N: usize> {
    /// Read the value at `key`. `None` means the key is absent.
    fn get(&self, key: &CoordPath<N>) -> Result<Option<Vec<u8>>, KvError>;

    /// Write `value` at `key`, replacing any prior value.
    fn put(&mut self, key: &CoordPath<N>, value: &[u8]) -> Result<(), KvError>;

    /// Remove the value at `key`. Removing an absent key is a no-op.
    fn remove(&mut self, key: &CoordPath<N>) -> Result<(), KvError>;
}

/// Native key-value implementation over a coordinate region.
///
/// Each slot holds a length-prefixed value at the key's packed offset. A zero
/// length header means the key is absent. The record size is
/// `8 + max_value_len` bytes per slot.
#[derive(Debug)]
pub struct RegionKv<O: Origin> {
    region: CoordRegion<O>,
    max_value_len: usize,
}

impl<O: Origin> RegionKv<O> {
    /// Bind `origin` into a region-backed key-value store.
    pub fn bind(origin: O, max_value_len: usize) -> Self {
        let region = CoordRegion::bind(origin, 8 + max_value_len);
        Self {
            region,
            max_value_len,
        }
    }

    /// The byte length of one slot.
    pub fn slot_len(&self) -> usize {
        8 + self.max_value_len
    }

    /// The underlying region.
    pub fn region(&self) -> &CoordRegion<O> {
        &self.region
    }

    /// The underlying region, mutably.
    pub fn region_mut(&mut self) -> &mut CoordRegion<O> {
        &mut self.region
    }
}

impl<O: Origin, const N: usize> KvStore<N> for RegionKv<O> {
    fn get(&self, key: &CoordPath<N>) -> Result<Option<Vec<u8>>, KvError> {
        let offset = self.region.offset_of(key);
        let mut header = [0u8; 8];
        let n = self.region.origin().read(offset, &mut header)?;
        if n < 8 {
            return Ok(None);
        }
        let len = u64::from_le_bytes(header) as usize;
        if len == 0 {
            return Ok(None);
        }
        let mut value = vec![0u8; len];
        let m = self.region.origin().read(offset + 8, &mut value)?;
        if m < len {
            return Ok(None);
        }
        Ok(Some(value))
    }

    fn put(&mut self, key: &CoordPath<N>, value: &[u8]) -> Result<(), KvError> {
        if value.len() > self.max_value_len {
            return Err(KvError::ValueTooLarge {
                value_len: value.len(),
                max_len: self.max_value_len,
            });
        }
        let offset = self.region.offset_of(key);
        self.region
            .origin_mut()
            .write(offset, &(value.len() as u64).to_le_bytes())?;
        if !value.is_empty() {
            self.region.origin_mut().write(offset + 8, value)?;
        }
        Ok(())
    }

    fn remove(&mut self, key: &CoordPath<N>) -> Result<(), KvError> {
        let offset = self.region.offset_of(key);
        self.region.origin_mut().write(offset, &[0u8; 8])?;
        Ok(())
    }
}
