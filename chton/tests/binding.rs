use chton::binding::{CoordRegion, read_record, write_record};
use chton::origin::MemoryOrigin;
use tagma_core::{Coord, CoordPath};

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

#[test]
fn offset_is_mixed_radix() {
    let region = CoordRegion::bind(MemoryOrigin::new(), 4);
    let one = CoordPath::new([coord(1)]);
    let two = CoordPath::new([coord(2)]);
    let deep = CoordPath::new([coord(1), coord(2)]);

    assert_eq!(region.offset_of(&one), 4);
    assert_eq!(region.offset_of(&two), 8);
    assert_eq!(region.offset_of(&deep), (Coord::N_VALID as u64 + 2) * 4);
}

#[test]
fn record_round_trip() {
    let mut region = CoordRegion::bind(MemoryOrigin::new(), 16);
    let key = CoordPath::new([coord(7)]);
    write_record(&mut region, &key, b"payload").unwrap();

    let mut buf = [0u8; 16];
    let n = read_record(&region, &key, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"payload");
    assert_eq!(region.record_len(), 16);
}
