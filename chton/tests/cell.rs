// Cell2 contract tests (issue #181 no_std layering).
//
// Cell2 is the interior-mutability primitive for the store surface. On
// native/WASIX and no_std targets it is a critical-section Mutex<RefCell>;
// on wasm32-unknown-unknown it is a plain RefCell. These tests pin the
// observable contract that the refactor must preserve:
//
//  1. value round-trip through borrow/borrow_mut,
//  2. same-thread nested shared borrow is reentrant (RefCell semantics),
//  3. shared-then-exclusive borrow of the same cell panics,
//  4. cross-thread access serializes (native critical-section path),
//  5. two distinct cells are independent: holding a borrow on one cell
//     does not block a borrow on another.
//
// The wasm32-unknown-unknown path is compiled by the workspace wasm check;
// these tests run on native and exercise the critical-section backend.

use chton::cell::Cell2;
use std::sync::Arc;

#[test]
fn value_round_trip() {
    let c = Cell2::new(7u64);
    assert_eq!(*c.borrow(), 7);
    *c.borrow_mut() = 42;
    assert_eq!(*c.borrow(), 42);
}

#[test]
fn same_cell_shared_borrow_is_reentrant() {
    let c = Cell2::new(1u64);
    let g1 = c.borrow();
    // The critical-section backend allows a nested shared borrow in the
    // same thread (unlike std::sync::Mutex, which would deadlock). This is
    // the RefCell semantics preserved by Mutex<RefCell>.
    let g2 = c.borrow();
    assert_eq!(*g1 + *g2, 2);
    drop(g1);
    drop(g2);
}

#[test]
#[should_panic(expected = "already borrowed")]
fn shared_then_exclusive_borrow_panics() {
    let c = Cell2::new(1u64);
    let _guard = c.borrow();
    // RefCell reentrancy check fires: a shared borrow held while requesting
    // an exclusive borrow panics.
    let _second = c.borrow_mut();
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
