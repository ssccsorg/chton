//! File-backed origin.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::{AddressMode, Binding, Capabilities, Direction, Origin, OriginError, Persistence};

/// A durable byte region backed by a file.
///
/// Positional access is implemented with `Seek` plus `Read` and `Write`,
/// which are portable across unix and wasip2. Memory mapping is a later
/// binding over the same origin surface.
#[derive(Debug)]
pub struct FileOrigin {
    file: File,
    len: u64,
}

impl FileOrigin {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, OriginError> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl Origin for FileOrigin {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            address_mode: AddressMode::Byte,
            direction: Direction::Duplex,
            persistence: Persistence::Durable,
            binding: Binding::Copied,
        }
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, OriginError> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset))?;
        Ok(file.read(buf)?)
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), OriginError> {
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(OriginError::Unsupported)?;
        let file = &mut self.file;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(data)?;
        if end > self.len {
            self.len = end;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), OriginError> {
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(())
    }
}
