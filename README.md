# Chton: Spatial Computing Persistent IO Layer

Arithmetic, not hashing. Coordinates, not tables. O(1), not O(log N).

Chton eliminates hashing and indexing at the structural level. It replaces them with deterministic arithmetic on coordinate spaces.

## Architecture

One core, two roles:

- **Layer** over legacy databases (PostgreSQL, Redis, RocksDB): absorbs indexing and multi-dimensional queries. The underlying DB receives flat offset requests.
- **Direct solution** for spatial-native domains (VR, LiDAR, digital twins, robotics): coordinate addressing replaces tables entirely.

Chton contains no domain model. It is not entangled with FIH, knowledge synthesis, or personal memory. These are concerns of the systems that consume Chton as infrastructure (nexus, Rem).

## Performance

p99 equals p50. Deterministic across all scales.

| Metric | CoordSpace |
|--------|-------|
| Single get (native) | 0.39 ns |
| Throughput (10K to 10M) | 21.5 ns flat |
| Prefix scan (nonexistent) | 1.65 ns |
| Multi-axis query | 94 ns AND |
| 125M keys memory | 119 MB fixed |
| Crash recovery | less than 5 ms (remap) |

## License

Tagma core: Apache 2.0 (open-core). Chton space IO layer and enterprise service components: commercial.

Chton for synTagma.
