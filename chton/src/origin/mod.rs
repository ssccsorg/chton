//! Origin layer: byte-level bindings, protocol-agnostic.
//!
//! An origin is a destination for materialization. The origin trait exposes
//! only the byte-level surface shared by every medium; it observes no
//! protocol semantics. The capability matrix distinguishes address mode,
//! direction, persistence, and binding, so that one surface accepts memory,
//! file, signal, network, and GPU origins.

use core::error::Error;
use core::fmt;

#[cfg(feature = "std")]
mod file;
#[cfg(all(unix, feature = "std"))]
mod mapped_file;
mod memory;

#[cfg(feature = "std")]
pub use file::FileOrigin;
#[cfg(all(unix, feature = "std"))]
pub use mapped_file::MappedFileOrigin;
pub use memory::MemoryOrigin;

/// Address mode of an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressMode {
    /// Byte-addressable, positional access.
    Byte,
    /// Block-addressable, fixed-size access units.
    Block,
    /// Sequential stream access.
    Stream,
}

/// Direction capability of an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
    Duplex,
}

/// Persistence of an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    Durable,
    Volatile,
    Transient,
}

/// Binding mechanism of an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Mapped,
    Copied,
    ZeroCopy,
}

/// Static capabilities of an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub address_mode: AddressMode,
    pub direction: Direction,
    pub persistence: Persistence,
    pub binding: Binding,
}

/// Errors from origin operations.
#[derive(Debug)]
pub enum OriginError {
    /// The underlying host IO operation failed. Present only when the
    /// `std` feature is enabled (file-backed origins).
    #[cfg(feature = "std")]
    Io(std::io::Error),
    OutOfBounds {
        offset: u64,
        len: u64,
    },
    Unsupported,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "std")]
            OriginError::Io(e) => write!(f, "origin io error: {e}"),
            OriginError::OutOfBounds { offset, len } => {
                write!(f, "origin access out of bounds: offset {offset}, len {len}")
            }
            OriginError::Unsupported => write!(f, "origin does not support the operation"),
        }
    }
}

impl Error for OriginError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            #[cfg(feature = "std")]
            OriginError::Io(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(feature = "std")]
impl From<std::io::Error> for OriginError {
    fn from(e: std::io::Error) -> Self {
        OriginError::Io(e)
    }
}

/// A byte-level binding to a physical destination.
///
/// Origins are protocol-agnostic: a protocol observes only this surface, and
/// an origin observes no protocol semantics. The trait is `Send + Sync` so
/// origins can be shared across async and threaded boundaries (for example
/// behind a mutex in an IO adapter).
pub trait Origin: Send + Sync {
    /// Static capabilities of this origin.
    fn capabilities(&self) -> Capabilities;

    /// Size of the origin in bytes.
    fn len(&self) -> u64;

    /// True when the origin holds no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read up to `buf.len()` bytes at `offset`.
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, OriginError>;

    /// Write `data` at `offset`, extending the origin when needed.
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), OriginError>;

    /// Flush buffered state to the underlying medium.
    fn flush(&mut self) -> Result<(), OriginError>;
}
