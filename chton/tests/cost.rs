// What a caller's operation costs the channel: calls in, calls out, and bytes.
//
// chton defines the efficiency contract, so the cost of an operation is a
// property this crate states. The numbers here are the current ones, and a
// backend that changes them changes this file with the change.

use std::sync::atomic::{AtomicUsize, Ordering};

use chton::io::{FileIo, FsIo, IoFuture, WriteOp, default_apply_batch};
use futures_executor::block_on;

#[derive(Default)]
struct Cost {
    reads: AtomicUsize,
    writes: AtomicUsize,
    deletes: AtomicUsize,
    lists: AtomicUsize,
    list_pages: AtomicUsize,
    bytes_read: AtomicUsize,
    bytes_written: AtomicUsize,
}

/// A channel that counts what a caller asks of the channel inside it.
struct Counting<IO> {
    inner: IO,
    cost: Cost,
}

impl<IO> Counting<IO> {
    fn new(inner: IO) -> Self {
        Self {
            inner,
            cost: Cost::default(),
        }
    }

    fn reads(&self) -> usize {
        self.cost.reads.load(Ordering::Relaxed)
    }

    fn writes(&self) -> usize {
        self.cost.writes.load(Ordering::Relaxed)
    }

    fn deletes(&self) -> usize {
        self.cost.deletes.load(Ordering::Relaxed)
    }

    fn lists(&self) -> usize {
        self.cost.lists.load(Ordering::Relaxed)
    }

    fn list_pages(&self) -> usize {
        self.cost.list_pages.load(Ordering::Relaxed)
    }

    fn bytes_read(&self) -> usize {
        self.cost.bytes_read.load(Ordering::Relaxed)
    }

    fn bytes_written(&self) -> usize {
        self.cost.bytes_written.load(Ordering::Relaxed)
    }
}

impl<IO: FileIo> FileIo for Counting<IO> {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            self.cost.reads.fetch_add(1, Ordering::Relaxed);
            let bytes = self.inner.read(path).await?;
            let len = bytes.as_ref().map_or(0, Vec::len);
            self.cost.bytes_read.fetch_add(len, Ordering::Relaxed);
            Ok(bytes)
        })
    }

    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()> {
        Box::pin(async move {
            self.cost.writes.fetch_add(1, Ordering::Relaxed);
            self.cost
                .bytes_written
                .fetch_add(data.len(), Ordering::Relaxed);
            self.inner.write(path, data).await
        })
    }

    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>> {
        Box::pin(async move {
            self.cost.lists.fetch_add(1, Ordering::Relaxed);
            self.inner.list(prefix).await
        })
    }

    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()> {
        Box::pin(async move {
            self.cost.deletes.fetch_add(1, Ordering::Relaxed);
            self.inner.delete(path).await
        })
    }

    fn list_page<'a>(
        &'a self,
        prefix: &'a str,
        cursor: Option<&'a [u8]>,
        max: usize,
    ) -> IoFuture<'a, (Vec<String>, Option<Vec<u8>>)> {
        Box::pin(async move {
            self.cost.list_pages.fetch_add(1, Ordering::Relaxed);
            self.inner.list_page(prefix, cursor, max).await
        })
    }
}

fn channel(name: &str) -> (Counting<FsIo>, std::path::PathBuf) {
    // A directory per test: the test threads share a process, so a name from the
    // process alone would have them writing and deleting one directory.
    let dir = std::env::temp_dir().join(format!("chton-cost-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let io = Counting::new(FsIo::new(&dir).expect("a host directory"));
    (io, dir)
}

#[test]
fn a_point_read_costs_one_read_of_the_record_s_bytes() {
    let (io, dir) = channel("point-read");

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.expect("a write");
        let before = io.bytes_read();
        let record = io.read("facts/f_a.fact").await.expect("a read");
        assert_eq!(record.as_deref(), Some(&b"alpha"[..]));
        assert_eq!(io.reads(), 1, "a point read made more than one call");
        assert_eq!(
            io.bytes_read() - before,
            5,
            "a point read moved more than the record"
        );
    });

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_read_of_a_missing_key_costs_one_read_and_no_bytes() {
    let (io, dir) = channel("missing");

    block_on(async {
        let record = io.read("facts/f_missing.fact").await.expect("a read");
        assert!(record.is_none());
        assert_eq!(io.reads(), 1);
        assert_eq!(io.bytes_read(), 0, "a missing key moved bytes");
    });

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_write_costs_one_call_of_its_bytes() {
    let (io, dir) = channel("write");

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.expect("a write");
        assert_eq!(io.writes(), 1);
        assert_eq!(io.bytes_written(), 5);
    });

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_listing_costs_one_call_and_no_payload_read() {
    let (io, dir) = channel("listing");

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.expect("a write");
        io.write("facts/f_b.fact", b"beta").await.expect("a write");
        let before = io.bytes_read();
        let listed = io.list("facts/").await.expect("a listing");
        assert_eq!(listed.len(), 2);
        assert_eq!(io.lists(), 1, "a listing made more than one call");
        assert_eq!(io.bytes_read(), before, "a listing read a payload");
    });

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_page_of_a_channel_that_does_not_page_is_one_call() {
    // The host directory hands the whole listing over in one page, which is the
    // default the trait declares for a channel that cannot resume.
    let (io, dir) = channel("page");

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.expect("a write");
        let (keys, cursor) = io.list_page("facts/", None, 1).await.expect("a page");
        assert_eq!(keys.len(), 1);
        assert!(cursor.is_none());
        assert_eq!(io.list_pages(), 1);
    });

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_default_batch_applies_one_operation_at_a_time() {
    // The fallback the record layer reaches, because a `BatchIo` channel cannot
    // be seen through a `FileIo` field. Issue #22 tracks the dispatch.
    let (io, dir) = channel("batch");
    let ops = [
        WriteOp::Write {
            path: "facts/f_a.fact".to_string(),
            data: b"alpha".to_vec(),
        },
        WriteOp::Write {
            path: "facts/f_b.fact".to_string(),
            data: b"beta".to_vec(),
        },
        WriteOp::Delete {
            path: "facts/f_a.fact".to_string(),
        },
    ];

    block_on(async {
        default_apply_batch(&io, &ops).await.expect("a batch");
        assert_eq!(io.writes(), 2, "the fallback stopped writing per operation");
        assert_eq!(
            io.deletes(),
            1,
            "the fallback stopped deleting per operation"
        );
    });

    let _ = std::fs::remove_dir_all(dir);
}
