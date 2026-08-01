//! Binding layer: coordinate to origin adaptation.
//!
//! The binding maps a coordinate path to an origin offset. The first context
//! uses fixed-depth mixed-radix packing:
//!
//! `offset = packed_index(path) * record_len`
//!
//! where `packed_index` folds the coordinate indices in mixed radix. This is
//! the fixed-depth form of the space; a recursive form follows the same
//! surface in a later context.

use crate::origin::{Origin, OriginError};
use tagma_core::{Coord, CoordPath};

/// A coordinate-addressed region over an origin.
#[derive(Debug)]
pub struct CoordRegion<O: Origin> {
    origin: O,
    record_len: usize,
}

impl<O: Origin> CoordRegion<O> {
    /// Bind an origin into a coordinate region with fixed-size records.
    pub fn bind(origin: O, record_len: usize) -> Self {
        Self { origin, record_len }
    }

    /// Packed mixed-radix offset of a coordinate path.
    pub fn offset_of<const N: usize>(&self, key: &CoordPath<N>) -> u64 {
        let mut index: u64 = 0;
        for coord in key.iter() {
            index = index * Coord::N_VALID as u64 + coord.index() as u64;
        }
        index * self.record_len as u64
    }

    /// The byte length of one record slot.
    pub fn record_len(&self) -> usize {
        self.record_len
    }

    /// The underlying origin.
    pub fn origin(&self) -> &O {
        &self.origin
    }

    /// The underlying origin, mutably.
    pub fn origin_mut(&mut self) -> &mut O {
        &mut self.origin
    }
}

/// Read a record at the key's offset into `buf`.
pub fn read_record<O: Origin, const N: usize>(
    region: &CoordRegion<O>,
    key: &CoordPath<N>,
    buf: &mut [u8],
) -> Result<usize, OriginError> {
    region.origin().read(region.offset_of(key), buf)
}

/// Write `data` at the key's offset.
pub fn write_record<O: Origin, const N: usize>(
    region: &mut CoordRegion<O>,
    key: &CoordPath<N>,
    data: &[u8],
) -> Result<(), OriginError> {
    let offset = region.offset_of(key);
    region.origin_mut().write(offset, data)
}
