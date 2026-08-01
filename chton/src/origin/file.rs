//! File-backed origin.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use super::{AddressMode, Binding, Capabilities, Direction, Origin, OriginError, Persistence};

/// A durable byte region backed by a file.
///
/// The first context uses positional read and write; memory mapping is a
/// later binding over the same origin surface.
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
        use std::os::unix::fs::FileExt;
        Ok(self.file.read_at(buf, offset)?)
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), OriginError> {
        use std::os::unix::fs::FileExt;
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(OriginError::Unsupported)?;
        self.file.write_all_at(data, offset)?;
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
