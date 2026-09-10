use chton::map::{CoordMapStore, MapError};
use chton::origin::{FileOrigin, MappedFileOrigin, MemoryOrigin};
use tagma_core::{Coord, CoordPath};
use tagma_map::coord_cube_map::CoordCubeMap;
use tagma_map::coord_gen::CoordKey;
use tagma_map::{CoordMap, CoordMapKey};

fn coord(index: u16) -> Coord {
    Coord::new(index).unwrap()
}

fn key(index: u16) -> CoordPath<1> {
    CoordPath::new([coord(index)])
}

fn mem_map<const N: usize>() -> CoordMapStore<N> {
    CoordMapStore::new(Box::new(MemoryOrigin::new()), 64)
}

#[test]
fn round_trip_memory() {
    let mut map = mem_map::<1>();

    assert!(map.get_path(&key(0)).unwrap().is_none());

    assert!(map.put_path(&key(0), b"zero").unwrap().is_none());
    assert!(map.put_path(&key(1), b"one").unwrap().is_none());
    assert_eq!(
        map.get_path(&key(0)).unwrap().as_deref(),
        Some(&b"zero"[..])
    );
    assert_eq!(map.get_path(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));

    // Overwrite returns the previous value and replaces it in place.
    let prev = map.put_path(&key(0), b"ZERO").unwrap();
    assert_eq!(prev.as_deref(), Some(&b"zero"[..]));
    assert_eq!(
        map.get_path(&key(0)).unwrap().as_deref(),
        Some(&b"ZERO"[..])
    );

    let removed = map.remove_path(&key(0)).unwrap();
    assert_eq!(removed.as_deref(), Some(&b"ZERO"[..]));
    assert!(map.get_path(&key(0)).unwrap().is_none());
    assert_eq!(map.get_path(&key(1)).unwrap().as_deref(), Some(&b"one"[..]));

    assert_eq!(map.len(), 1);
}

#[test]
fn value_too_large() {
    let mut map = mem_map::<1>();
    let err = map.put_path(&key(0), &[0u8; 57]).unwrap_err();
    assert!(matches!(
        err,
        MapError::ValueTooLarge {
            value_len: 57,
            max_len: 56
        }
    ));
}

#[test]
fn depth_six_round_trip() {
    let mut map = mem_map::<6>();
    let deep = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(6)]);
    map.put_path(&deep, b"deep-value").unwrap();
    assert_eq!(
        map.get_path(&deep).unwrap().as_deref(),
        Some(&b"deep-value"[..])
    );

    let sibling = CoordPath::new([coord(1), coord(2), coord(3), coord(4), coord(5), coord(7)]);
    assert!(map.get_path(&sibling).unwrap().is_none());
}

#[test]
fn persists_across_file_reopen() {
    // Stage 1 acceptance: insert into a depth-6 store, persist, reopen,
    // and read the value back.
    let path = std::env::temp_dir().join(format!("chton-map-n6-{}.bin", std::process::id()));
    let path2 = path.clone();
    {
        let origin = Box::new(FileOrigin::open(&path).unwrap());
        let mut map = CoordMapStore::<6>::new(origin, 64);
        let ck = CoordKey::new(*b"abcdef");
        assert!(
            map.insert_by_coordkey(&ck, b"persisted-value".to_vec())
                .is_none()
        );
        assert_eq!(map.len(), 1);
        map.flush().unwrap();
    }
    {
        let origin = Box::new(FileOrigin::open(&path2).unwrap());
        let map = CoordMapStore::<6>::load(origin, 64).unwrap();
        let ck = CoordKey::new(*b"abcdef");
        assert_eq!(
            map.get_by_coordkey(&ck).as_deref(),
            Some(&b"persisted-value"[..])
        );
        assert_eq!(map.len(), 1);
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn len_survives_reopen() {
    let path = std::env::temp_dir().join(format!("chton-map-len-{}.bin", std::process::id()));
    let path2 = path.clone();
    {
        let origin = Box::new(FileOrigin::open(&path).unwrap());
        let mut map = CoordMapStore::<2>::new(origin, 64);
        map.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
        map.insert_by_coordkey(&CoordKey::new([3, 4]), b"b".to_vec());
        map.insert_by_coordkey(&CoordKey::new([5, 6]), b"c".to_vec());
        map.flush().unwrap();
    }
    {
        let origin = Box::new(FileOrigin::open(&path2).unwrap());
        let map = CoordMapStore::<2>::load(origin, 64).unwrap();
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get_by_coordkey(&CoordKey::new([3, 4])).as_deref(),
            Some(&b"b"[..])
        );
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn coordmap_str_parity() {
    // Parity with the native CoordMapN: `insert` accepts exactly N-byte
    // keys, `get`/`remove` return None for other lengths.
    let mut map = mem_map::<3>();
    assert!(map.insert("foo", b"bar".to_vec()).is_none());
    assert_eq!(map.get("foo").as_deref(), Some(&b"bar"[..]));
    assert!(map.get("fo").is_none());
    assert!(map.get("food").is_none());
    assert!(map.remove("fo").is_none());
    assert!(map.contains_key("foo"));
}

#[test]
#[should_panic]
fn insert_with_wrong_key_length_panics() {
    let mut map = mem_map::<3>();
    map.insert("toolong", b"x".to_vec());
}

#[test]
fn clear_resets() {
    let mut map = mem_map::<2>();
    map.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
    map.insert_by_coordkey(&CoordKey::new([3, 4]), b"b".to_vec());
    assert_eq!(map.len(), 2);

    map.clear();
    assert!(map.is_empty());
    assert!(map.get_by_coordkey(&CoordKey::new([1, 2])).is_none());

    // The store is usable after clear.
    map.insert_by_coordkey(&CoordKey::new([7, 8]), b"c".to_vec());
    assert_eq!(
        map.get_by_coordkey(&CoordKey::new([7, 8])).as_deref(),
        Some(&b"c"[..])
    );
}

#[test]
fn empty_value_round_trip() {
    // Presence is the record offset, not the length prefix: an empty
    // value is a stored entry, distinct from an absent key.
    let mut map = mem_map::<2>();
    assert!(
        map.insert_by_coordkey(&CoordKey::new([1, 2]), Vec::new())
            .is_none()
    );
    assert_eq!(
        map.get_by_coordkey(&CoordKey::new([1, 2])),
        Some(Vec::new())
    );
    assert_eq!(map.len(), 1);
}

#[test]
fn persists_across_mapped_file_reopen() {
    // Flagship path end to end: CoordSpaceN (TreeStrategy) + map
    // (CoordMapStore) over the mapped binding (MappedFileOrigin). Insert,
    // flush, reopen, and read the value back from the mapped file.
    let path = std::env::temp_dir().join(format!("chton-map-n6-mapped-{}.bin", std::process::id()));
    let path2 = path.clone();
    {
        let origin = Box::new(MappedFileOrigin::open(&path).unwrap());
        let mut map = CoordMapStore::<6>::new(origin, 64);
        let ck = CoordKey::new(*b"abcdef");
        assert!(
            map.insert_by_coordkey(&ck, b"mapped-value".to_vec())
                .is_none()
        );
        assert_eq!(map.len(), 1);
        map.flush().unwrap();
    }
    {
        let origin = Box::new(MappedFileOrigin::open(&path2).unwrap());
        let map = CoordMapStore::<6>::load(origin, 64).unwrap();
        let ck = CoordKey::new(*b"abcdef");
        assert_eq!(
            map.get_by_coordkey(&ck).as_deref(),
            Some(&b"mapped-value"[..])
        );
        assert_eq!(map.len(), 1);
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn buffered_state_tracks_header_dirty() {
    let mut map = mem_map::<2>();
    assert!(!map.is_buffered());

    map.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
    assert!(map.is_buffered(), "a write leaves the header unsynced");

    map.flush().unwrap();
    assert!(!map.is_buffered(), "flush persists the header");
}

#[test]
fn iter_yields_entries() {
    let mut map = mem_map::<2>();
    map.insert_by_coordkey(&CoordKey::new([1, 2]), b"a".to_vec());
    map.insert_by_coordkey(&CoordKey::new([3, 4]), b"b".to_vec());

    let entries = map.iter().unwrap();
    assert_eq!(entries.len(), 2);
    // Coordinate-ascending order.
    assert_eq!(entries[0].0, CoordKey::new([1, 2]));
    assert_eq!(entries[0].1, b"a");
    assert_eq!(entries[1].0, CoordKey::new([3, 4]));
    assert_eq!(entries[1].1, b"b");
}
#[test]
fn iter_projects_full_range_paths_onto_byte_keys() {
    // CoordMapStore addresses the full Coord index domain, while the
    // CoordKey output surface is byte-space (0..255 per character).
    // Stored paths whose coordinates are at or above 256 must project
    // onto their low bytes without CoordKey::from_coord_path, which
    // rejects such indices (syntagma issue #59).
    let mut map = mem_map::<2>();
    let high = CoordPath::new([coord(5586), coord(256)]);
    map.put_path(&high, b"high-value").unwrap();
    assert_eq!(map.len(), 1);

    let entries = map.iter().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, CoordKey::new([coord(5586).index() as u8, 0]));
    assert_eq!(entries[0].1, b"high-value");

    // The path-based API still resolves the full-range entry.
    assert_eq!(
        map.get_path(&high).unwrap().as_deref(),
        Some(&b"high-value"[..])
    );
}

#[test]
fn iter_projection_is_lossy_across_the_byte_domain() {
    // Index 0 and index 256 both project onto byte 0. The projection exists
    // for the tagma-map key surface; the path API stays authoritative.
    let mut map = mem_map::<2>();
    let low = CoordPath::new([coord(0), coord(0)]);
    let high = CoordPath::new([coord(256), coord(256)]);
    map.put_path(&low, b"low").unwrap();
    map.put_path(&high, b"high").unwrap();
    assert_eq!(map.len(), 2);

    let entries = map.iter().unwrap();
    assert_eq!(entries.len(), 2);
    for (key, _) in &entries {
        assert_eq!(*key, CoordKey::new([0, 0]), "both paths project to byte 0");
    }
    let mut values: Vec<Vec<u8>> = entries.into_iter().map(|(_, value)| value).collect();
    values.sort();
    assert_eq!(values, vec![b"high".to_vec(), b"low".to_vec()]);

    // The path-based API keeps the two entries distinct.
    assert_eq!(map.get_path(&low).unwrap().as_deref(), Some(&b"low"[..]));
    assert_eq!(map.get_path(&high).unwrap().as_deref(), Some(&b"high"[..]));
}

#[test]
fn proximity_finds_full_range_paths() {
    // CoordMapStore addresses the full Coord index domain, so the
    // CoordCubeMap query over it must not be limited to byte keys.
    let mut map = mem_map::<2>();
    let center = CoordPath::new([coord(300), coord(300)]);
    let near = CoordPath::new([coord(300), coord(301)]);
    let far = CoordPath::new([coord(300), coord(320)]);
    map.put_path(&center, b"center").unwrap();
    map.put_path(&near, b"near").unwrap();
    map.put_path(&far, b"far").unwrap();

    let results = map.proximity::<2, 1>(&center, 1);
    let mut values: Vec<Vec<u8>> = results.into_iter().map(|(_, value)| value).collect();
    values.sort();
    assert_eq!(values, vec![b"center".to_vec(), b"near".to_vec()]);
}

#[test]
fn full_range_keys_survive_file_reopen() {
    let path = std::env::temp_dir().join(format!("chton-map-fullrange-{}.bin", std::process::id()));
    let path2 = path.clone();
    let high = CoordPath::new([coord(5586), coord(256)]);
    {
        let origin = Box::new(FileOrigin::open(&path).unwrap());
        let mut map = CoordMapStore::<2>::new(origin, 64);
        map.put_path(&high, b"high-value").unwrap();
        map.flush().unwrap();
    }
    {
        let origin = Box::new(FileOrigin::open(&path2).unwrap());
        let map = CoordMapStore::<2>::load(origin, 64).unwrap();
        assert_eq!(
            map.get_path(&high).unwrap().as_deref(),
            Some(&b"high-value"[..])
        );
        let entries = map.iter().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, CoordKey::new([coord(5586).index() as u8, 0]));
    }
    std::fs::remove_file(&path).unwrap();
}

#[cfg(unix)]
#[test]
fn full_range_keys_survive_mapped_reopen() {
    let path = std::env::temp_dir().join(format!(
        "chton-map-fullrange-mapped-{}.bin",
        std::process::id()
    ));
    let path2 = path.clone();
    let high = CoordPath::new([coord(5586), coord(256)]);
    {
        let origin = Box::new(MappedFileOrigin::open(&path).unwrap());
        let mut map = CoordMapStore::<2>::new(origin, 64);
        map.put_path(&high, b"mapped-value").unwrap();
        map.flush().unwrap();
    }
    {
        let origin = Box::new(MappedFileOrigin::open(&path2).unwrap());
        let map = CoordMapStore::<2>::load(origin, 64).unwrap();
        assert_eq!(
            map.get_path(&high).unwrap().as_deref(),
            Some(&b"mapped-value"[..])
        );
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn proximity_finds_nearby() {
    // The CoordCube query primitive over the materialized store: entries
    // within L-infinity radius of a center path.
    let mut map = mem_map::<2>();
    let center = CoordKey::new([5, 5]);
    let nearby = CoordKey::new([5, 6]);
    let far = CoordKey::new([5, 20]);
    map.insert_by_coordkey(&center, b"center".to_vec());
    map.insert_by_coordkey(&nearby, b"nearby".to_vec());
    map.insert_by_coordkey(&far, b"far".to_vec());

    let center_path = center.to_coord_path();
    let results = map.proximity::<2, 1>(&center_path, 1);
    assert_eq!(results.len(), 2);
    let found: Vec<CoordPath<2>> = results.iter().map(|(p, _)| *p).collect();
    assert!(found.contains(&center_path));
    assert!(found.contains(&nearby.to_coord_path()));
}
