// search.json scenario: index documents into the coordinate space, query
// by proximity, and retrieve matching entries, end to end over the
// materialized store (KvEntityStore over a file origin).
//
// The document scheme follows http://docs.ssccs.org/search.json: a JSON
// array of items, each with a `title` and a `text` field. Loading the
// remote data is forbidden in tests, so the fixture below is a local
// representative set in the same scheme (SSCCS documentation domain).
// The nexus semantic_scenarios.rs test exercises the same data through
// the semantic index; this test exercises it through the materialized
// coordinate store.
//
// Empirically exercises the two production-relevant contracts of the
// store surface:
//
// 1. durability: index -> flush -> reopen yields the same corpus and the
//    same spatial layout, so both retrieval by key and proximity queries
//    agree before and after the reopen;
// 2. error contract: an oversized value panics at the trait boundary
//    with a descriptive message, and the store stays usable afterwards
//    (the interior borrow is released before the panic, so the backing
//    mutex is not poisoned).

use chton::origin::{FileOrigin, MemoryOrigin};
use chton::store::{EntityStore, KvEntityStore};
use futures_executor::block_on;
use serde::{Deserialize, Serialize};
use tagma_core::{Coord, CoordPath};

/// A search.json item: `title` and `text`, the two fields the loader
/// extracts from docs.ssccs.org/search.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Doc {
    title: String,
    text: String,
}

/// Build the canonical N-character Hangul key whose path is exactly
/// `coords` (the injective Hangul mapping of `str_to_coordpath`).
fn hangul_key<const N: usize>(coords: [u16; N]) -> String {
    coords
        .iter()
        .map(|&c| Coord::new(c).unwrap().to_char())
        .collect()
}

fn path(coords: [u16; 6]) -> CoordPath<6> {
    CoordPath::new(coords.map(|c| Coord::new(c).unwrap()))
}

/// Representative search.json items (SSCCS documentation domain).
fn corpus() -> Vec<Doc> {
    vec![
        Doc {
            title: "Segment space".into(),
            text: "A segment is the unit of observation in the coordinate space; each field maps memory to structure."
                .into(),
        },
        Doc {
            title: "Immutable store".into(),
            text: "The immutable store indexes facts and intents with deterministic verification and provenance."
                .into(),
        },
        Doc {
            title: "Graph attention".into(),
            text: "Graph attention networks learn structure over the knowledge graph of documents and embeddings."
                .into(),
        },
        Doc {
            title: "Federated learning".into(),
            text: "Federated learning trains models across agents while keeping data local and memory bounded."
                .into(),
        },
        Doc {
            title: "RISC-V hardware".into(),
            text: "The risc and fpga hardware pipeline computes observations in parallel with energy constraints."
                .into(),
        },
    ]
}

/// Coordinate layout: axis 0 is the topic bucket, axis 1 the position.
/// Documents at positions 5..7 are within the query radius; positions
/// 50..51 are far away on the same topic.
fn layout() -> Vec<(Doc, [u16; 6])> {
    let positions = [
        [10, 5, 0, 0, 0, 0],
        [10, 6, 0, 0, 0, 0],
        [10, 7, 0, 0, 0, 0],
        [10, 50, 0, 0, 0, 0],
        [10, 51, 0, 0, 0, 0],
    ];
    corpus().into_iter().zip(positions).collect()
}

const CENTER: [u16; 6] = [10, 5, 0, 0, 0, 0];

#[test]
fn search_json_index_flush_reopen_proximity() {
    let file = std::env::temp_dir().join(format!("chton-search-json-{}.bin", std::process::id()));
    let file2 = file.clone();
    let center = path(CENTER);

    {
        let store = KvEntityStore::<6, Doc>::new(Box::new(FileOrigin::open(&file).unwrap()), 4096);
        block_on(async {
            for (doc, coords) in layout() {
                store.insert(hangul_key(coords), doc).await;
            }
            assert_eq!(store.len().await, 5, "corpus indexed");

            // Proximity before the flush: radius-2 box around axis1=5
            // covers positions 5, 6, 7 and excludes 50, 51.
            let near = store.proximity::<2, 3>(&center, 2);
            assert_eq!(near.len(), 3, "radius-2 box covers the near cluster");
            assert!(near.iter().any(|(_, d)| d.title == "Segment space"));
            assert!(near.iter().any(|(_, d)| d.title == "Immutable store"));
            assert!(near.iter().any(|(_, d)| d.title == "Graph attention"));
            assert!(!near.iter().any(|(_, d)| d.title == "Federated learning"));
            assert!(!near.iter().any(|(_, d)| d.title == "RISC-V hardware"));

            store.flush().unwrap();
        });
    }

    {
        let store =
            KvEntityStore::<6, Doc>::load(Box::new(FileOrigin::open(&file2).unwrap()), 4096)
                .unwrap();
        block_on(async {
            assert_eq!(store.len().await, 5, "corpus survives the reopen");

            // Retrieval by key after the reopen.
            let k_segment = hangul_key([10, 5, 0, 0, 0, 0]);
            assert_eq!(store.get(&k_segment).await.unwrap().title, "Segment space");

            // Proximity after the reopen: the spatial layout is durable.
            let near = store.proximity::<2, 3>(&center, 2);
            assert_eq!(near.len(), 3, "spatial layout survives the reopen");
            let mut titles: Vec<String> = near.iter().map(|(_, d)| d.title.clone()).collect();
            titles.sort();
            assert_eq!(
                titles,
                vec!["Graph attention", "Immutable store", "Segment space"]
            );
        });
    }

    std::fs::remove_file(&file).unwrap();
}

#[test]
fn search_json_oversized_value_panics_without_poisoning() {
    // Slot 128 bounds values to 120 bytes; the oversized document
    // exceeds it, so the insert fails at the trait boundary.
    let store = KvEntityStore::<6, Doc>::new(Box::new(MemoryOrigin::new()), 128);
    let big = Doc {
        title: "x".repeat(300),
        text: "y".repeat(300),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        block_on(store.insert("oversized".into(), big))
    }));
    let err = result.expect_err("oversized value must panic at the trait boundary");
    let msg = err
        .downcast_ref::<String>()
        .map(|s| s.as_str())
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        msg.contains("kv entity store insert failed"),
        "panic must carry a descriptive message, got: {msg}"
    );

    // The interior borrow was released before the panic: the store is
    // still usable (no poisoned mutex).
    block_on(async {
        store
            .insert(
                "small".into(),
                Doc {
                    title: "ok".into(),
                    text: "ok".into(),
                },
            )
            .await;
        assert_eq!(store.len().await, 1, "store remains usable after the panic");
        assert_eq!(store.get("small").await.unwrap().title, "ok");
    });
}
