// Store surface tests: the generic EntityStore family (memory and
// materialized implementations) over the chton behavior layer.

use chton::origin::{FileOrigin, MemoryOrigin};
use chton::store::{CoordEntityStore, EntityStore, KvEntityStore, MemoryEntityStore};
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
        // by the ByteWise key mapping; the two keys differ in the first
        // byte, so filtering on that axis value selects exactly one.
        let a_coord = chton::store::str_to_coordpath::<6>("doc_a");
        let matched = store
            .axis_filtered(&[(4, a_coord.coords()[4].index())])
            .await;
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].title, "alpha");
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
