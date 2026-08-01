//! In-memory origin.

use super::{
    AddressMode, Binding, Capabilities, Direction, Origin, OriginError, Persistence,
};

/// A volatile byte region backed by a `Vec<u8>`.
#[derive(Debug, Clone, Default)]
pub struct MemoryOrigin {
    data: Vec<u8>,
}

impl MemoryOrigin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

impl Origin for MemoryOrigin {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            address_mode: AddressMode::Byte,
            direction: Direction::Duplex,
            persistence: Persistence::Volatile,
            binding: Binding::Copied,
        }
    }

    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, OriginError> {
        let start = offset as usize;
        if start > self.data.len() {
            return Err(OriginError::OutOfBounds {
                offset,
                len: self.data.len() as u64,
            });
        }
        let available = self.data.len() - start;
        let n = available.min(buf.len());
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), OriginError> {
        let start = offset as usize;
        let end = start
            .checked_add(data.len())
            .ok_or(OriginError::OutOfBounds {
                offset,
                len: u64::MAX,
            })?;
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), OriginError> {
        Ok(())
    }
}
