# Chton

Materialization IO layer for coordinate spaces over physical media.

Chton lands the tagma coordinate space onto physical media. The tagma
specification (syntagma) defines the coordinate space; chton provides the
materialization: byte-level bindings over media, per-space-type layout
strategies, and materialization protocols.

## Layers

| Layer | Content |
|:---|:---|
| origin | byte-level bindings: `Origin` trait with capability matrix (address mode, direction, persistence, binding), `MemoryOrigin`, `FileOrigin` |
| binding | per-space-type materialization strategies: `SpaceStrategy` trait, `TreeStrategy<N>` (fixed-depth tree layout, the CoordSpaceN form) |
| protocol | materialization protocols over origins: key-value first (`KvStore`, `RegionKv`) |
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
- Per-space-type strategies keep the materialization independent of the
  space type: the fixed-depth tree (CoordSpaceN form) is the first strategy;
  DynCoordSpace and other tagma space types are later strategies on the same
  surface.

## Status

Early implementation. The origin, binding (tree strategy), and key-value
protocol layers work over memory and file origins. Unit tests and a usage
scenario are in the repository and run through `./run.sh`. Memory mapping,
checkpoint and restore, and wave origins are later work.

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
