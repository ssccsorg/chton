// Store surface tests: the generic EntityStore family (memory and
// materialized implementations) over the chton behavior layer.

use chton::origin::{FileOrigin, MemoryOrigin};
use chton::store::{
    CoordEntityStore, EntityStore, KeyError, KvEntityStore, MemoryEntityStore, str_to_coordpath,
};
use futures_executor::block_on;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Doc {
    title: String,
}

#[test]
fn memory_entity_store_round_trip() {
    let store = MemoryEntityStore::<Doc>::new();
    block_on(async {
        store
            .insert(
                "d1".into(),
                Doc {
                    title: "alpha".into(),
                },
            )
            .await;
        store
            .insert(
                "d2".into(),
                Doc {
                    title: "beta".into(),
                },
            )
            .await;
        assert_eq!(store.len().await, 2);
        assert!(store.contains_key("d1").await);
        assert_eq!(store.get("d1").await.unwrap().title, "alpha");

        let removed = store.remove("d1").await.unwrap();
        assert_eq!(removed.title, "alpha");
        assert!(!store.contains_key("d1").await);

        store
            .replace_from(vec![(
                "d3".into(),
                Doc {
                    title: "gamma".into(),
                },
            )])
            .await;
        // replace_from replaces the whole store, not appends.
        let values = store.values().await;
        assert_eq!(values.len(), 1);
        assert!(values.iter().any(|d| d.title == "gamma"));
    });
}

#[test]
fn coord_entity_store_round_trip_and_axis_filter() {
    let store = CoordEntityStore::<6, Doc>::new();
    block_on(async {
        store
            .insert(
                "doc_a".into(),
                Doc {
                    title: "alpha".into(),
                },
            )
            .await;
        store
            .insert(
                "doc_b".into(),
                Doc {
                    title: "beta".into(),
                },
            )
            .await;
        assert_eq!(store.len().await, 2);

        // axis_filtered: match on the creator axis (axis 4) value produced
        // by the key mapping; the filter axis is taken from the key's own
        // mapped path, so it selects exactly that key's entry.
        let a_coord = chton::store::str_to_coordpath::<6>("doc_a").unwrap();
        let matched = store
            .axis_filtered(&[(4, a_coord.coords()[4].index())])
            .await;
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].title, "alpha");
    });
}

#[test]
fn general_encoding_injective_within_capacity() {
    // Key classes that collided under the previous byte-wise mapping must
    // now be distinct under the injective general encoding: prefix keys
    // with and without a trailing NUL, keys longer than the old N-byte
    // limit, byte keys, and Hangul keys that are not canonical at this
    // depth (canonical is 12 Hangul characters at depth 13).
    let store = CoordEntityStore::<13, Doc>::new();
    block_on(async {
        for (k, title) in [
            ("ab", "ab"),
            ("ab\x00", "ab-nul"),
            ("abcdef", "six"),
            ("abcdefghijklmnop", "long"),
            ("가각간갈감갑", "hangul"),
            ("\x00\x01\x02\x03\x04\x05", "bytes"),
        ] {
            store
                .insert(
                    k.into(),
                    Doc {
                        title: title.into(),
                    },
                )
                .await;
        }

        assert_eq!(store.len().await, 6, "no key may collide");
        assert_eq!(store.get("ab").await.unwrap().title, "ab");
        assert_eq!(store.get("ab\x00").await.unwrap().title, "ab-nul");
        assert_eq!(store.get("abcdef").await.unwrap().title, "six");
        assert_eq!(store.get("abcdefghijklmnop").await.unwrap().title, "long");
        assert_eq!(store.get("가각간갈감갑").await.unwrap().title, "hangul");
        assert_eq!(
            store.get("\x00\x01\x02\x03\x04\x05").await.unwrap().title,
            "bytes"
        );
    });
}

#[test]
fn canonical_and_general_formats_are_disjoint() {
    // At depth 7 a 6-Hangul key is canonical (direct path, marker axis
    // 0); a 6-byte general key encodes onto the payload axes with a
    // non-zero marker. The two formats never share a path.
    let store = CoordEntityStore::<7, Doc>::new();
    block_on(async {
        store
            .insert(
                "가나다라마바".into(),
                Doc {
                    title: "canonical".into(),
                },
            )
            .await;
        store
            .insert(
                "\x00\x01\x02\x03\x04\x05".into(),
                Doc {
                    title: "general".into(),
                },
            )
            .await;

        assert_eq!(store.len().await, 2);
        assert_eq!(store.get("가나다라마바").await.unwrap().title, "canonical");
        assert_eq!(
            store.get("\x00\x01\x02\x03\x04\x05").await.unwrap().title,
            "general"
        );

        let canonical = chton::store::str_to_coordpath::<7>("가나다라마바").unwrap();
        let general = chton::store::str_to_coordpath::<7>("\x00\x01\x02\x03\x04\x05").unwrap();
        // Canonical: the six characters map directly, marker axis 0.
        assert_eq!(canonical.coords()[6].index(), 0);
        for (i, ch) in "가나다라마바".chars().enumerate() {
            assert_eq!(
                canonical.coords()[i].index(),
                tagma_core::Coord::from_char(ch).unwrap().index()
            );
        }
        // General: non-zero marker axis, so the formats are disjoint.
        assert_ne!(general.coords()[6].index(), 0);
        assert_ne!(canonical, general);
    });
}

#[test]
fn empty_key_is_rejected() {
    // An empty string has no representable path: mapping rejects it and
    // probing treats it as absent.
    let err = str_to_coordpath::<7>("").unwrap_err();
    assert!(matches!(err, KeyError::Empty));

    let store = CoordEntityStore::<7, Doc>::new();
    block_on(async {
        assert!(store.get("").await.is_none());
        assert!(!store.contains_key("").await);
        assert_eq!(store.remove("").await, None);
    });
}

#[test]
fn hangul_key_of_non_canonical_length_is_general() {
    // Canonical keys are exactly M-1 Hangul characters. At depth 7 a
    // 5-Hangul key is not canonical and its 15 bytes exceed the general
    // capacity, so it is rejected; at depth 13 it fits as a general key.
    let err = str_to_coordpath::<7>("가나다라마").unwrap_err();
    assert!(matches!(err, KeyError::TooLong { .. }), "got: {err:?}");

    let general = str_to_coordpath::<13>("가나다라마").unwrap();
    assert_ne!(
        general.coords()[12].index(),
        0,
        "marker axis carries the byte length"
    );
}

#[test]
fn general_encoding_preserves_order_for_same_length_keys() {
    // Same-length keys: lexicographic byte order maps to coordinate
    // order because the digits are big-endian base-11172.
    let keys = ["abc", "abd", "abe"];
    let paths: Vec<Vec<u16>> = keys
        .iter()
        .map(|k| {
            str_to_coordpath::<7>(k).unwrap().coords()[..6]
                .iter()
                .map(|c| c.index())
                .collect()
        })
        .collect();
    assert!(
        paths[0] < paths[1] && paths[1] < paths[2],
        "byte order must be preserved: {paths:?}"
    );
}

#[test]
fn over_capacity_key_rejected() {
    // A key longer than the payload capacity is rejected on insert and
    // treated as absent on probe. Capacity at depth 7 is about 10 bytes.
    let store = CoordEntityStore::<7, Doc>::new();
    let long = "abcdefghijklmnop";
    block_on(async {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            block_on(store.insert(
                long.into(),
                Doc {
                    title: "long".into(),
                },
            ))
        }));
        assert!(result.is_err(), "over-capacity insert must panic");
        assert!(store.get(long).await.is_none());
        assert!(!store.contains_key(long).await);
        assert_eq!(store.remove(long).await, None);
    });
}

#[test]
fn kv_entity_store_round_trip_and_flush() {
    let store = KvEntityStore::<16, Doc>::new(Box::new(MemoryOrigin::new()), 512);
    block_on(async {
        store
            .insert(
                "d1".into(),
                Doc {
                    title: "alpha".into(),
                },
            )
            .await;
        assert_eq!(store.get("d1").await.unwrap().title, "alpha");
        assert!(store.is_buffered());
        store.flush().unwrap();
        assert!(!store.is_buffered());

        let values = store.values().await;
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].title, "alpha");
    });
}

#[test]
fn kv_entity_store_persists_across_reopen() {
    let path = std::env::temp_dir().join(format!("chton-store-{}.bin", std::process::id()));
    let path2 = path.clone();

    {
        let store = KvEntityStore::<16, Doc>::new(Box::new(FileOrigin::open(&path).unwrap()), 4096);
        block_on(async {
            store
                .insert(
                    "d_persist".into(),
                    Doc {
                        title: "kept".into(),
                    },
                )
                .await;
            store.flush().unwrap();
        });
    }
    {
        let store =
            KvEntityStore::<16, Doc>::load(Box::new(FileOrigin::open(&path2).unwrap()), 4096)
                .unwrap();
        block_on(async {
            assert_eq!(store.len().await, 1);
            assert_eq!(store.get("d_persist").await.unwrap().title, "kept");
        });
    }
    std::fs::remove_file(&path).unwrap();
}
