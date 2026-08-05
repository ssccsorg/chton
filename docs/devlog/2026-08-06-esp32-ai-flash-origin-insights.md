# esp32-ai: Storage-Side Implementation Insights

date: 2026-08-06
status: draft
project: Chton
related:
- tagma (syntagma)
- Chton development direction master (2026-08-01)
- Chton IO materialization design (2026-07-31)
- FileIo flat key-space boundary (chton/src/io/file_io.rs)
- nexus FIH blackboard (ssccs-nexus2)
- rem hardware roadmap (rem/docs/partneships/nexpcb.qmd)

## Context

The esp32-ai project runs a 28.9M-parameter language model on an ESP32-S3
with no connectivity. Its relevance to chton is not the model. It is the
storage pattern: a large payload held on bare-metal flash, addressed as a
flat key-space, and read sparsely per use. The FileIo boundary in chton
already names bare-metal flash as a backend. This devlog reads esp32-ai
from the storage side and turns the reading into a development plan for a
new origin column.

## The Case

The model stores 25M parameters in an embedding table on flash. Each
generated token pulls about 450 bytes from that table, roughly six rows,
into the compute path. The output head and weight tensors live in PSRAM,
staged at boot. The dense core stays flash-mapped and executes in place. The activations and norm weights live in SRAM. The tiers
are split by access frequency: many touches per token, one read per
position, and a few bytes per token.

Two properties matter for chton. The flash partition is a flat key-space:
a row index addresses an embedding vector, and the address is the offset.
The core runs flash-mapped and executes in place, a mapped binding
without an MMU.

## The Storage Pattern

The pattern generalizes as follows.

| Property | esp32-ai instance | chton abstraction |
|:---|:---|:---|
| medium | flash partition at 0x110000 | bare-metal flash origin |
| addressing | row index, address equals offset | per-level array indexing |
| access | sparse, about 450 bytes per token | sparse reads over a flat key-space |
| binding | execute-in-place, no MMU | mapped binding, MappedFileOrigin analogue |
| format | no serialization between table and reader | storage format is the memory layout |

The 25M-parameter table is a materialized coordinate space in the chton
sense: the storage format is the memory layout, and the reader projects
only the rows it needs.

## A New Origin Column Emerges

The materialization matrix grows by adding paths, with stacks
kept fixed. The esp32-ai case adds a cell: the fixed-depth tree strategy, or
a flat row layout, over a flash origin. The new column is the FlashOrigin.

FlashOrigin differs from FileOrigin in constraints, with the
contract unchanged.
The medium is byte-addressable, endurance-limited, and asymmetric in read
and write cost. The binding is XIP where the target supports it, and
explicit read where it does not. The contract stays the FileIo surface:
read, write, list, delete, over a flat key-space, with BatchIo where
atomic commit exists.

The row-addressable read pattern suggests a sparse access primitive in
the origin layer: read the addressed rows and leave the rest of the region untouched. The PLE table is the reference shape: rows are vectors, the key
is the row index, and the value is fixed width.

## Development Plan

Phase 1: extract the flash access pattern into the origin capability
matrix. Address mode (row-indexable), direction (read-heavy),
persistence (non-volatile), and binding (XIP or explicit read) are the
axes already present in the Origin trait.

Phase 2: implement FlashOrigin over a host-simulated flash image. A file
backed region with an XIP flag exercises the same contract without
hardware, matching the directory and disk-image development modes used
elsewhere in the stack.

Phase 3: add the sparse read primitive. The primitive takes a key list
and returns the addressed rows with a fixed-width record slot, so the
record slot size invariant from TreeStrategy carries over.

Phase 4: verify the pattern against the esp32-ai reference numbers. The
measured result, 9.88 tokens per second with 94.9 ms per token of
compute, bounds the read budget: about 450 bytes per token from flash,
which the flash origin must sustain without moving the whole table.

## Constraints and Open Gates

- Flash endurance and write symmetry: the flash origin favors the read
  path, and write amplification must be explicit in the capability
  matrix.
- No MMU on the target class: the mapped binding is XIP or an explicit
  read, so MappedFileOrigin semantics need a non-mmap fallback.
- Record slot size must be recorded in the region header, consistent
  with the TreeStrategy header hardening.
- The async FileIo surface applies; a sync wrapper covers the bare-metal
  path.

## Relation to the Master Paper

The development direction master fixes the growth rule: the fabric grows
by adding paths, with stacks kept fixed. FlashOrigin is a new origin
column on the existing surface. It does not create a new family. It
extends Chton-Storage with a bare-metal flash cell and leaves the matrix
surface, the address scheme, and the protocol paths unchanged.

The master paper lists the wave origin as the SignalOrigin column and
snapshot materialization as the SnapshotOrigin column. FlashOrigin is the
next column in the same order, and it is the first column whose reference
implementation exists outside the stack as measured silicon.
