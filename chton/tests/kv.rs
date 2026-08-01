use chton::binding::TreeStrategy;
use chton::origin::{FileOrigin, MemoryOrigin};
use chton::protocol::kv::{KvError, RegionKv};
use tagma_core::{Coord, CoordPath};
use tagma_kv::{CoordKV, CoordKVKey};

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

fn key(index: u16) -> CoordPath<1> {
    CoordPath::new([coord(index)])
}

fn bind_mem(max_value_len: usize) -> RegionKv<MemoryOrigin, TreeStrategy<1>, 1> {
    let origin = MemoryOrigin::new();
    let strategy = TreeStrategy::<1>::load_or_new(&origin, 8 + max_value_len as u64).unwrap();
    RegionKv::bind(origin, strategy, max_value_len)
}

#[test]
fn kv_round_trip_memory() {
    let mut kv = bind_mem(64);

    assert!(kv.get_path(&key(0)).unwrap().is_none());

    kv.put_path(&key(0), b"zero").unwrap();
    kv.put_path(&key(1), b"one").unwrap();
    assert_eq!(kv.get_path(&key(0)).unwrap().as_deref(), Some(&b"zero"[..]));
    assert_eq!(kv.get_path(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));

    kv.put_path(&key(0), b"ZERO").unwrap();
    assert_eq!(kv.get_path(&key(0)).unwrap().as_deref(), Some(&b"ZERO"[..]));

    kv.remove_path(&key(0)).unwrap();
    assert!(kv.get_path(&key(0)).unwrap().is_none());
    assert_eq!(kv.get_path(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));
}

#[test]
fn kv_value_too_large() {
    let mut kv = bind_mem(4);
    let err = kv.put_path(&key(0), b"toolarge").unwrap_err();
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
    kv.put_path(&deep, b"deep-value").unwrap();
    assert_eq!(
        kv.get_path(&deep).unwrap().as_deref(),
        Some(&b"deep-value"[..])
    );

    let sibling = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(7)]);
    assert!(kv.get_path(&sibling).unwrap().is_none());
}

#[test]
fn kv_persists_across_file_reopen() {
    let path = std::env::temp_dir().join(format!("chton-kv-{}.bin", std::process::id()));
    {
        let origin = FileOrigin::open(&path).unwrap();
        let strategy = TreeStrategy::<1>::load_or_new(&origin, 8 + 32).unwrap();
        let mut kv = RegionKv::bind(origin, strategy, 32);
        kv.put_path(&key(3), b"persisted").unwrap();
        kv.flush().unwrap();
    }
    {
        let origin = FileOrigin::open(&path).unwrap();
        let strategy = TreeStrategy::<1>::load_or_new(&origin, 8 + 32).unwrap();
        let kv = RegionKv::bind(origin, strategy, 32);
        assert_eq!(
            kv.get_path(&key(3)).unwrap().as_deref(),
            Some(&b"persisted"[..])
        );
        assert!(kv.get_path(&key(4)).unwrap().is_none());
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn coord_kv_str_round_trip() {
    let mut kv = bind_mem(64);

    assert!(kv.get("a").is_none());

    kv.insert("a", b"alpha".to_vec());
    kv.insert("b", b"beta".to_vec());
    assert_eq!(kv.get("a").as_deref(), Some(&b"alpha"[..]));
    assert_eq!(kv.get("b").as_deref(), Some(&b"beta"[..]));
    assert!(kv.contains_key("a"));
    assert!(!kv.contains_key("c"));
    assert_eq!(kv.len(), 2);

    // insert returns the previous value
    let prev = kv.insert("a", b"ALPHA".to_vec());
    assert_eq!(prev.as_deref(), Some(&b"alpha"[..]));
    assert_eq!(kv.len(), 2);

    // remove returns the removed value
    let removed = kv.remove("a");
    assert_eq!(removed.as_deref(), Some(&b"ALPHA"[..]));
    assert!(kv.get("a").is_none());
    assert_eq!(kv.len(), 1);
}

#[test]
fn coord_kv_clear() {
    let mut kv = bind_mem(64);
    kv.insert("a", b"1".to_vec());
    kv.insert("b", b"2".to_vec());
    assert_eq!(kv.len(), 2);

    kv.clear();
    assert!(kv.is_empty());
    assert!(kv.get("a").is_none());
    assert!(kv.get("b").is_none());

    // the origin is reusable after clear
    kv.insert("c", b"3".to_vec());
    assert_eq!(kv.get("c").as_deref(), Some(&b"3"[..]));
    assert_eq!(kv.len(), 1);
}

#[test]
fn coord_kv_wrong_key_length_rejected() {
    let mut kv = bind_mem(64);
    // TreeStrategy<1> requires exactly 1-byte keys
    assert!(kv.insert("ab", b"x".to_vec()).is_none());
    assert!(kv.get("ab").is_none());
    assert!(kv.remove("ab").is_none());
}

#[test]
fn coord_kv_key_by_coordkey() {
    let mut kv = bind_mem(64);
    let ck = tagma_kv::coord_gen::CoordKey::<1>::new([b'z']);

    kv.insert_by_coordkey(&ck, b"zed".to_vec());
    assert_eq!(kv.get_by_coordkey(&ck).as_deref(), Some(&b"zed"[..]));
    assert!(kv.contains_key_by_coordkey(&ck));

    let removed = kv.remove_by_coordkey(&ck);
    assert_eq!(removed.as_deref(), Some(&b"zed"[..]));
    assert!(!kv.contains_key_by_coordkey(&ck));
}

#[test]
fn coord_kv_len_tracks_put_remove() {
    let mut kv = bind_mem(64);
    assert_eq!(kv.len(), 0);

    kv.put_path(&key(0), b"v").unwrap();
    assert_eq!(kv.len(), 1);
    kv.put_path(&key(0), b"v2").unwrap();
    assert_eq!(kv.len(), 1);
    kv.put_path(&key(1), b"w").unwrap();
    assert_eq!(kv.len(), 2);

    kv.remove_path(&key(0)).unwrap();
    assert_eq!(kv.len(), 1);
    kv.remove_path(&key(9)).unwrap(); // absent key is a no-op
    assert_eq!(kv.len(), 1);
}
