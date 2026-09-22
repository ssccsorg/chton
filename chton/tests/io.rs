// IO flow surface tests: the flat key-space FileIo contract.

use chton::io::{BufferIo, CoordMapStoreIo, Durable, FileIo, FsIo, IoFuture, ReadyIo};
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

// ── Paging a prefix's keys ────────────────────────────────────────────────
//
// What a caller that cannot hold the list reads instead. The contract is the set of keys and
// nothing about their order, so a channel that can only enumerate in the order its medium holds
// is not asked for more.

/// Every key the channel hands over for `prefix`, page by page at `max`.
async fn collect_pages<IO: FileIo>(io: &IO, prefix: &str, max: usize) -> Vec<String> {
    let mut keys = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let (page, next) = io.list_page(prefix, cursor.as_deref(), max).await.unwrap();
        keys.extend(page);
        match next {
            Some(token) => cursor = Some(token),
            None => break,
        }
    }
    keys
}

#[test]
fn a_page_carries_every_key_the_prefix_holds_once() {
    let dir = std::env::temp_dir().join(format!("chton-pages-{}", std::process::id()));
    let io = FsIo::new(&dir).unwrap();

    block_on(async {
        for name in ["a", "b", "c", "d", "e"] {
            io.write(&format!("facts/f_{name}.fact"), b"x")
                .await
                .unwrap();
        }
        io.write("hints/h_a.hint", b"x").await.unwrap();

        let mut paged = collect_pages(&io, "facts/", 2).await;
        let mut listed = io.list("facts/").await.unwrap();
        paged.sort();
        listed.sort();
        assert_eq!(paged, listed, "the pages hold the prefix's keys, each once");
        assert_eq!(paged.len(), 5, "every key arrives: {paged:?}");
    });

    std::fs::remove_dir_all(&dir).unwrap();
}

/// A channel that implements the four required operations and nothing else, so `list_page` is
/// the default.
struct PlainIo(Vec<String>);

impl FileIo for PlainIo {
    fn read<'a>(&'a self, _path: &'a str) -> IoFuture<'a, Option<Vec<u8>>> {
        Box::pin(async { Ok(None) })
    }

    fn write<'a>(&'a self, _path: &'a str, _data: &'a [u8]) -> IoFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>> {
        let keys: Vec<String> = self
            .0
            .iter()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        Box::pin(async move { Ok(keys) })
    }

    fn delete<'a>(&'a self, _path: &'a str) -> IoFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// A channel that pages on its own terms. Its cursor is an index it defined, and a reader of
/// pages does not get the list: `list` hands over nothing, so a caller that reads the pages is
/// reading the override and not the default.
struct PagedIo(Vec<String>);

impl FileIo for PagedIo {
    fn read<'a>(&'a self, _path: &'a str) -> IoFuture<'a, Option<Vec<u8>>> {
        Box::pin(async { Ok(None) })
    }

    fn write<'a>(&'a self, _path: &'a str, _data: &'a [u8]) -> IoFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn list<'a>(&'a self, _prefix: &'a str) -> IoFuture<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn delete<'a>(&'a self, _path: &'a str) -> IoFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn list_page<'a>(
        &'a self,
        prefix: &'a str,
        cursor: Option<&'a [u8]>,
        max: usize,
    ) -> IoFuture<'a, (Vec<String>, Option<Vec<u8>>)> {
        let start = match cursor {
            Some(bytes) => {
                u32::from_le_bytes(bytes.try_into().expect("a four-byte cursor")) as usize
            }
            None => 0,
        };
        let matching: Vec<String> = self
            .0
            .iter()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        let page: Vec<String> = matching.iter().skip(start).take(max).cloned().collect();
        let next = start + page.len();
        let token = (next < matching.len()).then(|| (next as u32).to_le_bytes().to_vec());
        Box::pin(async move { Ok((page, token)) })
    }
}

impl BufferIo for PagedIo {
    fn is_buffered(&self) -> bool {
        false
    }

    fn flush<'a>(&'a self) -> IoFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn paged_channel() -> PagedIo {
    PagedIo(
        ["f_a", "f_b", "f_c", "f_d"]
            .iter()
            .map(|name| format!("facts/{name}.fact"))
            .collect(),
    )
}

/// An implementor that cannot resume is not changed by this: the default hands over the list,
/// and says so rather than dropping the keys that do not fit the bound.
#[test]
fn a_channel_that_does_not_page_hands_over_the_list_at_once() {
    let io = PlainIo(vec![
        "facts/f_a.fact".to_string(),
        "facts/f_b.fact".to_string(),
        "hints/h_a.hint".to_string(),
    ]);

    block_on(async {
        let (page, next) = io.list_page("facts/", None, 1).await.unwrap();
        assert_eq!(
            page.len(),
            2,
            "the default ignores the bound rather than dropping keys it cannot return later"
        );
        assert!(next.is_none(), "the default has no way to continue");
    });
}

/// Where a page ends is the channel's, and the order across the pages is the channel's too: the
/// cursor is a token it defined, and the caller reads it back unchanged.
#[test]
fn a_channel_that_pages_decides_where_a_page_ends() {
    let io = paged_channel();
    let expected: Vec<String> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| format!("facts/f_{name}.fact"))
        .collect();

    let keys = block_on(collect_pages(&io, "facts/", 2));
    assert_eq!(keys, expected, "the pages carry every key once");

    let listed = block_on(io.list("facts/")).unwrap();
    assert!(
        listed.is_empty(),
        "the pages came from the default rather than from the override"
    );
}

/// The wrapper forwards the paging it wraps. Without this a wrapped device channel would fall
/// back to the default and hand over the list the wrapper exists to keep it from holding.
#[test]
fn the_wrapper_keeps_the_paging_it_wraps() {
    let io = Durable::new(paged_channel());
    assert_eq!(block_on(collect_pages(&io, "facts/", 2)).len(), 4);
}
