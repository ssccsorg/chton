//! Block seam: a medium that erases before it writes again.
//!
//! The byte seam ([`super::Origin`]) reads and writes at an offset and has no
//! erase. A part takes a write once per generation and reclaims a byte by
//! erasing the sector around it, and a driver presents the whole region as a
//! view rather than a copy. Those two properties are what this seam adds, and
//! they are why it is a second trait rather than a method on `Origin`.
//!
//! [`super::AddressMode::Block`] names this seam. A block medium is the one the
//! capability matrix already declares and the origin trait could not answer.

use core::cell::UnsafeCell;

use super::OriginError;

/// The value an erased byte holds.
pub const ERASED: u8 = 0xFF;

/// A medium addressed as one region that erases before it writes again.
///
/// # Safety
///
/// [`BlockOrigin::bytes`] returns a view that stays valid, at the same
/// addresses, until the block is dropped. A write or an erase changes what
/// those addresses hold and never where they are, so a view taken before one
/// describes the region after it. An implementation that moves its backing
/// store breaks every borrow the caller holds.
///
/// A caller also owes the one-thread contract: no read runs beside a write.
pub unsafe trait BlockOrigin {
    /// Every byte the block holds, as a view.
    fn bytes(&self) -> &[u8];

    /// Write `bytes` at `at`.
    ///
    /// A part refuses a write to a byte it has already written; a host takes it
    /// anywhere.
    fn write(&self, at: usize, bytes: &[u8]) -> Result<(), OriginError>;

    /// Erase `[from, to)`.
    ///
    /// A part erases a sector at a time, so a range that is not whole sectors is
    /// one it refuses; a host erases the range it is given.
    fn erase(&self, from: usize, to: usize) -> Result<(), OriginError>;
}

/// A block a host holds in memory.
///
/// The block borrows the slice for its life rather than owning it, so a caller
/// keeps the image it handed in and can write it to a file or dump it once the
/// block is dropped. A slice takes a write anywhere, which is what makes it a
/// host: the forward-only rule belongs to the format above it.
pub struct BlockSlice<'a> {
    region: UnsafeCell<&'a mut [u8]>,
}

impl<'a> BlockSlice<'a> {
    /// Offer a slice as the block a channel writes.
    pub fn new(region: &'a mut [u8]) -> Self {
        Self {
            region: UnsafeCell::new(region),
        }
    }
}

// Safety: the view is the slice, which does not move, and one thread at a time
// is the contract of the caller this serves.
unsafe impl BlockOrigin for BlockSlice<'_> {
    fn bytes(&self) -> &[u8] {
        // Safety: the caller holds `&self` for the length of the call, and no
        // method hands out a second reference into the region while one is
        // alive.
        unsafe { &**self.region.get() }
    }

    fn write(&self, at: usize, bytes: &[u8]) -> Result<(), OriginError> {
        // Safety: as above, and no shared reference into the region is alive
        // when a mutating call starts, because every mutating call returns
        // before the next one and none of them hands out a reference that
        // outlives it.
        let region = unsafe { &mut **self.region.get() };
        let end = at
            .checked_add(bytes.len())
            .filter(|end| *end <= region.len())
            .ok_or(OriginError::OutOfBounds {
                offset: at as u64,
                len: bytes.len() as u64,
            })?;
        region[at..end].copy_from_slice(bytes);
        Ok(())
    }

    fn erase(&self, from: usize, to: usize) -> Result<(), OriginError> {
        let region = unsafe { &mut **self.region.get() };
        let len = to.saturating_sub(from);
        let end = from
            .checked_add(len)
            .filter(|end| *end <= region.len() && *end >= from)
            .ok_or(OriginError::OutOfBounds {
                offset: from as u64,
                len: len as u64,
            })?;
        region[from..end].fill(ERASED);
        Ok(())
    }
}
