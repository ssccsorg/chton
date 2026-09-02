// no_std anchors for the chton storage path (issue #181).
//
// This integration test runs against the `--no-default-features` build of
// the library: the library is compiled without std, so any std-only type
// that leaks past its feature gates fails to compile here. The test body
// itself deliberately uses no std APIs, mirroring what an MCU consumer
// can call. Run with:
//
//   cargo test --no-default-features --test no_std_anchors
//
// The std-only host backends (FsIo, FileOrigin, MappedFileOrigin) are
// exercised by the regular (std) test suite in tests/io.rs and
// tests/origin.rs; they are not reachable from this file by design.

// The no_std build disables critical-section's std implementation, so the
// native test binary needs an explicit implementation to link. This is a
// no-op (single-threaded test) placeholder; a real MCU provides the same
// symbols from the firmware/HAL.
use critical_section::RawRestoreState;

struct TestCriticalSection;
critical_section::set_impl!(TestCriticalSection);

unsafe impl critical_section::Impl for TestCriticalSection {
    unsafe fn acquire() -> RawRestoreState {
        false
    }
    unsafe fn release(_restore_state: RawRestoreState) {}
}

use chton::cell::Cell2;
use chton::io::{FileIo, WriteOp};
use chton::origin::{
    AddressMode, Binding, Capabilities, Direction, MemoryOrigin, Origin, Persistence,
};
use chton::store::{CoordEntityStore, EntityStore, MemoryEntityStore};

// ── Cell2: interior mutability on the no_std path ──────────────────────

#[test]
fn cell2_surface_is_std_free() {
    // Construction must not require std. Borrowing is exercised by the
    // std test suite (tests/cell.rs); on a real MCU the firmware supplies
    // the critical-section implementation.
    let _cell = Cell2::<u64>::new(41);
}

// ── Origin: memory origin is the no_std materialization surface ────────

#[test]
fn memory_origin_round_trip_no_std() {
    let mut origin = MemoryOrigin::with_bytes(b"abc".to_vec());
    assert_eq!(origin.len(), 3);
    let caps = origin.capabilities();
    assert!(caps.address_mode == AddressMode::Byte);
    assert!(caps.direction == Direction::Duplex);
    assert!(caps.persistence == Persistence::Volatile);
    assert!(caps.binding == Binding::Copied);

    origin.write(0, b"xyz").unwrap();
    let mut buf = [0u8; 3];
    let n = origin.read(0, &mut buf).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf, b"xyz");
    origin.flush().unwrap();
}

// ── FileIo: the IO contract is std-free (implementations may be std) ───

#[test]
fn file_io_trait_surface_is_std_free() {
    // The FileIo trait itself must not leak std types: it is implemented
    // by in-memory, flash, and remote backends alike. A compile failure
    // here means the trait surface regressed toward std.
    fn assert_io_contract<I: FileIo>() {}
    assert_io_contract::<chton::io::CoordMapStoreIo<2>>();
}

#[test]
fn default_apply_batch_is_std_free() {
    // BatchIo is a lego trait (implemented by backends that support
    // atomic batches); the default sequential apply must work over any
    // FileIo without touching std.
    fn assert_apply<I: FileIo>(io: &I, ops: &[WriteOp]) {
        core::mem::drop(chton::io::default_apply_batch(io, ops));
    }
    let origin = Box::new(MemoryOrigin::with_bytes(b"".to_vec()));
    let map = chton::map::CoordMapStore::<2>::new(origin, 64);
    let io = chton::io::CoordMapStoreIo::<2>::new(map);
    let ops: &[WriteOp] = &[];
    assert_apply(&io, ops);
}

#[test]
fn write_op_is_std_free() {
    let op = WriteOp::Write {
        path: "k".into(),
        data: vec![1u8, 2, 3],
    };
    match op {
        WriteOp::Write { path, data } => {
            assert_eq!(path, "k");
            assert_eq!(data, vec![1u8, 2, 3]);
        }
        WriteOp::Delete { path } => assert_eq!(path, ""),
    }
}

// ── EntityStore: the record store contract is std-free ─────────────────

#[test]
fn entity_store_surface_is_std_free() {
    // The EntityStore trait is async and std-free by contract; execution
    // needs a runtime, so this anchor pins the type surface only. Runtime
    // behavior is covered by the std test suite.
    fn assert_store<V: Clone + 'static, S: EntityStore<V>>() {}
    assert_store::<u32, MemoryEntityStore<u32>>();
    assert_store::<u32, CoordEntityStore<2, u32>>();
}

// ── Capabilities: pure value type, no std dependency ───────────────────

#[test]
fn capabilities_construct_no_std() {
    let _caps = Capabilities {
        address_mode: AddressMode::Byte,
        direction: Direction::Duplex,
        persistence: Persistence::Volatile,
        binding: Binding::Copied,
    };
}

// ── Anchor documentation: the std-only surface ─────────────────────────
//
// The following types must NOT be referenced in this file. They exist
// only under the `std` feature and are exercised by tests/io.rs and
// tests/origin.rs on the host:
//
//   - chton::io::FsIo            (std::fs backend)
//   - chton::origin::FileOrigin  (std::fs file origin)
//   - chton::origin::MappedFileOrigin (mmap, unix only)
//
// If the `std` gate on any of these is accidentally removed, the
// `--no-default-features` build fails at their `std::` usage, which the
// CI job catches before merge.
