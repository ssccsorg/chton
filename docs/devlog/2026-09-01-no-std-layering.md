# Chton: no_std layering for the MCU storage path

date: 2026-09-01
status: review
project: Chton
related:
- neXus issue #181 (OS-less storage path)
- tagma (syntagma, 181-tagma-geo-nostd)
- neXus nex sub-workspace (181-petgraph-cypher-removal)
- Chton IO extraction (2026-08-01)

## Context

The Rem hardware targets MCU-class silicon (RISC-V RV32IMAC, 512 KB SRAM,
no OS). The storage path, chton plus the nex semantic layer, must run
on-device as wasm32-wasip2 inside a launcher-provided WASI bridge. This
devlog records the layering conversion that made the chton library
genuinely std-free and verified it on a real no-std target.

The guiding principle, inherited from the nexus storage core, is
hierarchical isolation. Code that must stay environment-agnostic lives in
the no_std core. Code that needs allocation lives in the alloc-backed
layer. Code that legitimately requires an OS lives behind the std feature
and is excluded from MCU builds entirely. An OS requirement is never
smuggled into a lower layer; it is always pushed to the top.

## Layer model

The library is split into three layers. Each layer only depends on the
layers below it, never the reverse.

L0, the no_std core, has no allocator requirement: Cell2, the FileIo trait
family, MemoryOrigin, and the entity store contract. L1 is alloc-backed:
CoordMapStore and CoordMapStoreIo. L2 is std-backed and gated behind the
std feature: FsIo, FileOrigin, MappedFileOrigin, and the SyncFileIo
wrapper. L2 modules compile out on MCU targets and on
wasm32-unknown-unknown; they remain available on native hosts and on
wasm32-wasip2, where std::fs exists.

## Cell2

Cell2 is the interior-mutability primitive for the store surface. It
replaced the per-platform std::sync::Mutex and wasm RefCell split with a
single critical-section implementation, so one code path serves native
hosts and MCU targets alike.

The native guard is a critical-section Mutex that wraps a RefCell. The
guard holds the critical section open for its lifetime and releases it on
drop. The RefCell adds the same-thread reentrancy check: a second borrow
in the same thread panics, matching the wasm RefCell path. Cross-thread
access blocks on the critical-section implementation until the guard
drops, matching the original Mutex semantics. The guard declares the
critical-section state field after the RefCell borrow, so the borrow flag
is cleared before the section is released.

The std implementation of critical-section 1.2 permits nested acquisition
within the same thread, so the RefCell reentrancy check, not the critical
section, is the real panic point for nested borrows. Six contract tests
pin the behavior: value round trip, nested shared borrow, shared-to-exclusive
panic, cross-thread serialization, independent cells, and guard scope.

## FileIo trait surface

The FileIo trait carries a Send + Sync bound on native targets and drops
it on wasm32-unknown-unknown. wasm32-wasip2 previously fell into the
native branch and inherited the bound, which single-threaded MCU runtimes
(Wasmi, WAMR) do not need. The wasip2 branch now takes the same
single-threaded surface as wasm32-unknown-unknown.

## std feature gates

The std feature enables the host-only backends and pulls in
critical-section's std implementation, which provides the
critical-section symbols on std targets. The gate also enables the
optional dependencies that only host backends use: futures-executor for
the SyncFileIo block_on wrapper, and walkdir for FsIo. memmap2 stays a
unix-only target dependency of the std build. The no_std target keeps the
feature off, and the firmware crate supplies a critical-section
implementation, for example through a HAL.

The shared dependencies were made std-free at the source: serde uses
default-features = false with derive and alloc, sha2 uses
default-features = false, and postcard uses the alloc feature. These were
leak points where a default feature would pull std into the MCU build.

## Verification gates

The run.sh CI gate now covers the storage path end to end. The gate runs
the no-default-features check, the no_std anchor integration tests, the
wasm32-unknown-unknown check as the true no_std target, and the
riscv32imac-unknown-none-elf check as the MCU target.

The no_std anchor tests run on the native host with an explicit
critical-section implementation provided in the test binary, because the
no_std build disables critical-section's std implementation and the native
test linker needs the symbols. A real MCU provides the same symbols from
the firmware.

The MCU gate targets the library only. The benches crate is a host-only
harness that depends on futures-executor and criterion, so it is excluded
from the no-std target checks, mirroring how the nexus core runner scopes
its MCU checks to the nex storage crates.

## Build status per crate

The riscv32imac-unknown-none-elf check compiles cleanly for every layer.
tagma-core was already no_std and alloc. tagma-geo and tagma-map were
converted to no_std, including a 32-bit fix for a coordinate-slot
constant overflow that only surfaced on the 32-bit target. chton, nex-core,
nex-io, and nex-fih all pass the target check with zero warnings. The
nex-fih layer gates its std-only surfaces, the export bundle, the
FihBlackboard sync wrapper, and the FihSession wrapper, behind the std
feature, so the MCU build excludes them without losing them on hosts.

## Issues encountered

The workspace-level riscv32 check initially failed on slab and
futures-core, pulled in by the benches crate through futures-executor.
Scoping the check to the chton library resolved it. A clippy
let-underscore-future error in the anchor test was fixed by dropping the
constructed future explicitly with core::mem::drop.

## References

- neXus issue #181: OS-less storage path
- Rem hardware spec: nexpcb.qmd, no_std firmware stack
- Rem firmware architecture: FihStorage on wasm32-wasip2
- Chton IO extraction devlog: 2026-08-01
