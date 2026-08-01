# Chton: IO Infrastructure Extraction from tagma and nexus

date: 2026-08-01
status: draft
project: Chton
related:
- tagma (syntagma)
- neXus (ssccs-nexus2)
- Chton IO materialization design (2026-07-31)

## Context

This devlog records the initial extraction work that created the chton IO
infrastructure: IO implementation details were pulled from the tagma and
nexus codebases into chton, the materialization layer of the three-layer
ontology (tagma = specification, chton = IO materialization, nexus =
semantics).

## Extraction from nexus

The nexus IO abstraction lived in the nex-io crate: FileIo (async flat
key-space IO), BatchIo, IoFuture, WriteOp, SyncFileIo, FsIo, and
default_apply_batch. Per the IO layer separation, IO concepts belong to
chton. The move followed the transition rule: implementations first, with
the source crate re-exporting for compatibility.

- All nex-io content, traits and implementations, moved to chton as the io
  module
- nex-io became a re-export shim (`pub use chton::io::*;`) so consumers
  compile unchanged
- The moved code was reviewed for domain-specific concepts: zero remain
- Dependency wiring: chton is a public git repository; tagma-core resolves
  as a git dependency on the syntagma repository main branch

## Extraction from tagma

The tagma side contributed the space structure and its addressing
semantics: Coord, CoordPath, and CoordSpaceN, the fixed-depth tree of
11,172-wide nodes. The materialization of this structure is chton's
responsibility.

- The initial flat mixed-radix packing was rejected: it overflows u64 at
  depth 5 and above, which is below the CoordId scale used by consumers
- The tree form is the base: per-level array indexing, depth bounded by
  file size, never by integer width
- The space structure remains in tagma; the per-type materialization
  strategy lives in chton

## The chton Infrastructure

### Origin layer

The Origin trait exposes the byte-level surface shared by every medium,
with the capability matrix (address mode, direction, persistence, binding):

- MemoryOrigin: volatile byte region with EOF semantics for sparse reads
- FileOrigin: durable byte region with portable positional IO via Seek

### Binding layer

SpaceStrategy is the per-space-type materialization surface. TreeStrategy
is the first strategy: the CoordSpaceN layout materialized over an origin.

- branch nodes: 11,172 x u64 child offsets, 0 means absent
- leaf nodes: 11,172 x u64 record offsets, 0 means absent
- single bump allocator with the root span reserved, free list for record
  reuse
- header state (magic, version, bump, free list head) persisted in the
  origin

### Protocol layer

The key-value protocol is the first materialization protocol: keys are
coordinate paths, values are opaque bytes. The value format is a length
prefix plus raw bytes. The layout is the storage format; there is no
serialization step.

### Tooling

run.sh is the single entry point: fmt, clippy, build, tests, and the
kv_demo usage scenario. Fourteen tests cover the origin, binding, and
protocol layers, including a depth-6 path walk and file persistence across
reopen.

## Portability note

The wasm32-wasip2 build (spin) exposed the Unix-only nature of the first
FileOrigin implementation. Positional IO now uses Seek plus Read and
Write, portable across unix and wasip2. The supported build target map is
managed as a coordinated follow-up; IO tools are implemented with variety
so that space-type and build-target combinations each have a usable tool.

## Direction

The Chton brand covers the materialization families: Chton-Storage (disk
and filesystem origins, the current role), Chton-Wave (signal origins, the
tagma-wave materialization side), and Chton-Memory (time and snapshot
materialization). Memory mapping, checkpoint and restore, and wave origins
are later work.
