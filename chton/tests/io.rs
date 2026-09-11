// IO flow surface tests: the flat key-space FileIo contract.

use chton::io::{BufferIo, CoordMapStoreIo, Durable, FileIo, FsIo, ReadyIo};
use chton::map::CoordMapStore;
use chton::origin::MemoryOrigin;
use futures_executor::block_on;

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

#[test]
fn fs_io_round_trip() {
    let dir = std::env::temp_dir().join(format!("chton-io-{}", std::process::id()));
    let io = FsIo::new(&dir).unwrap();

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.unwrap();
        assert_eq!(
            io.read("facts/f_a.fact").await.unwrap().as_deref(),
            Some(&b"alpha"[..])
        );
        let listed = io.list("facts/").await.unwrap();
        assert_eq!(listed, vec!["facts/f_a.fact".to_string()]);

        io.delete("facts/f_a.fact").await.unwrap();
        assert!(io.read("facts/f_a.fact").await.unwrap().is_none());
    });

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn coord_map_store_io_round_trip() {
    // The CoordMap store over a memory origin, behind the flat key-space
    // FileIo surface. The store is buffered until flushed.
    let map = CoordMapStore::<16>::new(Box::new(MemoryOrigin::new()), 512);
    let io = CoordMapStoreIo::new(map);

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.unwrap();
        assert_eq!(
            io.read("facts/f_a.fact").await.unwrap().as_deref(),
            Some(&b"alpha"[..])
        );

        let listed = io.list("facts/").await.unwrap();
        assert_eq!(listed, vec!["facts/f_a.fact".to_string()]);
        // Values live in buffered slots until the store is flushed.
        assert!(io.is_buffered());
        io.flush().await.unwrap();
        assert!(!io.is_buffered());

        io.delete("facts/f_a.fact").await.unwrap();
        assert!(io.read("facts/f_a.fact").await.unwrap().is_none());
    });
}

#[test]
fn coord_map_store_io_rejects_over_capacity_paths() {
    // Depth 16 holds paths of 1..=15 bytes; a longer path is rejected
    // instead of being hashed (injective length-prefix contract).
    let map = CoordMapStore::<16>::new(Box::new(MemoryOrigin::new()), 512);
    let io = CoordMapStoreIo::new(map);
    let long = "x".repeat(16);
    block_on(async {
        let err = io.write(&long, b"data").await.unwrap_err();
        assert!(err.contains("exceeds"), "got: {err}");
        assert!(io.read(&long).await.unwrap_err().contains("exceeds"));
        assert!(io.delete(&long).await.unwrap_err().contains("exceeds"));
    });
}

// ── Readiness and durability ──────────────────────────────────────────────
//
// What a channel declares about waiting, and what the wrapper over a buffered
// channel guarantees about the medium.

/// The outcome of one poll of a future, which is what a consumer without an
/// executor does.
fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    let mut future = pin!(future);
    future
        .as_mut()
        .poll(&mut Context::from_waker(&noop_waker()))
}

fn noop_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    const RAW: RawWaker = RawWaker::new(std::ptr::null(), &VTABLE);

    fn clone(_: *const ()) -> RawWaker {
        RAW
    }
    fn noop(_: *const ()) {}

    // Safety: the vtable carries no state and every operation is a no-op, which
    // is the contract Waker requires.
    unsafe { Waker::from_raw(RAW) }
}

fn memory_channel() -> CoordMapStoreIo<16> {
    CoordMapStoreIo::new(CoordMapStore::<16>::new(Box::new(MemoryOrigin::new()), 512))
}

#[test]
fn every_operation_of_a_declared_channel_is_ready_on_its_first_poll() {
    let io = memory_channel();

    assert!(poll_once(io.write("facts/f_a.fact", b"alpha")).is_ready());
    assert!(poll_once(io.read("facts/f_a.fact")).is_ready());
    assert!(poll_once(io.list("facts/")).is_ready());
    assert!(poll_once(io.flush()).is_ready());
    assert!(poll_once(io.delete("facts/f_a.fact")).is_ready());
}

#[test]
fn a_buffered_channel_holds_a_write_until_something_flushes_it() {
    // Why the wrapper below exists: a write into the store is in the store, and
    // the origin does not have it until a flush.
    let io = memory_channel();

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.unwrap();
        assert!(
            io.is_buffered(),
            "the store claimed to be durable after a write"
        );
    });
}

#[test]
fn the_durable_wrapper_makes_a_write_durable_when_it_returns() {
    let io = Durable::new(memory_channel());

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.unwrap();
        assert!(
            !io.is_buffered(),
            "the wrapper returned with state still buffered"
        );
        assert_eq!(
            io.read("facts/f_a.fact").await.unwrap().as_deref(),
            Some(&b"alpha"[..])
        );
    });
}

#[test]
fn the_wrapper_inherits_the_readiness_declaration() {
    // A compile-time fact, held so that a wrapper that started waiting, or a
    // channel that stopped declaring, fails here rather than in a device build.
    fn requires_a_ready_channel<IO: ReadyIo>() {}
    requires_a_ready_channel::<Durable<CoordMapStoreIo<16>>>();
}
