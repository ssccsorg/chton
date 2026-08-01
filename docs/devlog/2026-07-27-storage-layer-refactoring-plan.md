# Storage Layer Refactoring Plan

date: 2026-07-27
status: draft
project: Chton
related:

- neXus (ssccs-nexus2)
- Tagma Core (syntagma)

## Context

Chton is the commercial Tagma store: a collection of storage specifications,
persistence solutions, and methodologies built on top of Tagma Core. It is not a
layer that sits beneath neXus; rather, neXus is built on Chton's
infrastructure and specification.

Currently, neXus contains its own storage engine internally (FileIo trait,
PersistentSpace, FihStorage). This engine needs to be extracted into Chton
so that:

- Chton becomes the one true low-level IO/storage layer for the entire SSCCS
  ecosystem
- neXus focuses on FIH lifecycle, knowledge base, and swarm agent runtime
- External projects see zero API change

## Core Principle

External projects that already use neXus must experience **zero
code changes**. Only neXus's internal dependency changes: instead of owning its
storage engine, neXus imports it from Chton.

## Hierarchy

```text
Tagma Core (syntagma)
  Coord, CoordPath, CoordSpace, CoordSet, 300-gate decoder
  Every axis is just an axis. Time is an axis, not a special index.
    |
    v
Chton (ct)
  = Tagma Core + production components = commercial Tagma store
  |-- io::FileIo          (trait + FsIo, SyncFileIo, BatchIo)
  |-- io::SpaceIo         (coord-based IO, built on FileIo)
  |-- space::PersistentSpace  (mmap persistence, crash recovery)
  |-- cluster::*          (Coord-based sharding, rebalance-free)
  |-- proto::*            (RESP, S3 REST adapters)
  |-- security::*         (TLS/mTLS, RBAC, at-rest encryption)
  |-- ops::*              (CLI, Prometheus, Grafana)
    |
    +---> neXus (ssccs-nexus2)
    |       FihRuntime on Chton
    |       FihStorage, FihBlackboard, nexd, swarm agents
    |
    |
    +---> (future projects, selective use of Chton/neXus)
```

## What Moves Where

| Current Location (neXus) | Target (coordspace) | Notes |
|--------------------------|---------------------|-------|
| `nex::io::FileIo` | `coordspace::io::FileIo` | Trait: read/write/list/delete |
| `nex::io::BatchIo` | `coordspace::io::BatchIo` | Trait: apply_batch |
| `nex::io::WriteOp` | `coordspace::io::WriteOp` | Enum: Write/Delete |
| `nex::io::IoFuture` | `coordspace::io::IoFuture` | Type alias |
| `nex::io::FsIo` | `coordspace::io::FsIo` | Filesystem impl |
| `nex::io::SyncFileIo` | `coordspace::io::SyncFileIo` | Sync wrapper |
| `nex::io::default_apply_batch` | `coordspace::io::default_apply_batch` | Helper fn |
| `nex::storage::core::PersistentSpace` | `coordspace::space::PersistentSpace` | With rebuild_cache, flush |
| `nex::storage::composite` | — | **Will be removed entirely** |
| `nex::storage::petgraph` | — | **Will be removed entirely** |
| `nexus_model::FihCoord` | — | **Removed**, replaced by `CoordPath` |
| `nexus_model::FihHash` | — | **Removed**, CoordPath newtype |

## What Stays in neXus

| Item | Reason |
|------|--------|
| `FihStorage<IO>` | FIH lifecycle: submit_fact, submit_intent, submit_hint, claim, conclude |
| `FihBlackboard` | Swarm agent coordination, stigmergy |
| `Fact`, `Intent`, `Hint` | FIH semantic types |
| `nexd` | Runtime daemon |

## What Gets Removed

| Item | Replacement |
|------|-------------|
| `nexus_model::FihCoord` | `CoordPath` (already in tagma-core) |
| `nexus_model::FihHash` | `CoordPath` newtype |
| `nexus_storage_composite` | **Removed** — not needed |
| `nexus_storage_petgraph` | **Removed** — CoordSet bitwise ops replace graph traversal |
| `AxisIndex` | CoordPath axis slicing (list with prefix) |
| `TimeIndex` | Time is a coordinate axis; no special index needed |
| `SpatioTemporalIndex` | Same as TimeIndex; just another axis |

## Interface Contract (Zero Breakage)

### FileIo trait — unchanged signature

```rust
// Before:  nex::io::FileIo
// After:   coordspace::io::FileIo, re-exported by nex::io
pub trait FileIo: Send + Sync {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>>;
    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()>;
    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>>;
    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()>;
}
```

### FihStorage API — unchanged

```rust
// Before & After: identical signatures
impl<IO: FileIo> FihStorage<IO> {
    pub fn new(io: IO, project_id: &str) -> Self;
    pub async fn submit_fact(&self, fact: &Fact) -> Result<FihHash>;
    pub async fn read_state(&self) -> StateSnapshot;
    pub async fn rebuild_cache(&self) -> Result<()>;
    // ... all existing methods
}
```

### FatIo — unchanged

```rust
// Before: impl nex::io::FileIo for FatIo
// After:  impl coordspace::io::FileIo for FatIo (nex re-exports, same trait)
impl FileIo for FatIo { ... }
```

## Implementation Steps

### Phase 1: coordspace crate scaffolding

Create the `coordspace` crate in the cs repository. Add modules:

```text
coordspace/src/
|-- lib.rs
|-- io/
|   |-- mod.rs
|   |-- fs.rs
|   +-- sync.rs
+-- space.rs
```

Dependencies: `tagma-core` from `github.com/ssccsorg/tagma`.

### Phase 2: FileIo migration

1. Copy `FileIo`, `BatchIo`, `WriteOp`, `IoFuture`, `default_apply_batch` from
   `nex/src/io/file_io.rs` into `coordspace/src/io/mod.rs`.
2. Copy `FsIo` from `nex/src/io/fs_io.rs` into `coordspace/src/io/fs.rs`.
3. Copy `SyncFileIo` into `coordspace/src/io/sync.rs`.
4. Change `nex/src/io.rs` to `pub use coordspace::io::*`.
5. Add `coordspace` dependency to `nex/Cargo.toml`.
6. Run `cargo test` on nex — must pass.

### Phase 3: PersistentSpace migration

1. Copy `PersistentSpace` (with rebuild_cache, flush, recovery) from
   `nex/src/storage/core/` into `coordspace/src/space.rs`.
2. Add `SpaceIo` trait to `coordspace/src/io/mod.rs` (coord-based IO atop FileIo).
3. Refactor `FihStorage` to use `coordspace::space::PersistentSpace` internally.
4. Run `cargo test` on nex — must pass.

### Phase 4: Type unification

1. Replace `nexus_model::FihHash` with a `CoordPath` newtype.
2. Remove `nexus_model::FihCoord` (use `CoordPath` directly).
3. Update `FihStorage` to use `CoordPath` internally.
4. Re-export from `nexus_model` for backward compatibility.
5. Run `cargo test` on nex — must pass.

### Phase 5: Remove obsolete storage backends

1. `nexus-storage-composite` — remove workspace member, delete code.
2. `nexus-storage-petgraph` — remove workspace member, delete code.

### Phase 6: Verify external projects

1. Run `cargo test` on **rem** — zero code changes expected.
2. Run `cargo test` on **es** — zero code changes expected.

## Design Decisions

1. **No special index types.** Time is a coordinate axis. Spatial queries use
   CoordSet bitwise operations. No separate TimeIndex, AxisIndex, or
   SpatioTemporalIndex structs.

2. **Chton does not know about FIH.** It is a low-level IO layer.
   `Fact`, `Intent`, `Hint` are neXus concepts. CoordSpace deals only in
   `CoordPath -> bytes` mappings.

3. **Chton owns security.** Encryption, signing, and access control live
   in `coordspace::security`. neXus inherits these through the IO layer.

4. **Minimum viable extraction.** Only FileIo and PersistentSpace move in the
   initial phase. Protocol adapters, clustering, and security layers are added
   incrementally as Chton Enterprise matures.

5. **FatIo stays.** It implements `FileIo` and its location is not
   critical. It could move into Chton later if needed.

## Timeline (Estimated)

| Phase | Duration | Deliverable |
|-------|----------|-------------|
| 1 | 1 day | `coordspace` crate with io/ mod |
| 2 | 2 days | FileIo migrated, nex re-exports, tests passing |
| 3 | 3 days | PersistentSpace migrated, FihStorage uses it |
| 4 | 2 days | FihHash/FihCoord replaced by CoordPath |
| 5 | 1 day | composite/petgraph removed |
| 6 | 1 day | External project verification |
| **Total** | **10 days** | Chton v0.1.0 + neXus v0.2.0 |

## External project impact

| Project | Code changes | Cargo.toml changes |
|---------|-------------|-------------------|
| **nex-calc, nex-api** | 0 | 0 |
