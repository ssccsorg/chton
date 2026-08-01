# Chton: Development Direction Master

date: 2026-08-01
status: draft
project: Chton
related:
- tagma (syntagma)
- neXus (ssccs-nexus2)
- Chton IO materialization design (2026-07-31)
- Chton IO infrastructure extraction (2026-08-01)

## Context

This master paper consolidates the design direction for the chton
materialization layer. It supersedes nothing; it binds the earlier design
meeting (2026-07-31), the IO extraction work (2026-08-01), and the code
review findings into one development policy. The paper fixes the growth
rule that all future work must satisfy: the fabric grows by adding paths,
not by adding stacks.

## The Fabric Principle

The materialization surface is one matrix. Space types are rows, origins
are columns, and the layer stack is the shared vertical axis. A materialized
family is a set of lit cells in this matrix, never a separate stack:

| Family | Cells | Status |
|:---|:---|:---|
| Chton-Storage | Fixed-depth tree x Memory, File | implemented |
| Chton-Wave | Recursive tree x Signal | future |
| Chton-Memory | Dense array x Snapshot | future |

Growth means adding a new origin column or a new space type row. The matrix
surface, its address scheme, and its protocol paths stay unchanged. Every
family is a source that is also an output, so the same surface serves any
pair of abstracted sources and outputs. The materialization path matrix is
the hub.

Consequences for design:

- tagma-kv is absorbed as a protocol path, not as a KV stack beside the
  matrix.
- nexus consumes chton as one surface, not as parallel IO stacks.
- checkpoint and restore become the SnapshotOrigin column, not a new layer.
- the wave origin becomes the SignalOrigin column, not a new stack.

## Layer Policy

The three-layer ontology is closed:

| Layer | Role | Owner |
|:---|:---|:---|
| tagma | transformation theory: what a conversion is | specification |
| chton | transformation execution: how a conversion binds | materialization |
| nexus | semantics over the materialized space | semantic hub |

tagma owns definitions only. The tagma family map is final:

| Family | Nature | Owner |
|:---|:---|:---|
| tagma-core | coordinates, paths, space structures | tagma (specification) |
| tagma-id | identity conventions | tagma (specification) |
| tagma-geo | spatial operations | tagma (specification) |
| tagma-kv | storage semantics | chton (too specific, storage concept) |
| tagma-wave | definition of conversion | tagma (specification) |
| wave materialization | signal origin implementation | chton (future) |

The tagma-wave document stays in the tagma definition layer. Signal origin
materialization moves to chton once the concept is established.

## Current State

The chton workspace implements the origin, binding, protocol, and io
layers over memory and file origins. Fourteen tests cover origin, binding,
and protocol. The io module is a flat key-space surface absorbed from
nexus nex-io, which is now a re-export shim. tagma-core resolves as a git
dependency and is unchanged. The nexus side carries the io split on branch
164-chton-io-split.

## Code Review Findings

The code review against the fabric principle found two violations and one
open gate.

### Binding layer leaks protocol semantics

The SpaceStrategy trait carries key-value record management
(alloc_record, free_record, record_slot_size) into a layer documented as
protocol-agnostic. A log or blob protocol would inherit record and free
list semantics it does not need, splitting the binding trait per protocol.
The matrix must keep binding (how a space lays out over an origin) and
protocol (how a path is read and written) separate so that any protocol
can traverse any space strategy.

### The space form is not recorded in the format

The header stores magic, version, bump, and free list head but not the
tree depth N, node size, or record slot size. A file written at depth 2
reopened at depth 1 misreads silently. The D2 gate (fixed-depth tree) is
effectively closed, so N is the row index of the matrix and must be part
of the file contract.

### The D2 gate

The fixed-depth tree form is the base. Addressing is per-level array
indexing, bounded by file size, never by integer width. The recursive and
dense forms are later rows on the same surface.

## Other Review Findings

Storage integrity is the weakest area:

- FsIo allows path traversal: the resolve check accepts dots and slashes
  and does not constrain the joined path.
- A corrupt length prefix in the key-value record triggers unbounded
  allocation.
- The record slot size invariant is unenforced; an undersized slot
  overwrites neighboring records silently.
- An empty value is conflated with an absent key because zero is both the
  absent sentinel and a valid length.
- Short reads are interpreted as absence because the origin performs a
  single read call without a read-exact loop.
- The on-disk format is not self-describing and header fields are not
  validated; a torn write can leave live regions reused silently.
- The io module has no tests at all, and no build target coverage exists
  for wasm32-wasip2 or wasm32-unknown-unknown in the tooling.

The zero-serialization claim is directionally true: values are raw bytes
and coordinates are positional array indices. It is fragile because the
layout lacks integrity metadata and schema versioning.

## Migration Order

The transition rule is fixed: implementations move first, traits move
second, and the source repository re-exports for compatibility. The order:

1. finish the nexus io split: delete dead implementation files, pin the
   chton and tagma-core git dependencies across all lockfiles, resolve the
   unused chton dependency in fih-model
2. move tagma-kv into chton as a protocol path, with syntagma re-exporting
   for compatibility
3. refine tagma to specification only, no executors
4. implement checkpoint and restore as the SnapshotOrigin column, and
   mmap as the mapped binding of the same layout
5. switch nexus dependencies, then swap internal adapters; the trait
   surface stays unchanged
6. switch rem to write_checkpoint and restore_from_snapshot as the first
   consumer verification

tagma stays unchanged until step 2. The first integration is on the nexus
side because implementations live there; tagma changes begin when the
key-value implementations move out.

## Open Items

- MappedFileOrigin (mmap, mapped binding) on unix
- checkpoint and restore as the SnapshotOrigin column
- recursive and dense space strategies as later matrix rows
- the wave origin as the SignalOrigin column once the tagma-wave concept
  is established
- build target map (unix, wasip2, wasm32) with tooling coverage
- io layer tests and storage integrity fixes from the code review
