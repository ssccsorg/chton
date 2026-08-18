# Chton: IO Materialization Layer Design

date: 2026-07-31
status: draft
project: Chton
related:
- tagma (syntagma)
- neXus (ssccs-nexus2)

## Context

This document records the design meeting outcome for the chton IO layer. It
supersedes the storage layer refactoring plan from 2026-07-27 with a finalized
three-layer ontology. The work is a coordinated refactoring across three
repositories: tagma (specification), chton (materialization), and nexus
(semantics). 

## Design Philosophy

### Ontology

tagma is the index coordinate space: a mathematical specification, not
storage. It owns definitions, including the meaning of transformations such as
coordspace to wave conversion. chton is the universal materialization layer:
it lands the tagma specification onto physical origins. nexus is the semantic
hub: an active collaborative blackboard on the materialized space.

### Boundaries

The coordinate space is structure. Key-value is one materialization
protocol, and its destination need not be disk: the same protocol runs over
memory, file, or signal origins. Storage is what a disk origin does,
transmission is what a signal origin does, and key-value is the protocol that
materializes over either. The current tagma-map is a native implementation
with a key-value interface, not a bridge to a legacy store: the key is a
CoordPath and the index is the coordinate space itself. The key-value surface
is the on-ramp; the coordinate structure is the engine. Interfaces belong to
the layer of their concept: space interfaces remain in tagma, while
materialization protocols belong entirely to chton.

The tagma family map:

| Family | Nature | Owner |
|:---|:---|:---|
| tagma-core | coordinates, paths, space structures | tagma (specification) |
| tagma-id | identity conventions | tagma (specification) |
| tagma-geo | spatial operations | tagma (specification) |
| tagma-map | materialization protocol (first protocol of chton) | chton (rename-level move, for example chton-map) |
| tagma-signal | definition of coordspace to wave conversion | tagma (specification) |
| wave materialization | signal origin implementation | chton (future) |

### Origin abstraction

chton binds a coordinate space to any origin. The core operation is:

```rust
fn bind(origin: Origin, space: TagmaSpace) -> MappedSpace;
```

Origins include memory, file, signal, network, GPU memory, and device
registers. The capability matrix distinguishes address mode (byte, block,
stream), direction (read, write, duplex), persistence (durable, volatile,
transient), and binding (mapped, copied, zero-copy). The first context
implements Memory and File origins.

### Internal layering

chton itself is strictly layered. The protocol layer is independent of the
origin layer, so any protocol can materialize onto any origin:

| Layer | Content | Constraint |
|:---|:---|:---|
| Protocol | materialization protocols (map first; log, blob, stream as later candidates) | origin-agnostic, defined over the origin surface |
| Binding | protocol to origin adaptation | capability matrix (address mode, direction, persistence, binding) |
| Origin | Memory, File (first context); Signal, Network, GPU (future) | protocol-agnostic, byte-level binding |

The map protocol over a signal origin is the wave instance: request and
response become coordinate emissions, which is the tagma-signal definition
realized as a protocol binding.

### Models

nexus is the semantic hub, chton is the IO connection with converters at each
end, and tagma is the shared form. Persistence is an attribute of the
destination, not a framework feature: a disk destination stores by nature, a
signal destination propagates by nature. Delivery and persistence are the same
operation viewed through different destinations.

## Practical Plan

### Transition principle

Transition order follows a fixed rule: implementations move first, traits
move second. Implementations are leaf nodes, so a move preserves callers
when the source repository re-exports the moved module (`pub use`), and each
move is verifiable by tests. Trait relocation happens only after the
implementation layout is settled, guided by the layer design, because traits
are the contracts that callers depend on. This ordering keeps code breakage
minimal at every step.

Concretely: tagma-map implementations move to chton first (rename-level, for
example chton-map) with syntagma re-exporting for compatibility; the CoordMap
interface then follows per design decision B (storage concepts complete
within chton), while space interfaces remain in tagma. nexus depends on tagma
for the space interface and on chton for the key-value layer. Moved
implementations become chton Origin::Memory as-is, so no deprecation period
is needed. nexus changes are limited to Cargo.toml dependencies and import
paths; consumers see no change.

### Phases

1. D2 form decision: fixed-depth, recursive, or hybrid space structure. This
   is the interface contract between tagma and chton and determines the
   key-value layout.
2. tagma refinement: specification only, no executors. The key-value
   implementations move out.
3. chton core: origin layer (Memory, File) with the protocol layer (map as
   the first protocol), mmap, checkpoint, and restore.
4. nexus integration: dependency switch, then internal adapter swap. The
   trait surface stays unchanged.


### Future scope

- Encryption flow: chton issue 1, consumed by nexus FIH
- Recovery guarantees: chton issue 2, resolved by memory-disk mapping
  semantics
- Wave origin: tagma-signal definition in tagma, materialization in chton once
  the concept is established

## Open Gate

D2, the space form decision, is the only remaining design gate. The geo
bounding box iterator already provides a structure-independent range query
specification.
