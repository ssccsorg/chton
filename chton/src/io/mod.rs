// ── chton io: flat key-space IO abstraction ────────────────────────────
//
// The async path-based IO surface of the materialization layer. It
// coexists with the byte-level Origin trait (crate::origin). FileIo is
// the async path-based surface; Origin is the sync byte-level surface.
// Unification of the two surfaces is a later design step.
//
// This module has zero knowledge of domain models, contracts, or storage
// semantics. It is a pure IO abstraction.

pub mod file_io;
/// Filesystem-backed IO. Not available on wasm32-unknown-unknown.
/// (Available on wasm32-wasip2 where std::fs is present.)
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub mod fs_io;

pub use file_io::{BatchIo, FileIo, IoFuture, SyncFileIo, WriteOp, default_apply_batch};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use fs_io::FsIo;
