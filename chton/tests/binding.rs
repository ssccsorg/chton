use chton::binding::{BindingError, SpaceStrategy, TreeStrategy};
use chton::origin::{FileOrigin, MemoryOrigin, Origin};
use tagma_core::{Coord, CoordPath};

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

#[test]
fn depth_six_path_resolves_without_overflow() {
    // The flat mixed-radix packing overflowed u64 at depth 5 and above.
    // The tree strategy addresses per level, so depth 6 is bounded by file
    // size, never by integer width.
    let mut origin = MemoryOrigin::new();
    let mut strategy = TreeStrategy::<6>::new(16);
    let key = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(6)]);

    let slot = strategy.locate(&origin, &key).unwrap();
    assert_eq!(slot.record_offset, 0);

    let slot = strategy.locate_or_create(&mut origin, &key).unwrap();
    assert_eq!(slot.record_offset, 0);
    assert!(slot.leaf_slot_offset > 0);

    // A sibling key under the same prefix must resolve to a distinct slot.
    let other = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(7)]);
    let slot_other = strategy.locate(&origin, &other).unwrap();
    assert_eq!(slot_other.record_offset, 0);
    assert_ne!(slot_other.leaf_slot_offset, slot.leaf_slot_offset);
}

#[test]
fn locate_is_read_only() {
    let origin = MemoryOrigin::new();
    let strategy = TreeStrategy::<2>::new(16);
    let key = CoordPath::new([coord(9), coord(9)]);

    let slot = strategy.locate(&origin, &key).unwrap();
    assert_eq!(slot.record_offset, 0);
    assert_eq!(origin.len(), 0);
}

#[test]
fn write_leaf_and_free_list_reuse() {
    let mut origin = MemoryOrigin::new();
    let mut strategy = TreeStrategy::<1>::new(16);
    let key = CoordPath::new([coord(7)]);

    let slot = strategy.locate_or_create(&mut origin, &key).unwrap();
    let rec = strategy.alloc_record(&mut origin).unwrap();
    strategy
        .write_leaf(&mut origin, slot.leaf_slot_offset, rec)
        .unwrap();

    let got = strategy.locate(&origin, &key).unwrap();
    assert_eq!(got.record_offset, rec);

    strategy.free_record(&mut origin, rec).unwrap();
    let rec2 = strategy.alloc_record(&mut origin).unwrap();
    assert_eq!(rec2, rec);
}

#[test]
#[should_panic(expected = "below the 8-byte record header")]
fn record_slot_size_below_header_panics() {
    let _ = TreeStrategy::<1>::new(7);
}

#[test]
fn depth_mismatch_on_reopen_is_corrupt() {
    let path = std::env::temp_dir().join(format!("chton-binding-depth-{}.bin", std::process::id()));
    {
        let mut origin = FileOrigin::open(&path).unwrap();
        let mut strategy = TreeStrategy::<2>::new(16);
        strategy.flush(&mut origin).unwrap();
    }
    {
        let origin = FileOrigin::open(&path).unwrap();
        // Reopening the depth-2 file at depth 1 must fail loudly, not
        // misread leaf slots as records.
        let strategy = TreeStrategy::<1>::load_or_new(&origin, 16);
        assert!(matches!(strategy, Err(BindingError::Corrupt { .. })));
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn record_slot_size_mismatch_on_reopen_is_corrupt() {
    let path = std::env::temp_dir().join(format!("chton-binding-slot-{}.bin", std::process::id()));
    {
        let mut origin = FileOrigin::open(&path).unwrap();
        let mut strategy = TreeStrategy::<2>::new(16);
        strategy.flush(&mut origin).unwrap();
    }
    {
        let origin = FileOrigin::open(&path).unwrap();
        // Reopening the file at a different record slot size would
        // misalign bump and free list reads; the header records the size.
        let strategy = TreeStrategy::<2>::load_or_new(&origin, 32);
        assert!(matches!(strategy, Err(BindingError::Corrupt { .. })));
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn truncated_slot_inside_allocated_region_is_corrupt() {
    let mut origin = MemoryOrigin::new();
    let mut strategy = TreeStrategy::<1>::new(16);
    let key = CoordPath::new([coord(3)]);
    let slot = strategy.locate_or_create(&mut origin, &key).unwrap();
    let record = strategy.alloc_record(&mut origin).unwrap();
    origin.write(record, &[0xAB; 8]).unwrap();
    strategy
        .write_leaf(&mut origin, slot.leaf_slot_offset, record)
        .unwrap();

    // Truncate mid-slot: the region was allocated, so a partial read
    // there is corruption, not absence.
    let truncated =
        MemoryOrigin::with_bytes(origin.as_slice()[..slot.leaf_slot_offset as usize + 4].to_vec());
    let err = strategy.locate(&truncated, &key).unwrap_err();
    assert!(matches!(err, BindingError::Corrupt { .. }));
}

#[test]
fn truncation_to_slot_boundary_reads_as_absent() {
    let mut origin = MemoryOrigin::new();
    let mut strategy = TreeStrategy::<1>::new(16);
    let key = CoordPath::new([coord(3)]);
    let slot = strategy.locate_or_create(&mut origin, &key).unwrap();
    let record = strategy.alloc_record(&mut origin).unwrap();
    origin.write(record, &[0xAB; 8]).unwrap();
    strategy
        .write_leaf(&mut origin, slot.leaf_slot_offset, record)
        .unwrap();

    // Truncating exactly to the slot start removes the slot itself: the
    // read is absence, the boundary between absent and corrupt.
    let truncated =
        MemoryOrigin::with_bytes(origin.as_slice()[..slot.leaf_slot_offset as usize].to_vec());
    let got = strategy.locate(&truncated, &key).unwrap();
    assert_eq!(got.record_offset, 0);
}

#[test]
fn child_pointer_beyond_file_is_corrupt() {
    let mut origin = MemoryOrigin::new();
    let mut strategy = TreeStrategy::<2>::new(16);
    let key = CoordPath::new([coord(1), coord(2)]);
    let slot = strategy.locate_or_create(&mut origin, &key).unwrap();
    assert!(slot.leaf_slot_offset > 0);

    // Truncate the origin to the root span: the leaf node is gone but
    // the root slot still points at it. Following the pointer must fail
    // loudly, not read the missing region as absence.
    let root_span = 64 + 8 * Coord::N_VALID as u64;
    let truncated = MemoryOrigin::with_bytes(origin.as_slice()[..root_span as usize].to_vec());
    let err = strategy.locate(&truncated, &key).unwrap_err();
    assert!(matches!(err, BindingError::Corrupt { .. }));
}
