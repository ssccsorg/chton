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
}

#[cfg(target_family = "wasm")]
pub trait FileIo {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>>;
    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()>;
    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>>;
    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()>;
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
