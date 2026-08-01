//! Usage scenario: key-value materialization over memory and file origins.
//!
//! Scenario steps:
//! 1. Bind a memory origin: values live in DRAM (volatile materialization).
//! 2. Bind a file origin: values persist across reopen (durable materialization).
//! 3. The same protocol surface runs over both destinations; only the
//!    capability matrix differs.

use chton::origin::{FileOrigin, MemoryOrigin, Origin};
use chton::protocol::kv::{KvStore, RegionKv};
use tagma_core::{Coord, CoordPath};

fn key(index: u16) -> CoordPath<1> {
    CoordPath::new([Coord::new(index).unwrap()])
}

fn main() {
    // Scenario 1: memory origin, volatile materialization.
    let mut mem = RegionKv::bind(MemoryOrigin::new(), 64);
    mem.put(&key(0), b"coordinate-zero").unwrap();
    mem.put(&key(1), b"coordinate-one").unwrap();
    println!(
        "memory: get(key0) = {:?}",
        mem.get(&key(0))
            .unwrap()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    );
    let _ = mem.remove(&key(1));
    println!(
        "memory: after remove, get(key1) = {:?}",
        mem.get(&key(1))
            .unwrap()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    );
    println!(
        "memory: capabilities = {:?}",
        mem.region().origin().capabilities()
    );

    // Scenario 2: file origin, durable materialization with reopen.
    let path = std::env::temp_dir().join("chton-kv-demo.bin");
    let _ = std::fs::remove_file(&path);
    {
        let mut file = RegionKv::bind(FileOrigin::open(&path).unwrap(), 64);
        file.put(&key(2), b"persisted-value").unwrap();
        file.region_mut().origin_mut().flush().unwrap();
        println!("file: put(key2) and flush");
        println!(
            "file: capabilities = {:?}",
            file.region().origin().capabilities()
        );
    }
    {
        let file = RegionKv::bind(FileOrigin::open(&path).unwrap(), 64);
        match file.get(&key(2)).unwrap() {
            Some(v) => println!(
                "file: reopened, get(key2) = {:?}",
                String::from_utf8_lossy(&v)
            ),
            None => println!("file: reopened, get(key2) = None (persistence lost)"),
        }
    }
    let _ = std::fs::remove_file(&path);

    println!("scenario complete: the same KV protocol materialized over DRAM and disk");
}
