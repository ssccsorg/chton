//! Binding layer: coordinate to origin adaptation.
//!
//! The space structure (tagma) is materialized over an origin (chton) by a
//! per-type strategy. Different tagma space types have different structures,
//! so each type gets its own strategy; the first strategy is the fixed-depth
//! tree, which is the CoordSpaceN layout materialized over an origin:
//!
//! - branch node: 11,172 x u64 child offsets, 0 means absent
//! - leaf node: 11,172 x u64 record offsets, 0 means absent
//! - nodes and records are allocated from a single bump area; freed records
//!   are recycled through a free list
//!
//! File offset 0 is the absent sentinel and is never a valid node or record.
//! The layout is the storage format: there is no separate serialization step.
//!
//! The binding surface is the Lego block the router stacks: a strategy is
//! object-safe and boxable per depth N, so the materialization matrix can
//! hold heterogeneous space types as per-N trait objects over any origin.
//! The surface assumes a single writer: bump and free list state live in
//! the strategy, so one strategy must own the origin at a time.

use std::error::Error;
use std::fmt;

use crate::origin::{Origin, OriginError};
use tagma_core::{Coord, CoordPath};

/// Sentinel for an absent slot: offset 0 is never a valid node or record.
const ABSENT: u64 = 0;
/// Bytes per slot value.
const SLOT_BYTES: u64 = 8;
/// File header length.
const HEADER_LEN: u64 = 64;
/// Root node offset, right after the header.
const ROOT_OFFSET: u64 = HEADER_LEN;
/// Header magic.
const MAGIC: u32 = 0x4348_544F; // "CHTO"

/// Errors from binding operations.
#[derive(Debug)]
pub enum BindingError {
    Origin(OriginError),
    Corrupt { node: u64, slot: u64 },
    KeyTooLong { path_len: usize },
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingError::Origin(e) => write!(f, "binding origin error: {e}"),
            BindingError::Corrupt { node, slot } => {
                write!(f, "corrupt binding structure: node {node}, slot {slot}")
            }
            BindingError::KeyTooLong { path_len } => {
                write!(f, "key depth {path_len} exceeds strategy depth")
            }
        }
    }
}

impl Error for BindingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BindingError::Origin(e) => Some(e),
            _ => None,
        }
    }
}

impl From<OriginError> for BindingError {
    fn from(e: OriginError) -> Self {
        BindingError::Origin(e)
    }
}

/// A resolved slot for a coordinate path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// Position of the leaf slot that holds the record offset.
    pub leaf_slot_offset: u64,
    /// Record offset, 0 when absent.
    pub record_offset: u64,
}

/// Per-space-type materialization strategy over an origin.
///
/// The strategy defines how a space structure is laid out over the origin
/// and how a coordinate path resolves to a record slot. Record management
/// (allocation, free list, slot size) is backend scope: it is the storage
/// backend's record surface, independent of any protocol, so any protocol
/// consumes the same backend without inheriting protocol code. The trait
/// is object-safe; the router holds strategies as per-N trait objects.
pub trait SpaceStrategy<const N: usize> {
    /// Read-only resolve: locate the slot for a coordinate path without
    /// allocating. `record_offset` is 0 when absent.
    fn locate(&self, origin: &dyn Origin, key: &CoordPath<N>) -> Result<Slot, BindingError>;

    /// Write-side resolve: locate the slot, creating missing nodes.
    /// `record_offset` is 0 when no record exists yet.
    fn locate_or_create(
        &mut self,
        origin: &mut dyn Origin,
        key: &CoordPath<N>,
    ) -> Result<Slot, BindingError>;

    /// Write the record offset into a leaf slot.
    fn write_leaf(
        &mut self,
        origin: &mut dyn Origin,
        leaf_slot_offset: u64,
        record_offset: u64,
    ) -> Result<(), BindingError>;

    /// Allocate a record slot from the free list or the bump area.
    fn alloc_record(&mut self, origin: &mut dyn Origin) -> Result<u64, BindingError>;

    /// Return a record slot to the free list.
    fn free_record(&mut self, origin: &mut dyn Origin, offset: u64) -> Result<(), BindingError>;

    /// The byte size of one record slot.
    fn record_slot_size(&self) -> u64;

    /// Persist strategy state (header) to the origin. This writes layout
    /// state; call `Origin::flush` to push buffered bytes to the medium.
    /// The caller composes the two flushes.
    fn flush(&mut self, origin: &mut dyn Origin) -> Result<(), BindingError>;

    /// Reset the strategy to a fresh state: the root span is zeroed, the
    /// bump area restarts after the root, and the free list is emptied.
    /// Existing data becomes unreachable; the origin is reused in place.
    fn reset(&mut self, origin: &mut dyn Origin) -> Result<(), BindingError>;
}

/// Fixed-depth tree strategy over an origin.
///
/// `TreeStrategy<N>` materializes the CoordSpaceN layout: an N-level tree of
/// 11,172-wide nodes. Addressing is per-level array indexing, so depth is
/// bounded by file size, never by integer width.
#[derive(Debug)]
pub struct TreeStrategy<const N: usize> {
    record_slot_size: u64,
    bump: u64,
    free_head: u64,
    node_size: u64,
}

impl<const N: usize> TreeStrategy<N> {
    /// Create a strategy with a fresh header state. The root node span is
    /// reserved: all allocations append after it, so nodes and records can
    /// never overlap.
    ///
    /// `record_slot_size` must be at least 8 bytes: every record carries
    /// an 8-byte length prefix. A smaller slot makes the format
    /// unrepresentable.
    pub fn new(record_slot_size: u64) -> Self {
        assert!(
            record_slot_size >= SLOT_BYTES,
            "TreeStrategy: record_slot_size {record_slot_size} is below the 8-byte record header"
        );
        let node_size = SLOT_BYTES * Coord::N_VALID as u64;
        Self {
            record_slot_size,
            bump: ROOT_OFFSET + node_size,
            free_head: ABSENT,
            node_size,
        }
    }

    /// Load the header from the origin when present, otherwise return a
    /// fresh strategy. `default_record_slot_size` applies only to fresh
    /// files; an existing file supplies its own recorded slot size.
    pub fn load_or_new(
        origin: &dyn Origin,
        default_record_slot_size: u64,
    ) -> Result<Self, BindingError> {
        let mut strategy = Self::new(default_record_slot_size);
        strategy.load(origin)?;
        Ok(strategy)
    }

    /// Load the header from the origin when present, otherwise keep the
    /// fresh state.
    pub fn load(&mut self, origin: &dyn Origin) -> Result<(), BindingError> {
        if origin.len() < HEADER_LEN {
            return Ok(());
        }
        let mut header = [0u8; HEADER_LEN as usize];
        origin.read(0, &mut header)?;
        let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
        if magic != MAGIC {
            return Ok(());
        }
        // The format records the tree depth, node size, and record slot
        // size so a file written at one shape is not silently misread at
        // another.
        let depth = u16::from_le_bytes(header[4..6].try_into().unwrap());
        let node_size = u64::from_le_bytes(header[24..32].try_into().unwrap());
        let record_slot_size = u64::from_le_bytes(header[32..40].try_into().unwrap());
        if depth as usize != N || node_size != self.node_size {
            return Err(BindingError::Corrupt { node: 0, slot: 0 });
        }
        // The recorded slot size is adopted: the file is self-describing,
        // so reopen needs no caller-supplied size. A size below the record
        // header is corruption.
        if record_slot_size < SLOT_BYTES {
            return Err(BindingError::Corrupt { node: 0, slot: 0 });
        }
        self.record_slot_size = record_slot_size;
        self.bump = u64::from_le_bytes(header[8..16].try_into().unwrap());
        self.free_head = u64::from_le_bytes(header[16..24].try_into().unwrap());
        // The bump pointer must stay past the root span; a header that
        // points into the header or root is corrupt.
        if self.bump < ROOT_OFFSET + self.node_size {
            return Err(BindingError::Corrupt { node: 0, slot: 0 });
        }
        // A free list head points at a record written by free_record, so
        // a head at or beyond the file length was never written or the
        // file was truncated.
        if self.free_head != ABSENT && self.free_head >= origin.len() {
            return Err(BindingError::Corrupt { node: 0, slot: 0 });
        }
        Ok(())
    }

    /// Count materialized records: the number of leaf slots holding a
    /// non-absent record offset. Walks the whole tree; used to restore
    /// the entry count when a store is reopened.
    pub fn count_records(&self, origin: &dyn Origin) -> Result<u64, BindingError> {
        self.count_node(origin, ROOT_OFFSET, 0)
    }

    fn count_node(
        &self,
        origin: &dyn Origin,
        node: u64,
        depth: usize,
    ) -> Result<u64, BindingError> {
        let mut count = 0u64;
        for index in 0..Coord::N_VALID as u64 {
            let pos = node + index * SLOT_BYTES;
            let value = Self::read_slot(origin, pos)?;
            if value == ABSENT {
                continue;
            }
            if depth + 1 == N {
                count += 1;
            } else {
                Self::check_child_in_bounds(origin, value)?;
                count += self.count_node(origin, value, depth + 1)?;
            }
        }
        Ok(count)
    }

    fn write_header(&self, origin: &mut dyn Origin) -> Result<(), BindingError> {
        let mut header = [0u8; HEADER_LEN as usize];
        header[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        header[4..6].copy_from_slice(&(N as u16).to_le_bytes());
        header[8..16].copy_from_slice(&self.bump.to_le_bytes());
        header[16..24].copy_from_slice(&self.free_head.to_le_bytes());
        header[24..32].copy_from_slice(&self.node_size.to_le_bytes());
        header[32..40].copy_from_slice(&self.record_slot_size.to_le_bytes());
        origin.write(0, &header)?;
        Ok(())
    }

    fn alloc_node(&mut self, origin: &mut dyn Origin) -> Result<u64, BindingError> {
        let node = self.bump;
        self.bump = self
            .bump
            .checked_add(self.node_size)
            .ok_or(BindingError::Origin(OriginError::OutOfBounds {
                offset: self.bump,
                len: u64::MAX,
            }))?;
        // Zero the node area so absent slots read as 0.
        origin.write(node, &vec![0u8; self.node_size as usize])?;
        Ok(node)
    }

    fn read_slot(origin: &dyn Origin, pos: u64) -> Result<u64, BindingError> {
        let mut buf = [0u8; SLOT_BYTES as usize];
        // A slot position at or beyond the origin length is absence: the
        // node was never allocated. A partial read inside an allocated
        // region is corruption.
        if pos >= origin.len() {
            return Ok(ABSENT);
        }
        let n = origin.read(pos, &mut buf)?;
        if n < SLOT_BYTES as usize {
            return Err(BindingError::Corrupt { node: pos, slot: 0 });
        }
        Ok(u64::from_le_bytes(buf))
    }

    /// Verify a child node pointer lies within the origin. A valid file
    /// allocates each node before writing its pointer, so a pointer that
    /// reaches past the file is corruption, not absence.
    fn check_child_in_bounds(origin: &dyn Origin, child: u64) -> Result<(), BindingError> {
        let end = child
            .checked_add(SLOT_BYTES * Coord::N_VALID as u64)
            .ok_or(BindingError::Corrupt {
                node: child,
                slot: 0,
            })?;
        if end > origin.len() {
            return Err(BindingError::Corrupt {
                node: child,
                slot: 0,
            });
        }
        Ok(())
    }

    fn write_slot(origin: &mut dyn Origin, pos: u64, value: u64) -> Result<(), BindingError> {
        origin.write(pos, &value.to_le_bytes())?;
        Ok(())
    }
}

impl<const N: usize> SpaceStrategy<N> for TreeStrategy<N> {
    fn locate(&self, origin: &dyn Origin, key: &CoordPath<N>) -> Result<Slot, BindingError> {
        let mut node = ROOT_OFFSET;
        for depth in 0..N {
            let coord = match key.get(depth) {
                Some(c) => c,
                None => {
                    return Err(BindingError::KeyTooLong {
                        path_len: key.iter().count(),
                    });
                }
            };
            let slot_pos = node + coord.index() as u64 * SLOT_BYTES;
            let value = Self::read_slot(origin, slot_pos)?;
            if depth == N - 1 {
                return Ok(Slot {
                    leaf_slot_offset: slot_pos,
                    record_offset: value,
                });
            }
            if value == ABSENT {
                return Ok(Slot {
                    leaf_slot_offset: slot_pos,
                    record_offset: ABSENT,
                });
            }
            Self::check_child_in_bounds(origin, value)?;
            node = value;
        }
        Err(BindingError::KeyTooLong {
            path_len: key.iter().count(),
        })
    }

    fn locate_or_create(
        &mut self,
        origin: &mut dyn Origin,
        key: &CoordPath<N>,
    ) -> Result<Slot, BindingError> {
        let mut node = ROOT_OFFSET;
        for depth in 0..N {
            let coord = match key.get(depth) {
                Some(c) => c,
                None => {
                    return Err(BindingError::KeyTooLong {
                        path_len: key.iter().count(),
                    });
                }
            };
            let slot_pos = node + coord.index() as u64 * SLOT_BYTES;
            let value = Self::read_slot(origin, slot_pos)?;

            if depth == N - 1 {
                return Ok(Slot {
                    leaf_slot_offset: slot_pos,
                    record_offset: value,
                });
            }

            if value == ABSENT {
                let child = self.alloc_node(origin)?;
                Self::write_slot(origin, slot_pos, child)?;
                node = child;
            } else {
                Self::check_child_in_bounds(origin, value)?;
                node = value;
            }
        }
        Err(BindingError::KeyTooLong {
            path_len: key.iter().count(),
        })
    }

    fn write_leaf(
        &mut self,
        origin: &mut dyn Origin,
        leaf_slot_offset: u64,
        record_offset: u64,
    ) -> Result<(), BindingError> {
        Self::write_slot(origin, leaf_slot_offset, record_offset)
    }

    fn alloc_record(&mut self, origin: &mut dyn Origin) -> Result<u64, BindingError> {
        if self.free_head != ABSENT {
            // A free list head points at a record written by free_record,
            // so a head at or beyond the file length is corruption.
            if self.free_head >= origin.len() {
                return Err(BindingError::Corrupt {
                    node: self.free_head,
                    slot: 0,
                });
            }
            let record = self.free_head;
            self.free_head = Self::read_slot(origin, record)?;
            return Ok(record);
        }
        let record = self.bump;
        self.bump = self
            .bump
            .checked_add(self.record_slot_size)
            .ok_or(BindingError::Origin(OriginError::OutOfBounds {
                offset: self.bump,
                len: u64::MAX,
            }))?;
        Ok(record)
    }

    fn free_record(&mut self, origin: &mut dyn Origin, offset: u64) -> Result<(), BindingError> {
        Self::write_slot(origin, offset, self.free_head)?;
        self.free_head = offset;
        Ok(())
    }

    fn record_slot_size(&self) -> u64 {
        self.record_slot_size
    }

    fn flush(&mut self, origin: &mut dyn Origin) -> Result<(), BindingError> {
        self.write_header(origin)?;
        Ok(())
    }

    fn reset(&mut self, origin: &mut dyn Origin) -> Result<(), BindingError> {
        self.bump = ROOT_OFFSET + self.node_size;
        self.free_head = ABSENT;
        origin.write(ROOT_OFFSET, &vec![0u8; self.node_size as usize])?;
        self.write_header(origin)?;
        Ok(())
    }
}
