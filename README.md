# Chton

Materialization IO fabric: matrix router and transformation implementations for coordinate spaces over physical media.

Chton lands the tagma coordinate space onto physical media. The tagma
specification (syntagma) defines the coordinate space; chton provides the
materialization: byte-level bindings over media, per-space-type layout
strategies, and the backends that bind the tagma-kv CoordKV protocol
surface to origins.

The name chton comes from the Greek chthōn (earth): the layer every
project stands on. The tagma space is the ideal form; chton is the ground
it lands on. Memory is the native habitat of the coordinate space, so
chton materializes onto media outside memory: file, signal, network, and
GPU origins. Memory appears only as a projection surface, the mapped
binding of an external medium.

## Layers

| Layer | Content |
|:---|:---|
| origin | byte-level bindings: `Origin` trait with capability matrix (address mode, direction, persistence, binding), `MemoryOrigin`, `FileOrigin`, `MappedFileOrigin` (mmap, unix, the mapped binding) |
| binding | per-space-type materialization strategies: `SpaceStrategy` trait, `TreeStrategy<N>` (fixed-depth tree layout, the CoordSpaceN form) |
| kv | materialized key-value surface: `CoordKVStore<N>`, the tagma-kv `CoordKV`/`CoordKVKey` contract over the binding backend |
| io | flat key-space IO surface: `FileIo`, `BatchIo`, `FsIo` (absorbed from nexus) |

## Design

- The storage format is the memory layout: there is no separate
  serialization step.
- Addressing is per-level array indexing: depth is bounded by file size,
  never by integer width.
- A protocol is origin-agnostic and an origin is protocol-agnostic. Storage
  is an attribute of the destination: a disk origin stores by nature, a
  signal origin propagates by nature. The same protocol materializes over
  either.
- The key-value protocol surface is the tagma-kv CoordKV contract, owned by
  tagma; chton provides the materialization backends that bind the surface
  to origins.
- The record boundary in the kv layer is the seam for a codec layer:
  payload encryption before write and decryption after read change no
  trait surface and no slot layout.
- Per-space-type strategies keep the materialization independent of the
  space type: the fixed-depth tree (CoordSpaceN form) is the first strategy;
  DynCoordSpace and other tagma space types are later strategies on the same
  surface.

## Status

Early implementation. The origin, binding (tree strategy), and kv
(materialized CoordKV) layers work over memory, file, and mapped-file
origins, and the io layer absorbs the flat key-space surface from nexus.
The unit test suite runs through `./run.sh`. Checkpoint and restore,
wave origins, and the record codec layer are later work.

## Boundaries

Chton contains no domain model. FIH, knowledge synthesis, and personal
memory are concerns of the systems that consume chton as infrastructure
(nexus, rem).

## Dependencies

- tagma-core (github.com/ssccsorg/syntagma): the coordinate space
  specification

## License

Tagma core: Apache 2.0 (open-core). Chton space IO layer and enterprise
service components: commercial.
