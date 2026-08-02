//! Mapped-file origin (unix): an external medium projected into the
//! address space, the mapped binding of the same layout.
//!
//! The file is memory-mapped: reads and writes are direct memory
//! accesses, and `flush` persists the mapping to the medium. The mapping
//! always covers the full file length; a write past the end extends the
//! file and recreates the mapping. An empty file is not mapped until the
//! first write, so `len()` starts at zero like the other origins.

use std::fs::File;
use std::path::Path;

use memmap2::MmapMut;

use super::{AddressMode, Binding, Capabilities, Direction, Origin, OriginError, Persistence};

/// A durable byte region backed by a memory-mapped file.
///
/// This is the mapped binding: the external medium is projected into the
/// address space, so the binding is `Mapped` rather than `Copied`. The
/// layout materialized over it is the same as over `FileOrigin`; only the
/// binding differs.
#[derive(Debug)]
pub struct MappedFileOrigin {
    file: File,
    map: Option<MmapMut>,
    len: u64,
}

impl MappedFileOrigin {
    /// Open (or create) the file and map it into the address space when
    /// it holds bytes. An empty file is mapped lazily on the first write.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, OriginError> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let len = file.metadata()?.len();
        let map = if len > 0 {
            Some(unsafe { MmapMut::map_mut(&file)? })
        } else {
            None
        };
        Ok(Self { file, map, len })
    }

    /// The mapped bytes, for inspection. Empty while the file is empty.
    pub fn as_slice(&self) -> &[u8] {
        self.map.as_deref().unwrap_or(&[])
    }
}

impl Origin for MappedFileOrigin {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            address_mode: AddressMode::Byte,
            direction: Direction::Duplex,
            persistence: Persistence::Durable,
            binding: Binding::Mapped,
        }
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, OriginError> {
        let start = offset as usize;
        if start >= self.len as usize {
            // EOF semantics, matching the other origins: reading at or
            // beyond the length returns zero bytes.
            return Ok(0);
        }
        let map = self.map.as_ref().expect("map present when len > 0");
        let n = (self.len as usize - start).min(buf.len());
        buf[..n].copy_from_slice(&map[start..start + n]);
        Ok(n)
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), OriginError> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(OriginError::OutOfBounds {
                offset,
                len: u64::MAX,
            })?;
        if end > self.len {
            // Extend the file first, then remap. If the remap fails, the
            // old mapping and length are left intact, so the origin stays
            // consistent for reads within the previous length.
            self.file.set_len(end)?;
            self.map = Some(unsafe { MmapMut::map_mut(&self.file)? });
            self.len = end;
        } else if self.map.is_none() {
            // The file is empty and the write is empty too.
            return Ok(());
        }
        let map = self.map.as_mut().expect("map present when len > 0");
        let start = offset as usize;
        map[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), OriginError> {
        if let Some(map) = self.map.as_ref() {
            map.flush()?;
        }
        self.file.sync_all()?;
        Ok(())
    }
}
