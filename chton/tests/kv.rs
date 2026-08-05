use chton::kv::{KvError, CoordKVStore};
use chton::origin::{FileOrigin, MappedFileOrigin, MemoryOrigin};
use tagma_core::{Coord, CoordPath};
use tagma_kv::coord_cube_kv::CoordCubeKV;
use tagma_kv::coord_gen::CoordKey;
use tagma_kv::{CoordKV, CoordKVKey};

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

fn key(index: u16) -> CoordPath<1> {
    CoordPath::new([coord(index)])
}

fn mem_kv<const N: usize>() -> CoordKVStore<N> {
    CoordKVStore::new(Box::new(MemoryOrigin::new()), 64)
}

#[test]
fn round_trip_memory() {
    let mut kv = mem_kv::<1>();

    assert!(kv.get_path(&key(0)).unwrap().is_none());

    assert!(kv.put_path(&key(0), b"zero").unwrap().is_none());
    assert!(kv.put_path(&key(1), b"one").unwrap().is_none());
    assert_eq!(kv.get_path(&key(0)).unwrap().as_deref(), Some(&b"zero"[..]));
    assert_eq!(kv.get_path(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));

    // Overwrite returns the previous value and replaces it in place.
    let prev = kv.put_path(&key(0), b"ZERO").unwrap();
    assert_eq!(prev.as_deref(), Some(&b"zero"[..]));
    assert_eq!(kv.get_path(&key(0)).unwrap().as_deref(), Some(&b"ZERO"[..]));

    let removed = kv.remove_path(&key(0)).unwrap();
    assert_eq!(removed.as_deref(), Some(&b"ZERO"[..]));
    assert!(kv.get_path(&key(0)).unwrap().is_none());
    assert_eq!(kv.get_path(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));

    assert_eq!(kv.len(), 1);
}

#[test]
fn value_too_large() {
    let mut kv = mem_kv::<1>();
    let err = kv.put_path(&key(0), &[0u8; 57]).unwrap_err();
    assert!(matches!(
        err,
        KvError::ValueTooLarge {
            value_len: 57,
            max_len: 56
        }
    ));
}

#[test]
fn depth_six_round_trip() {
    let mut kv = mem_kv::<6>();
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
fn persists_across_file_reopen() {
    // Stage 1 acceptance: insert into a depth-6 store, persist, reopen,
    // and read the value back.
    let path = std::env::temp_dir().join(format!("chton-kv-n6-{}.bin", std::process::id()));
    let path2 = path.clone();
    {
        let origin = Box::new(FileOrigin::open(&path).unwrap());
        let mut kv = CoordKVStore::<6>::new(origin, 64);
        let ck = CoordKey::new(*b"abcdef");
        assert!(
            kv.insert_by_coordkey(&ck, b"persisted-value".to_vec())
                .is_none()
        );
        assert_eq!(kv.len(), 1);
        kv.flush().unwrap();
    }
    {
        let origin = Box::new(FileOrigin::open(&path2).unwrap());
        let kv = CoordKVStore::<6>::load(origin, 64).unwrap();
        let ck = CoordKey::new(*b"abcdef");
        assert_eq!(
            kv.get_by_coordkey(&ck).as_deref(),
            Some(&b"persisted-value"[..])
        );
        assert_eq!(kv.len(), 1);
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn len_survives_reopen() {
    let path = std::env::temp_dir().join(format!("chton-kv-len-{}.bin", std::process::id()));
    let path2 = path.clone();
    {
        let origin = Box::new(FileOrigin::open(&path).unwrap());
        let mut kv = CoordKVStore::<2>::new(origin, 64);
        kv.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
        kv.insert_by_coordkey(&CoordKey::new([3, 4]), b"b".to_vec());
        kv.insert_by_coordkey(&CoordKey::new([5, 6]), b"c".to_vec());
        kv.flush().unwrap();
    }
    {
        let origin = Box::new(FileOrigin::open(&path2).unwrap());
        let kv = CoordKVStore::<2>::load(origin, 64).unwrap();
        assert_eq!(kv.len(), 3);
        assert_eq!(
            kv.get_by_coordkey(&CoordKey::new([3, 4])).as_deref(),
            Some(&b"b"[..])
        );
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn coordkv_str_parity() {
    // Parity with the native CoordKVN: `insert` accepts exactly N-byte
    // keys, `get`/`remove` return None for other lengths.
    let mut kv = mem_kv::<3>();
    assert!(kv.insert("foo", b"bar".to_vec()).is_none());
    assert_eq!(kv.get("foo").as_deref(), Some(&b"bar"[..]));
    assert!(kv.get("fo").is_none());
    assert!(kv.get("food").is_none());
    assert!(kv.remove("fo").is_none());
    assert!(kv.contains_key("foo"));
}

#[test]
#[should_panic]
fn insert_with_wrong_key_length_panics() {
    let mut kv = mem_kv::<3>();
    kv.insert("toolong", b"x".to_vec());
}

#[test]
fn clear_resets() {
    let mut kv = mem_kv::<2>();
    kv.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
    kv.insert_by_coordkey(&CoordKey::new([3, 4]), b"b".to_vec());
    assert_eq!(kv.len(), 2);

    kv.clear();
    assert!(kv.is_empty());
    assert!(kv.get_by_coordkey(&CoordKey::new([1, 2])).is_none());

    // The store is usable after clear.
    kv.insert_by_coordkey(&CoordKey::new([7, 8]), b"c".to_vec());
    assert_eq!(
        kv.get_by_coordkey(&CoordKey::new([7, 8])).as_deref(),
        Some(&b"c"[..])
    );
}

#[test]
fn empty_value_round_trip() {
    // Presence is the record offset, not the length prefix: an empty
    // value is a stored entry, distinct from an absent key.
    let mut kv = mem_kv::<2>();
    assert!(
        kv.insert_by_coordkey(&CoordKey::new([1, 2]), Vec::new())
            .is_none()
    );
    assert_eq!(kv.get_by_coordkey(&CoordKey::new([1, 2])), Some(Vec::new()));
    assert_eq!(kv.len(), 1);
}

#[test]
fn persists_across_mapped_file_reopen() {
    // Flagship path end to end: CoordSpaceN (TreeStrategy) + kv
    // (CoordKVStore) over the mapped binding (MappedFileOrigin). Insert,
    // flush, reopen, and read the value back from the mapped file.
    let path = std::env::temp_dir().join(format!("chton-kv-n6-mapped-{}.bin", std::process::id()));
    let path2 = path.clone();
    {
        let origin = Box::new(MappedFileOrigin::open(&path).unwrap());
        let mut kv = CoordKVStore::<6>::new(origin, 64);
        let ck = CoordKey::new(*b"abcdef");
        assert!(
            kv.insert_by_coordkey(&ck, b"mapped-value".to_vec())
                .is_none()
        );
        assert_eq!(kv.len(), 1);
        kv.flush().unwrap();
    }
    {
        let origin = Box::new(MappedFileOrigin::open(&path2).unwrap());
        let kv = CoordKVStore::<6>::load(origin, 64).unwrap();
        let ck = CoordKey::new(*b"abcdef");
        assert_eq!(
            kv.get_by_coordkey(&ck).as_deref(),
            Some(&b"mapped-value"[..])
        );
        assert_eq!(kv.len(), 1);
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn buffered_state_tracks_header_dirty() {
    let mut kv = mem_kv::<2>();
    assert!(!kv.is_buffered());

    kv.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
    assert!(kv.is_buffered(), "a write leaves the header unsynced");

    kv.flush().unwrap();
    assert!(!kv.is_buffered(), "flush persists the header");
}

#[test]
fn iter_yields_entries() {
    let mut kv = mem_kv::<2>();
    kv.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
    kv.insert_by_coordkey(&CoordKey::new([3, 4]), b"b".to_vec());

    let entries = kv.iter().unwrap();
    assert_eq!(entries.len(), 2);
    // Coordinate-ascending order.
    assert_eq!(entries[0].0, CoordKey::new([1, 2]));
    assert_eq!(entries[0].1, b"a");
    assert_eq!(entries[1].0, CoordKey::new([3, 4]));
    assert_eq!(entries[1].1, b"b");
}

#[test]
fn proximity_finds_nearby() {
    // The CoordCube query primitive over the materialized store: entries
    // within L-infinity radius of a center path.
    let mut kv = mem_kv::<2>();
    let center = CoordKey::new([5, 5]);
    let nearby = CoordKey::new([5, 6]);
    let far = CoordKey::new([5, 20]);
    kv.insert_by_coordkey(&center, b"center".to_vec());
    kv.insert_by_coordkey(&nearby, b"nearby".to_vec());
    kv.insert_by_coordkey(&far, b"far".to_vec());

    let center_path = center.to_coord_path();
    let results = kv.proximity::<2, 1>(&center_path, 1);
    assert_eq!(results.len(), 2);
    let found: Vec<CoordPath<2>> = results.iter().map(|(p, _)| *p).collect();
    assert!(found.contains(&center_path));
    assert!(found.contains(&nearby.to_coord_path()));
}
