//! Key-value protocol: the first materialization protocol.
//!
//! The surface is a familiar key-value interface; the implementation is
//! native over the coordinate space. Keys are coordinate paths, values are
//! opaque bytes. The key-value surface is the on-ramp; the coordinate
//! structure is the engine. The value format is a length prefix followed by
//! raw bytes: the layout is the storage format, no serialization step.

use std::error::Error;
use std::fmt;

use crate::binding::{BindingError, SpaceStrategy};
use crate::origin::{Origin, OriginError};
use tagma_core::CoordPath;

/// Errors from key-value operations.
#[derive(Debug)]
pub enum KvError {
    Origin(OriginError),
    Binding(BindingError),
    ValueTooLarge { value_len: usize, max_len: usize },
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
/// The strategy resolves each key to a record slot; records are
/// length-prefixed values stored in slots allocated by the strategy.
#[derive(Debug)]
pub struct RegionKv<O: Origin, S: SpaceStrategy<N>, const N: usize> {
    origin: O,
    strategy: S,
    max_value_len: usize,
}

impl<O: Origin, S: SpaceStrategy<N>, const N: usize> RegionKv<O, S, N> {
    /// Bind an origin and a space strategy into a region-backed key-value
    /// store.
    pub fn bind(origin: O, strategy: S, max_value_len: usize) -> Self {
        Self {
            origin,
            strategy,
            max_value_len,
        }
    }

    /// The underlying origin.
    pub fn origin(&self) -> &O {
        &self.origin
    }

    /// The underlying origin, mutably.
    pub fn origin_mut(&mut self) -> &mut O {
        &mut self.origin
    }

    /// The underlying strategy, mutably.
    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }

    /// Persist strategy state and flush the origin.
    pub fn flush(&mut self) -> Result<(), KvError> {
        self.strategy.flush(&mut self.origin)?;
        self.origin.flush()?;
        Ok(())
    }

    fn read_record(&self, offset: u64) -> Result<Option<Vec<u8>>, KvError> {
        let mut header = [0u8; 8];
        let n = self.origin.read(offset, &mut header)?;
        if n < 8 {
            return Ok(None);
        }
        let len = u64::from_le_bytes(header) as usize;
        if len == 0 {
            return Ok(None);
        }
        let mut value = vec![0u8; len];
        let m = self.origin.read(offset + 8, &mut value)?;
        if m < len {
            return Ok(None);
        }
        Ok(Some(value))
    }
}

impl<O: Origin, S: SpaceStrategy<N>, const N: usize> KvStore<N> for RegionKv<O, S, N> {
    fn get(&self, key: &CoordPath<N>) -> Result<Option<Vec<u8>>, KvError> {
        let slot = self.strategy.locate(&self.origin, key)?;
        if slot.record_offset == 0 {
            return Ok(None);
        }
        self.read_record(slot.record_offset)
    }

    fn put(&mut self, key: &CoordPath<N>, value: &[u8]) -> Result<(), KvError> {
        if value.len() > self.max_value_len {
            return Err(KvError::ValueTooLarge {
                value_len: value.len(),
                max_len: self.max_value_len,
            });
        }
        let slot = self.strategy.locate_or_create(&mut self.origin, key)?;
        let record = if slot.record_offset != 0 {
            slot.record_offset
        } else {
            let record = self.strategy.alloc_record(&mut self.origin)?;
            self.strategy
                .write_leaf(&mut self.origin, slot.leaf_slot_offset, record)?;
            record
        };
        let header = (value.len() as u64).to_le_bytes();
        self.origin.write(record, &header)?;
        if !value.is_empty() {
            self.origin.write(record + 8, value)?;
        }
        Ok(())
    }

    fn remove(&mut self, key: &CoordPath<N>) -> Result<(), KvError> {
        let slot = self.strategy.locate(&self.origin, key)?;
        if slot.record_offset != 0 {
            self.strategy
                .free_record(&mut self.origin, slot.record_offset)?;
            self.strategy
                .write_leaf(&mut self.origin, slot.leaf_slot_offset, 0)?;
        }
        Ok(())
    }
}
