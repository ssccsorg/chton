use chton::origin::{FileOrigin, MemoryOrigin, Origin};
use chton::protocol::kv::{KvError, KvStore, RegionKv};
use tagma_core::{Coord, CoordPath};

fn key(index: u16) -> CoordPath<1> {
    CoordPath::new([Coord::new(index).unwrap()])
}

#[test]
fn kv_round_trip_memory() {
    let mut kv = RegionKv::bind(MemoryOrigin::new(), 64);

    assert!(kv.get(&key(0)).unwrap().is_none());

    kv.put(&key(0), b"zero").unwrap();
    kv.put(&key(1), b"one").unwrap();
    assert_eq!(kv.get(&key(0)).unwrap().as_deref(), Some(&b"zero"[..]));
    assert_eq!(kv.get(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));

    kv.put(&key(0), b"ZERO").unwrap();
    assert_eq!(kv.get(&key(0)).unwrap().as_deref(), Some(&b"ZERO"[..]));

    kv.remove(&key(0)).unwrap();
    assert!(kv.get(&key(0)).unwrap().is_none());
    assert_eq!(kv.get(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));
}

#[test]
fn kv_value_too_large() {
    let mut kv = RegionKv::bind(MemoryOrigin::new(), 4);
    let err = kv.put(&key(0), b"toolarge").unwrap_err();
    assert!(matches!(
        err,
        KvError::ValueTooLarge {
            value_len: 8,
            max_len: 4
        }
    ));
}

#[test]
fn kv_persists_across_file_reopen() {
    let path = std::env::temp_dir().join(format!("chton-kv-{}.bin", std::process::id()));
    {
        let mut kv = RegionKv::bind(FileOrigin::open(&path).unwrap(), 32);
        kv.put(&key(3), b"persisted").unwrap();
        kv.region_mut().origin_mut().flush().unwrap();
    }
    {
        let kv = RegionKv::bind(FileOrigin::open(&path).unwrap(), 32);
        assert_eq!(
            kv.get(&key(3)).unwrap().as_deref(),
            Some(&b"persisted"[..])
        );
        assert!(kv.get(&key(4)).unwrap().is_none());
    }
    std::fs::remove_file(&path).unwrap();
}
