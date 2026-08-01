use chton::binding::{SpaceStrategy, TreeStrategy};
use chton::origin::{MemoryOrigin, Origin};
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
