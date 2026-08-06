// search.json scenario: index documents into the coordinate space, query
// by proximity, and retrieve matching entries, end to end over the
// materialized store (KvEntityStore over a file origin).
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Doc {
    id: String,
    title: String,
    body: String,
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

/// The search.json document set: id, title, body.
fn corpus() -> Vec<Doc> {
    vec![
        Doc {
            id: "quantum".into(),
            title: "Quantum error correction".into(),
            body: "Quantum error correction reduces logical error rates below fault-tolerant thresholds."
                .into(),
        },
        Doc {
            id: "transformer".into(),
            title: "Transformer BLEU".into(),
            body: "Transformer models achieve state-of-the-art BLEU on translation benchmarks.".into(),
        },
        Doc {
            id: "graph-attention".into(),
            title: "Graph attention networks".into(),
            body: "Graph attention networks outperform GCN on ogbn-arxiv node classification.".into(),
        },
        Doc {
            id: "federated".into(),
            title: "Federated learning".into(),
            body: "Federated learning converges within 5% of centralized accuracy.".into(),
        },
        Doc {
            id: "contrastive".into(),
            title: "Contrastive learning".into(),
            body: "Contrastive learning needs only 5% of labeled data.".into(),
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
            assert!(near.iter().any(|(_, d)| d.id == "quantum"));
            assert!(near.iter().any(|(_, d)| d.id == "transformer"));
            assert!(near.iter().any(|(_, d)| d.id == "graph-attention"));
            assert!(!near.iter().any(|(_, d)| d.id == "federated"));
            assert!(!near.iter().any(|(_, d)| d.id == "contrastive"));

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
            let k_quantum = hangul_key([10, 5, 0, 0, 0, 0]);
            assert_eq!(store.get(&k_quantum).await.unwrap().id, "quantum");

            // Proximity after the reopen: the spatial layout is durable.
            let near = store.proximity::<2, 3>(&center, 2);
            assert_eq!(near.len(), 3, "spatial layout survives the reopen");
            let mut ids: Vec<String> = near.iter().map(|(_, d)| d.id.clone()).collect();
            ids.sort();
            assert_eq!(ids, vec!["graph-attention", "quantum", "transformer"]);
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
        id: "oversized".into(),
        title: "x".repeat(300),
        body: "y".repeat(300),
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
                    id: "small".into(),
                    title: "ok".into(),
                    body: "ok".into(),
                },
            )
            .await;
        assert_eq!(store.len().await, 1, "store remains usable after the panic");
        assert_eq!(store.get("small").await.unwrap().id, "small");
    });
}
