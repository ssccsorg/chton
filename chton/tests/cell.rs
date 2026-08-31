// Cell2 contract tests (issue #181 no_std layering).
//
// Cell2 is the interior-mutability primitive for the store surface. On
// native/WASIX and no_std targets it is a critical-section Mutex<RefCell>;
// on wasm32-unknown-unknown it is a plain RefCell. These tests pin the
// observable contract that the refactor must preserve:
//
//  1. value round-trip through borrow/borrow_mut,
//  2. nested borrow of the same cell panics (RefCell reentrancy check),
//  3. cross-thread access serializes (native critical-section path),
//  4. the two cell types are independent: holding a borrow on one cell
//     does not block a borrow on another.
//
// The wasm32-unknown-unknown path is compiled by the workspace wasm check;
// these tests run on native and exercise the critical-section backend.

use chhton::cell::Cell2;
use std::sync::Arc;

#[test]
fn value_round_trip() {
    let c = Cell2::new(7u64);
    assert_eq!(*c.borrow(), 7);
    *c.borrow_mut() = 42;
    assert_eq!(*c.borrow(), 42);
}

#[test]
#[should_panic(expected = "already borrowed")]
fn same_cell_nested_borrow_panics() {
    let c = Cell2::new(1u64);
    let _guard = c.borrow();
    // The RefCell reentrancy check fires: a second borrow of the same cell
    // in the same thread panics, matching the wasm RefCell path.
    let _second = c.borrow();
}

#[test]
fn independent_cells_do_not_nest() {
    // Two distinct cells must be borrowable at the same time. Under the
    // critical-section backend each acquire is sequential and balanced, so
    // holding one guard does not block acquiring the other.
    let a = Cell2::new(1u64);
    let b = Cell2::new(2u64);
    let ga = a.borrow();
    let gb = b.borrow();
    assert_eq!(*ga + *gb, 3);
    drop(ga);
    drop(gb);
    *a.borrow_mut() = 10;
    assert_eq!(*b.borrow(), 2);
}

#[test]
fn cross_thread_access_serializes() {
    // Two threads write to the same cell via borrow_mut. The critical-section
    // backend serializes the critical sections, so the final value is one of
    // the writes and the increment count is exact.
    let c = Arc::new(Cell2::new(0u64));
    let c1 = Arc::clone(&c);
    let c2 = Arc::clone(&c);

    let t1 = std::thread::spawn(move || {
        for _ in 0..1000 {
            let mut g = c1.borrow_mut();
            *g += 1;
        }
    });
    let t2 = std::thread::spawn(move || {
        for _ in 0..1000 {
            let mut g = c2.borrow_mut();
            *g += 1;
        }
    });
    t1.join().unwrap();
    t2.join().unwrap();
    assert_eq!(*c.borrow(), 2000);
}

#[test]
fn guard_scope_releases_cell() {
    // A borrow must be releasable before the next one is taken.
    let c = Cell2::new(5u64);
    {
        let g = c.borrow();
        assert_eq!(*g, 5);
    }
    // After the guard drops, the cell is borrowable again.
    *c.borrow_mut() = 6;
    assert_eq!(*c.borrow(), 6);
}
