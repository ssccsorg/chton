use chton::binding::TreeStrategy;
use chton::origin::{FileOrigin, MemoryOrigin};
use chton::protocol::kv::{KvError, KvStore, RegionKv};
use tagma_core::{Coord, CoordPath};

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

fn key(index: u16) -> CoordPath<1> {
    CoordPath::new([coord(index)])
}

fn bind_mem(max_value_len: usize) -> RegionKv<MemoryOrigin, TreeStrategy<1>, 1> {
    let origin = MemoryOrigin::new();
    let strategy =
        TreeStrategy::<1>::load_or_new(&origin, 8 + max_value_len as u64).unwrap();
    RegionKv::bind(origin, strategy, max_value_len)
}

#[test]
fn kv_round_trip_memory() {
    let mut kv = bind_mem(64);

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
    let mut kv = bind_mem(4);
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
fn kv_depth_six_round_trip() {
    let origin = MemoryOrigin::new();
    let strategy = TreeStrategy::<6>::load_or_new(&origin, 8 + 32).unwrap();
    let mut kv = RegionKv::bind(origin, strategy, 32);

    let deep = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(6)]);
    kv.put(&deep, b"deep-value").unwrap();
    assert_eq!(
        kv.get(&deep).unwrap().as_deref(),
        Some(&b"deep-value"[..])
    );

    let sibling = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(7)]);
    assert!(kv.get(&sibling).unwrap().is_none());
}

#[test]
fn kv_persists_across_file_reopen() {
    let path = std::env::temp_dir().join(format!("chton-kv-{}.bin", std::process::id()));
    {
        let origin = FileOrigin::open(&path).unwrap();
        let strategy = TreeStrategy::<1>::load_or_new(&origin, 8 + 32).unwrap();
        let mut kv = RegionKv::bind(origin, strategy, 32);
        kv.put(&key(3), b"persisted").unwrap();
        kv.flush().unwrap();
    }
    {
        let origin = FileOrigin::open(&path).unwrap();
        let strategy = TreeStrategy::<1>::load_or_new(&origin, 8 + 32).unwrap();
        let kv = RegionKv::bind(origin, strategy, 32);
        assert_eq!(
            kv.get(&key(3)).unwrap().as_deref(),
            Some(&b"persisted"[..])
        );
        assert!(kv.get(&key(4)).unwrap().is_none());
    }
    std::fs::remove_file(&path).unwrap();
}
