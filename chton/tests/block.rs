// The block seam: a region that erases before it writes again.

use chton::origin::{BlockOrigin, BlockSlice, ERASED, OriginError};

#[test]
fn a_view_stays_valid_across_a_write() {
    let mut image = [0u8; 8];
    let block = BlockSlice::new(&mut image);

    let view = block.bytes();
    assert_eq!(view.len(), 8);
    block.write(2, b"ab").expect("a write");
    // The addresses did not move, so the view taken before the write describes
    // the region after it. That is the contract the seam rests on.
    assert_eq!(&view[2..4], b"ab");
}

#[test]
fn a_write_past_the_end_is_refused() {
    let mut image = [0u8; 4];
    let block = BlockSlice::new(&mut image);

    match block.write(3, b"ab") {
        Err(OriginError::OutOfBounds { offset, len }) => {
            assert_eq!((offset, len), (3, 2));
        }
        other => panic!("expected an out of bounds refusal, got {other:?}"),
    }
}

#[test]
fn an_erase_writes_the_erased_value() {
    let mut image = [0u8; 6];
    let block = BlockSlice::new(&mut image);

    block.erase(1, 4).expect("an erase");
    assert_eq!(block.bytes(), &[0, ERASED, ERASED, ERASED, 0, 0]);
}

#[test]
fn an_erase_outside_the_region_is_refused() {
    let mut image = [0u8; 4];
    let block = BlockSlice::new(&mut image);

    assert!(matches!(
        block.erase(2, 6),
        Err(OriginError::OutOfBounds { .. })
    ));
    assert!(matches!(
        block.erase(6, 2),
        Err(OriginError::OutOfBounds { .. })
    ));
}

#[test]
fn a_block_carries_the_bytes_the_caller_handed_in() {
    let mut image = [7u8; 5];
    let block = BlockSlice::new(&mut image);

    assert_eq!(block.bytes(), &[7; 5]);
}
