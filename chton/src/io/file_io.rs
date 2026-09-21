// ── FileIo: flat key-space file IO abstraction ───────────────────────
//
// The single IO boundary. Every IO backend (local fs, remote object
// store, in-memory HashMap, bare-metal flash) implements this trait.
// The core never calls IO directly.
//
// Despite the name, this trait does NOT require `std::fs` or a local
// filesystem. Implementations include:
//   - SimIo: in-memory HashMap (nexus storage/sim)
//   - FsIo: std::fs (this crate)
//   - wasm backends: provider-specific adapters (CF Workers R2, spin)
//   - (your backend here): any flat key-space with read/write/list/delete
//
// # BatchIo (lego trait)
//
// `apply_batch` is a separate concern from read/write/list/delete.
// Implementations that support atomic batch commits implement `BatchIo`
// in addition to `FileIo`. This separation lets callers distinguish
// between backends that batch (R2 with concurrent sends) and those that
// don't (simple filesystem).
//
// # Why async?
//
// I/O is inherently asynchronous. At the hardware level, every I/O
// operation (DRAM read, DMA transfer, NVMe queue, network round-trip)
// involves pipelining, interrupts, or completion queues. None of it is
// truly synchronous. "Sync" is a programmer convenience abstraction over
// cooperative scheduling (async) or preemptive scheduling (OS threads).
//
// By making FileIo async at the trait level, we align with:
//   - CF Workers: await on R2 bucket.get() directly (no block_on)
//   - tokio: spawn + await on async fs/network
//   - wasm32: single-threaded, cooperative multitasking via await
//
// Sync callers use SyncFileIo wrapper, which calls
// futures_executor::block_on internally. Async is the design center;
// sync is the extension.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;

/// Type alias to suppress clippy::type_complexity on FileIo methods.
/// On non-wasm targets the future is Send; single-threaded wasm runtimes
/// (browser, wasip1, MCU wasip2 under Wasmi/WAMR) do not need it.
#[cfg(not(target_family = "wasm"))]
pub type IoFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

#[cfg(target_family = "wasm")]
pub type IoFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + 'a>>;

/// A single IO operation that can be committed or rolled back.
/// The caller enqueues these; the flush layer commits them as a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Write a record file: path -> bytes.
    Write { path: String, data: Vec<u8> },
    /// Delete a single file.
    Delete { path: String },
}

/// Async IO operations on a flat key-space.
///
/// The Send + Sync bound applies to non-wasm targets only. wasm targets
/// (browser wasm32-unknown-unknown, wasip1, and the MCU wasip2 launcher
/// target) are single-threaded runtimes, so the trait drops the bounds
/// there; a firmware or launcher backend still satisfies them if it can.
#[cfg(not(target_family = "wasm"))]
pub trait FileIo: Send + Sync {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>>;
    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()>;
    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>>;
    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()>;

    /// Keys under `prefix` one page at a time, so a holder of the enumeration keeps a page
    /// rather than the list.
    ///
    /// `cursor` is the token the previous call returned, or `None` to start. The call returns
    /// at most `max` keys and the token that continues the enumeration, or `None` when the
    /// enumeration is done. No order is promised, neither across the pages nor within one, so a
    /// channel that can only enumerate in whatever order its medium holds is not asked for more.
    ///
    /// The default hands over every key in one call, which is what `list` already does, so an
    /// implementor that cannot resume is not changed by this. A caller that needs a bounded page
    /// asks for a channel that overrides it, and the portability of this method is the reason it
    /// takes a cursor rather than a visitor: a visitor in the signature would carry the
    /// cfg-conditional `Send` bound of [`IoFuture`], and every override would then need two
    /// copies of itself.
    ///
    /// `max` is a bound the channel may honour. The default ignores it rather than dropping keys
    /// it has no way to hand over later.
    fn list_page<'a>(
        &'a self,
        prefix: &'a str,
        cursor: Option<&'a [u8]>,
        _max: usize,
    ) -> IoFuture<'a, (Vec<String>, Option<Vec<u8>>)> {
        Box::pin(async move {
            if cursor.is_some() {
                return Ok((Vec::new(), None));
            }
            let keys = self.list(prefix).await?;
            Ok((keys, None))
        })
    }
}

#[cfg(target_family = "wasm")]
pub trait FileIo {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>>;
    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()>;
    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>>;
    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()>;

    /// Keys under `prefix` one page at a time, so a holder of the enumeration keeps a page
    /// rather than the list.
    ///
    /// `cursor` is the token the previous call returned, or `None` to start. The call returns
    /// at most `max` keys and the token that continues the enumeration, or `None` when the
    /// enumeration is done. No order is promised, neither across the pages nor within one, so a
    /// channel that can only enumerate in whatever order its medium holds is not asked for more.
    ///
    /// The default hands over every key in one call, which is what `list` already does, so an
    /// implementor that cannot resume is not changed by this. A caller that needs a bounded page
    /// asks for a channel that overrides it, and the portability of this method is the reason it
    /// takes a cursor rather than a visitor: a visitor in the signature would carry the
    /// cfg-conditional `Send` bound of [`IoFuture`], and every override would then need two
    /// copies of itself.
    ///
    /// `max` is a bound the channel may honour. The default ignores it rather than dropping keys
    /// it has no way to hand over later.
    fn list_page<'a>(
        &'a self,
        prefix: &'a str,
        cursor: Option<&'a [u8]>,
        _max: usize,
    ) -> IoFuture<'a, (Vec<String>, Option<Vec<u8>>)> {
        Box::pin(async move {
            if cursor.is_some() {
                return Ok((Vec::new(), None));
            }
            let keys = self.list(prefix).await?;
            Ok((keys, None))
        })
    }
}

/// Optional buffering capability: vessels that hold buffered state
/// before it becomes durable implement this. The buffer spec gathers the
/// semantics in one place: whether non-durable state exists, and how to
/// persist it.
///
/// This is a Lego trait, separate from [`FileIo`], so the flow surface
/// stays pure: write-through and in-memory backends implement only
/// [`FileIo`], while buffering vessels (map header, mapped pages) implement
/// both. Consumers that need durability bind to `FileIo + BufferIo`.
pub trait BufferIo {
    /// Whether the vessel holds buffered state that is not yet durable.
    fn is_buffered(&self) -> bool;

    /// Persist buffered state to the medium.
    fn flush<'a>(&'a self) -> IoFuture<'a, ()>;
}

/// Batch IO: lego trait for backends that support atomic batch commits.
/// Separate from FileIo so callers can type-check batch support at compile time.
#[cfg(not(target_family = "wasm"))]
pub trait BatchIo: FileIo {
    fn apply_batch<'a>(&'a self, ops: &'a [WriteOp]) -> IoFuture<'a, ()>;
}

#[cfg(target_family = "wasm")]
pub trait BatchIo: FileIo {
    fn apply_batch<'a>(&'a self, ops: &'a [WriteOp]) -> IoFuture<'a, ()>;
}

/// A channel whose operations complete on their first poll.
///
/// A device channel does its work without waiting: memory, a mapped page, and
/// flash all do. Such a channel needs no executor, which is what lets a
/// synchronous consumer poll it once and take the result, and a consumer that has
/// no executor MUST only drive a channel that declares this. A channel that can
/// suspend, such as a host file system behind an asynchronous runtime, makes no
/// such claim and MUST be driven by its caller's own executor.
///
/// The declaration is a claim by the channel author and it is the whole of the
/// surface: a consumer asks for it as a bound, `IO: ReadyIo`, and a channel that
/// cannot make the claim stays out of that position. The claim is checkable, and
/// `tests/io.rs` drives each operation of a channel that makes it once and
/// requires it to be ready.
pub trait ReadyIo: FileIo {}

/// A channel whose writes are durable when they return.
///
/// [`BufferIo`] says that a consumer which needs durability binds to
/// `FileIo + BufferIo` and flushes state itself. This wrapper is that binding for
/// a consumer that would rather the write itself were durable, which is what a
/// record layer and a C caller expect: the write they were told succeeded is on
/// the medium.
///
/// Write and delete persist the channel's buffer when the operation left it
/// holding state, and read and list are forwarded as they are. A channel that is
/// already durable on write needs no wrapper, and wrapping one costs a check of
/// `is_buffered` after each write and nothing else.
///
/// The wrapper adds no wait, so it inherits the readiness of the channel inside
/// it: `Durable<A>` implements [`ReadyIo`] when `A` does.
pub struct Durable<IO> {
    inner: IO,
}

impl<IO> Durable<IO> {
    /// Wrap a channel so that a write is durable when it returns.
    pub fn new(inner: IO) -> Self {
        Self { inner }
    }

    /// The channel inside, for the consumer that composed it and may want to
    /// flush it directly.
    pub fn inner(&self) -> &IO {
        &self.inner
    }
}

impl<IO: FileIo + BufferIo> Durable<IO> {
    /// Persist the channel's buffer if the operation left it holding one.
    async fn persist(&self) -> Result<(), String> {
        if self.inner.is_buffered() {
            self.inner.flush().await?;
        }
        Ok(())
    }
}

impl<IO: FileIo + BufferIo> FileIo for Durable<IO> {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>> {
        self.inner.read(path)
    }

    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()> {
        Box::pin(async move {
            self.inner.write(path, data).await?;
            self.persist().await
        })
    }

    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>> {
        self.inner.list(prefix)
    }

    /// Forwarded, so a wrapped channel keeps the paging it declared rather than falling back to
    /// the default that hands over the list.
    fn list_page<'a>(
        &'a self,
        prefix: &'a str,
        cursor: Option<&'a [u8]>,
        max: usize,
    ) -> IoFuture<'a, (Vec<String>, Option<Vec<u8>>)> {
        self.inner.list_page(prefix, cursor, max)
    }

    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()> {
        Box::pin(async move {
            self.inner.delete(path).await?;
            self.persist().await
        })
    }
}

impl<IO: FileIo + BufferIo> BufferIo for Durable<IO> {
    fn is_buffered(&self) -> bool {
        self.inner.is_buffered()
    }

    fn flush<'a>(&'a self) -> IoFuture<'a, ()> {
        self.inner.flush()
    }
}

impl<IO: FileIo + BufferIo + ReadyIo> ReadyIo for Durable<IO> {}

/// Default apply_batch for any FileIo that does not implement BatchIo.
/// Iterates sequentially over ops.
pub async fn default_apply_batch(io: &impl FileIo, ops: &[WriteOp]) -> Result<(), String> {
    for op in ops {
        match op {
            WriteOp::Write { path, data } => io.write(path, data).await?,
            WriteOp::Delete { path } => io.delete(path).await?,
        }
    }
    Ok(())
}

/// Wraps a FileIo into a blocking/sync interface.
/// Uses futures_executor::block_on internally.
///
/// Std-only: `block_on` needs an executor, which needs std. On no_std
/// targets (MCU) callers drive the async `FileIo` methods directly from
/// the launcher's own executor (e.g. embassy).
#[cfg(feature = "std")]
pub struct SyncFileIo<A: FileIo> {
    inner: A,
}

#[cfg(feature = "std")]
impl<A: FileIo> SyncFileIo<A> {
    pub fn new(inner: A) -> Self {
        Self { inner }
    }

    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        futures_executor::block_on(self.inner.read(path))
    }

    pub fn write(&self, path: &str, data: &[u8]) -> Result<(), String> {
        futures_executor::block_on(self.inner.write(path, data))
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        futures_executor::block_on(self.inner.list(prefix))
    }

    pub fn delete(&self, path: &str) -> Result<(), String> {
        futures_executor::block_on(self.inner.delete(path))
    }

    pub fn apply_batch(&self, ops: &[WriteOp]) -> Result<(), String> {
        futures_executor::block_on(default_apply_batch(&self.inner, ops))
    }
}
