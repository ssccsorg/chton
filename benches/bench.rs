// ═══════════════════════════════════════════════════════════════════════════
// chton-bench — materialization benchmark suite (origin / binding / kv / store)
//
// Single-file Criterion benchmark, the stack standard (syntagma / nexus / ev).
// Run:
//   cargo bench -p chton-bench          # everything
//   cargo bench -p chton-bench -- kv    # kv group only
//   ./run.sh                            # timestamped capture + JSON export
//
// Coverage:
//   origin/   — byte-level bindings: MemoryOrigin vs FileOrigin vs MappedFileOrigin
//   binding/  — TreeStrategy: address resolution (locate), creation, enumeration
//   kv/       — CoordKVStore: put / get / remove / iter / flush
//   kv/reopen/— reopen via the persisted header count (scale-invariant)
//   spatial/  — CoordCube proximity (L-infinity box) at radius scaling
//   store/    — EntityStore family: Memory vs Coord vs Kv
//
// Paradigm (the master invariant this suite is written against):
//   tagma replaces indexing: the coordinate is the address. Resolution is
//   immediate (per-level array indexing, O(depth)) and enumeration is
//   proportional to materialized records, never to the address space. Two
//   layers must stay separate: the indexing layer (coordinate -> address)
//   and the storage layer (the linked tree over the origin). An
//   engineering-layer cost of the methodology that breaks through hardware
//   constraints (the 11,172-wide fan-out) is inherent and acceptable;
//   operations carry that constant. Degradation that arises from mixing the
//   layers (e.g. answering a count by walking the tree) is a wrong
//   implementation. The numbers below must not regress with the address
//   space or with record count beyond proportionality.
//
// Measured on Apple M-series (ARMv8.4-A Firestorm), release profile,
// criterion (median of 10 samples), 2026-08-06:
//   origin/memory_write_1k      3.39 µs      (1k x 8-byte writes)
//   origin/memory_read_1k       3.86 µs
//   origin/file_write_1k        1.98 ms      (seek + write syscalls)
//   origin/file_read_1k         1.13 ms
//   origin/mapped_write_1k      4.33 µs
//   origin/mapped_read_1k       3.75 µs
//   binding/locate_hit_1k       68.4 µs      (~68 ns/lookup, O(depth))
//   binding/locate_miss_1k      29.5 ns      (full-depth miss)
//   binding/locate_or_create_1k 52.8 ms      (node alloc dominates)
//   binding/iter_1k             38.5 ms      (O(nodes + records), bitmap)
//   binding/count_1k            45.2 ms
//   kv/put_1k                   56.0 ms      (~56 µs/insert, node alloc)
//   kv/get_1k                   133 µs       (~133 ns/lookup)
//   kv/remove_1k                56.3 ms
//   kv/iter_1k                  39.0 ms
//   kv/flush_1k                 10.4 ms
//   kv/reopen/n1000             18.0 µs      (O(1), persisted header count)
//   kv/reopen/n10000            17.4 µs      (scale-invariant: 1k == 10k)
//   spatial/proximity_r1_1k     1.91 µs      (box 3^6 = 729 paths)
//   spatial/proximity_r2_1k     30.9 µs      (box 5^6 = 15,625 paths)
//   spatial/proximity_r3_1k     115 µs       (box 7^6 = 117,649 paths)
//   store/memory_insert_1k      145 µs
//   store/coord_insert_1k       91.1 ms      (CoordSpaceN node alloc)
//   store/kv_insert_1k          59.1 ms
//   store/kv_values_1k          38.1 ms
//   store/kv_proximity_r2_1k    27.1 µs
//
// Reading the ledger:
//   Resolution (locate/get) is O(depth) and independent of record count;
//   the per-lookup cost is a few tens of nanoseconds at depth 6. Reopen is
//   O(1): n1000 and n10000 land at the same 17-18 µs. Enumeration (iter /
//   count / values) is proportional to materialized nodes and records. The
//   heavy insert costs (put, locate_or_create, coord/kv insert) are node
//   allocation: each new node zeroes its 89 KB span, the engineering cost
//   of the 11,172-wide direct-addressing fan-out (allowed by the
//   invariant). Proximity cost is the box volume (2r+1)^N per query.
//
// Correctness guards:
//   kv/reopen/* asserts len == n after every reopen; a regression in the
//   header-count path fails the benchmark.
// ═══════════════════════════════════════════════════════════════════════════

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use futures_executor::block_on;

use chton::binding::{SpaceStrategy, TreeStrategy};
use chton::kv::CoordKVStore;
use chton::origin::{FileOrigin, MappedFileOrigin, MemoryOrigin, Origin};
use chton::store::{CoordEntityStore, EntityStore, KvEntityStore, MemoryEntityStore};
use tagma_core::{Coord, CoordPath};
use tagma_kv::CoordKV;
use tagma_kv::coord_cube_kv::CoordCubeKV;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const RECORDS: usize = 1_000;

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

/// Base-11172 digits of `i`: distinct full-depth coordinate paths. The
/// tree is a forest of disjoint depth-6 chains, the sparse-random worst
/// case for node count (about 6 nodes per record).
fn path_of(i: usize) -> CoordPath<6> {
    let mut coords = [coord(0); 6];
    let mut rem = i as u64;
    for c in coords.iter_mut() {
        *c = coord((rem % 11172) as u16);
        rem /= 11172;
    }
    CoordPath::new(coords)
}

fn paths(n: usize) -> Vec<CoordPath<6>> {
    (0..n).map(path_of).collect()
}

fn temp_file(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("chton-bench-{tag}-{}.bin", std::process::id()))
}

// ===========================================================================
// origin/ — byte-level bindings
// ===========================================================================

fn bench_origin_memory_write(c: &mut Criterion) {
    let mut origin = MemoryOrigin::new();
    c.bench_function("origin/memory_write_1k", |b| {
        b.iter(|| {
            for i in 0..1_000u64 {
                origin.write(i * 8, &[0xAB; 8]).unwrap();
            }
            black_box(&origin);
        })
    });
}

fn bench_origin_memory_read(c: &mut Criterion) {
    let mut origin = MemoryOrigin::new();
    for i in 0..1_000u64 {
        origin.write(i * 8, &[0xAB; 8]).unwrap();
    }
    let mut buf = [0u8; 8];
    c.bench_function("origin/memory_read_1k", |b| {
        b.iter(|| {
            for i in 0..1_000u64 {
                black_box(origin.read(i * 8, &mut buf).unwrap());
            }
        })
    });
}

fn bench_origin_file_write(c: &mut Criterion) {
    let file = temp_file("file-write");
    let mut origin = FileOrigin::open(&file).unwrap();
    c.bench_function("origin/file_write_1k", |b| {
        b.iter(|| {
            for i in 0..1_000u64 {
                origin.write(i * 8, &[0xAB; 8]).unwrap();
            }
            black_box(&origin);
        })
    });
    std::fs::remove_file(&file).ok();
}

fn bench_origin_file_read(c: &mut Criterion) {
    let file = temp_file("file-read");
    {
        let mut origin = FileOrigin::open(&file).unwrap();
        for i in 0..1_000u64 {
            origin.write(i * 8, &[0xAB; 8]).unwrap();
        }
    }
    let origin = FileOrigin::open(&file).unwrap();
    let mut buf = [0u8; 8];
    c.bench_function("origin/file_read_1k", |b| {
        b.iter(|| {
            for i in 0..1_000u64 {
                black_box(origin.read(i * 8, &mut buf).unwrap());
            }
        })
    });
    std::fs::remove_file(&file).ok();
}

fn bench_origin_mapped_write(c: &mut Criterion) {
    let file = temp_file("mapped-write");
    let mut origin = MappedFileOrigin::open(&file).unwrap();
    c.bench_function("origin/mapped_write_1k", |b| {
        b.iter(|| {
            for i in 0..1_000u64 {
                origin.write(i * 8, &[0xAB; 8]).unwrap();
            }
            black_box(&origin);
        })
    });
    std::fs::remove_file(&file).ok();
}

fn bench_origin_mapped_read(c: &mut Criterion) {
    let file = temp_file("mapped-read");
    {
        let mut origin = MappedFileOrigin::open(&file).unwrap();
        for i in 0..1_000u64 {
            origin.write(i * 8, &[0xAB; 8]).unwrap();
        }
    }
    let origin = MappedFileOrigin::open(&file).unwrap();
    let mut buf = [0u8; 8];
    c.bench_function("origin/mapped_read_1k", |b| {
        b.iter(|| {
            for i in 0..1_000u64 {
                black_box(origin.read(i * 8, &mut buf).unwrap());
            }
        })
    });
    std::fs::remove_file(&file).ok();
}

// ===========================================================================
// binding/ — TreeStrategy
// ===========================================================================

fn build_tree(n: usize) -> (TreeStrategy<6>, MemoryOrigin) {
    let mut origin = MemoryOrigin::new();
    let mut strategy = TreeStrategy::<6>::new(64);
    for path in paths(n) {
        let slot = strategy.locate_or_create(&mut origin, &path).unwrap();
        let rec = strategy.alloc_record(&mut origin).unwrap();
        strategy.write_leaf(&mut origin, &slot, rec).unwrap();
    }
    (strategy, origin)
}

fn bench_binding_locate_hit(c: &mut Criterion) {
    let (strategy, origin) = build_tree(RECORDS);
    let ps = paths(RECORDS);
    c.bench_function("binding/locate_hit_1k", |b| {
        b.iter(|| {
            for p in &ps {
                black_box(strategy.locate(&origin, p).unwrap());
            }
        })
    });
}

/// A full-depth miss: [7, 0, 0, 0, 0, 1] shares the first five levels
/// with the inserted records and diverges at the leaf, so resolution
/// walks the full depth before reporting absence.
fn bench_binding_locate_miss(c: &mut Criterion) {
    let (strategy, origin) = build_tree(RECORDS);
    let miss = CoordPath::new([coord(7), coord(0), coord(0), coord(0), coord(0), coord(1)]);
    c.bench_function("binding/locate_miss_1k", |b| {
        b.iter(|| black_box(strategy.locate(&origin, &miss).unwrap()))
    });
}

fn bench_binding_locate_or_create(c: &mut Criterion) {
    c.bench_function("binding/locate_or_create_1k", |b| {
        b.iter(|| {
            let (mut strategy, mut origin) = (TreeStrategy::<6>::new(64), MemoryOrigin::new());
            for p in paths(RECORDS) {
                black_box(strategy.locate_or_create(&mut origin, &p).unwrap());
            }
        })
    });
}

fn bench_binding_iter(c: &mut Criterion) {
    let (strategy, origin) = build_tree(RECORDS);
    c.bench_function("binding/iter_1k", |b| {
        b.iter(|| black_box(strategy.iter(&origin).unwrap()))
    });
}

fn bench_binding_count(c: &mut Criterion) {
    let (strategy, origin) = build_tree(RECORDS);
    c.bench_function("binding/count_1k", |b| {
        b.iter(|| black_box(strategy.count_records(&origin).unwrap()))
    });
}

// ===========================================================================
// kv/ — CoordKVStore
// ===========================================================================

fn build_kv(n: usize) -> CoordKVStore<6> {
    let mut kv = CoordKVStore::<6>::new(Box::new(MemoryOrigin::new()), 64);
    for p in paths(n) {
        kv.put_path(&p, b"value").unwrap();
    }
    kv
}

fn bench_kv_put(c: &mut Criterion) {
    let ps = paths(RECORDS);
    c.bench_function("kv/put_1k", |b| {
        b.iter(|| {
            let mut kv = CoordKVStore::<6>::new(Box::new(MemoryOrigin::new()), 64);
            for p in &ps {
                kv.put_path(p, b"value").unwrap();
            }
            black_box(kv);
        })
    });
}

fn bench_kv_get(c: &mut Criterion) {
    let kv = build_kv(RECORDS);
    let ps = paths(RECORDS);
    c.bench_function("kv/get_1k", |b| {
        b.iter(|| {
            for p in &ps {
                black_box(kv.get_path(p).unwrap());
            }
        })
    });
}

fn bench_kv_remove(c: &mut Criterion) {
    let ps = paths(RECORDS);
    c.bench_function("kv/remove_1k", |b| {
        b.iter(|| {
            let mut kv = build_kv(RECORDS);
            for p in &ps {
                kv.remove_path(p).unwrap();
            }
        })
    });
}

fn bench_kv_iter(c: &mut Criterion) {
    let kv = build_kv(RECORDS);
    c.bench_function("kv/iter_1k", |b| b.iter(|| black_box(kv.iter().unwrap())));
}

fn bench_kv_flush(c: &mut Criterion) {
    let file = temp_file("flush");
    c.bench_function("kv/flush_1k", |b| {
        b.iter(|| {
            let origin = Box::new(FileOrigin::open(&file).unwrap());
            let mut kv = CoordKVStore::<6>::new(origin, 64);
            for p in paths(100) {
                kv.put_path(&p, b"value").unwrap();
            }
            kv.flush().unwrap();
        })
    });
    std::fs::remove_file(&file).ok();
}

// ---------------------------------------------------------------------------
// kv/reopen/ — the header-count invariant
// ---------------------------------------------------------------------------

/// Reopen reads the persisted record count from the header, so the load
/// must not scale with the record count: n1k and n10k land in the same
/// range, never 10x apart. A regression here is a layer-mixing error
/// (walking the tree to count records on reopen).
fn reopen(c: &mut Criterion, n: usize) {
    let file = temp_file(&format!("reopen-{n}"));
    {
        let origin = Box::new(FileOrigin::open(&file).unwrap());
        let mut kv = CoordKVStore::<6>::new(origin, 64);
        for p in paths(n) {
            kv.put_path(&p, b"value").unwrap();
        }
        kv.flush().unwrap();
    }
    c.bench_function(&format!("kv/reopen/n{n}"), |b| {
        b.iter(|| {
            let origin = Box::new(FileOrigin::open(&file).unwrap());
            let kv = CoordKVStore::<6>::load(origin, 64).unwrap();
            assert_eq!(kv.len(), n, "reopen must restore the record count");
            black_box(kv.len());
        })
    });
    std::fs::remove_file(&file).ok();
}

fn bench_kv_reopen_1k(c: &mut Criterion) {
    reopen(c, 1_000);
}

fn bench_kv_reopen_10k(c: &mut Criterion) {
    reopen(c, 10_000);
}

// ===========================================================================
// spatial/ — CoordCube proximity
// ===========================================================================

fn bench_spatial_proximity(c: &mut Criterion) {
    let kv = build_kv(RECORDS);
    let center = path_of(42);
    for (name, radius) in [("r1", 1usize), ("r2", 2), ("r3", 3)] {
        c.bench_function(&format!("spatial/proximity_{name}_1k"), |b| {
            b.iter(|| black_box(kv.proximity::<2, 3>(&center, radius)))
        });
    }
}

// ===========================================================================
// store/ — EntityStore family
// ===========================================================================

fn bench_store_memory_insert(c: &mut Criterion) {
    c.bench_function("store/memory_insert_1k", |b| {
        b.iter(|| {
            let store = MemoryEntityStore::<u64>::new();
            block_on(async {
                for i in 0..RECORDS as u64 {
                    store.insert(format!("k{i}"), i).await;
                }
            });
            black_box(&store);
        })
    });
}

fn bench_store_coord_insert(c: &mut Criterion) {
    c.bench_function("store/coord_insert_1k", |b| {
        b.iter(|| {
            let store = CoordEntityStore::<6, u64>::new();
            block_on(async {
                for i in 0..RECORDS as u64 {
                    store.insert(format!("k{i}"), i).await;
                }
            });
            black_box(&store);
        })
    });
}

fn bench_store_kv_insert(c: &mut Criterion) {
    c.bench_function("store/kv_insert_1k", |b| {
        b.iter(|| {
            let store = KvEntityStore::<6, u64>::new(Box::new(MemoryOrigin::new()), 64);
            block_on(async {
                for i in 0..RECORDS as u64 {
                    store.insert(format!("k{i}"), i).await;
                }
            });
            black_box(&store);
        })
    });
}

fn bench_store_kv_values(c: &mut Criterion) {
    let store = KvEntityStore::<6, u64>::new(Box::new(MemoryOrigin::new()), 64);
    block_on(async {
        for i in 0..RECORDS as u64 {
            store.insert(format!("k{i}"), i).await;
        }
    });
    c.bench_function("store/kv_values_1k", |b| {
        b.iter(|| black_box(block_on(store.values())))
    });
}

fn bench_store_kv_proximity(c: &mut Criterion) {
    let store = KvEntityStore::<6, u64>::new(Box::new(MemoryOrigin::new()), 64);
    block_on(async {
        for i in 0..RECORDS as u64 {
            store.insert(format!("k{i}"), i).await;
        }
    });
    let center = CoordPath::new([coord(5), coord(5), coord(0), coord(0), coord(0), coord(0)]);
    c.bench_function("store/kv_proximity_r2_1k", |b| {
        b.iter(|| black_box(store.proximity::<2, 3>(&center, 2)))
    });
}

// ===========================================================================
// Harness
// ===========================================================================

criterion_group!(
    benches,
    bench_origin_memory_write,
    bench_origin_memory_read,
    bench_origin_file_write,
    bench_origin_file_read,
    bench_origin_mapped_write,
    bench_origin_mapped_read,
    bench_binding_locate_hit,
    bench_binding_locate_miss,
    bench_binding_locate_or_create,
    bench_binding_iter,
    bench_binding_count,
    bench_kv_put,
    bench_kv_get,
    bench_kv_remove,
    bench_kv_iter,
    bench_kv_flush,
    bench_kv_reopen_1k,
    bench_kv_reopen_10k,
    bench_spatial_proximity,
    bench_store_memory_insert,
    bench_store_coord_insert,
    bench_store_kv_insert,
    bench_store_kv_values,
    bench_store_kv_proximity,
);
criterion_main!(benches);
