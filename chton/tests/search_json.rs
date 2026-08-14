// search.json scenario at real scale: the document set from
// http://docs.ssccs.org/search.json (a JSON array; each item carries
// `objectID`, `title`, `text`, plus fields unused here) is fetched at
// test runtime and indexed into the materialized coordinate store, then
// searched by spatial proximity.
//
// The data is fetched remotely on every run, the same pattern nexus uses
// in semantic_scenarios.rs. A network failure skips the test loudly
// instead of failing, so the local gate stays green offline.
//
// The search architecture mirrors the coordinate-space design:
// - a document store (KvEntityStore<6, Doc>) keyed by the objectID
//   coordinate (SHA-256 fingerprint, one record per document), and
// - a spatial index (KvEntityStore<6, Vec<String>>) keyed by the
//   vocabulary-fold coordinate of each document's text; a fold holds the
//   objectIDs whose texts map to it.
// A query folds its text and collects the objectIDs from every fold
// within the proximity radius, then retrieves the documents.
//
// The two empirical contracts under test:
// 1. durability at scale: index all documents -> flush -> reopen yields
//    the same corpus and the same proximity results (reopen reads the
//    persisted record count from the header instead of walking the
//    sparse 11172-wide tree);
// 2. error contract: a value exceeding the record slot panics with a
//    descriptive message and leaves the store usable (no poisoned mutex).

use chton::origin::{FileOrigin, MemoryOrigin};
use chton::store::{EntityStore, KvEntityStore};
use futures_executor::block_on;
use serde::{Deserialize, Serialize};
use tagma_core::{Coord, CoordPath};

/// The remote search.json endpoint, fetched at test runtime.
const SEARCH_JSON_URL: &str = "https://docs.ssccs.org/search.json";

/// A search.json item. Unknown fields (href, section, crumbs) are
/// ignored by serde, matching the loader scheme (title + text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Doc {
    #[serde(rename = "objectID")]
    object_id: String,
    title: String,
    text: String,
}

/// Vocabulary for the coordinate fold, curated from the SSCCS
/// documentation domain (a subset of the nexus semantic vocabulary).
const VOCABULARY: &[&str] = &[
    "segment",
    "scheme",
    "field",
    "observation",
    "projection",
    "computation",
    "immutable",
    "structure",
    "constraint",
    "energy",
    "memory",
    "data",
    "parallel",
    "deterministic",
    "fih",
    "fact",
    "intent",
    "hint",
    "blackboard",
    "semantic",
    "vector",
    "store",
    "index",
    "search",
    "rust",
    "compiler",
    "verification",
    "c2pa",
    "provenance",
    "hardware",
    "risc",
    "fpga",
    "collapse",
    "github",
    "foundation",
    "ssccs",
    "open",
    "source",
    "agent",
    "knowledge",
    "graph",
    "document",
    "embedding",
    "inference",
    "model",
    "neural",
    "token",
    "attention",
];

/// Vocabulary-fold coordinate: axis i is the count of vocabulary terms
/// from group i that appear in the text (word presence, 0..8 per axis).
fn fold_coords(text: &str) -> [u16; 6] {
    let lower = text.to_lowercase();
    let per = VOCABULARY.len().div_ceil(6);
    let mut coords = [0u16; 6];
    for (i, coord) in coords.iter_mut().enumerate() {
        let start = i * per;
        let end = (start + per).min(VOCABULARY.len());
        let sum = VOCABULARY[start..end]
            .iter()
            .filter(|w| lower.contains(**w))
            .count() as u16;
        *coord = sum;
    }
    coords
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

/// Fetch search.json at runtime. `None` means the network is unavailable
/// (offline run), so the caller skips the test loudly.
fn fetch_search_items() -> Option<Vec<Doc>> {
    match ureq::get(SEARCH_JSON_URL).call() {
        Ok(resp) => match resp.into_body().read_to_vec() {
            Ok(data) => match serde_json::from_slice(&data) {
                Ok(items) => Some(items),
                Err(e) => {
                    eprintln!("search.json test skipped: invalid payload: {e}");
                    None
                }
            },
            Err(e) => {
                eprintln!("search.json test skipped: body read failed: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("search.json test skipped: network unavailable: {e}");
            None
        }
    }
}

/// Real-scale documents that fit one 16 KiB record slot. The full set is
/// 793 items; the outliers are long-form pages.
fn fitted_docs() -> Vec<Doc> {
    fetch_search_items()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.text.len() + d.title.len() + d.object_id.len() <= 16_000)
        .collect()
}

#[test]
fn search_json_real_scale_spatial_search_and_durability() {
    let docs = fitted_docs();
    if docs.is_empty() {
        eprintln!("search.json test skipped: no documents fetched");
        return;
    }
    let n = docs.len();
    assert!(n >= 700, "real-scale sample: {n} of 793 docs indexed");

    let doc_file =
        std::env::temp_dir().join(format!("chton-search-docs-{}.bin", std::process::id()));
    let idx_file =
        std::env::temp_dir().join(format!("chton-search-idx-{}.bin", std::process::id()));
    let doc_file2 = doc_file.clone();
    let idx_file2 = idx_file.clone();

    // The query is the first real document's text; its fold is the
    // proximity center.
    let query = docs[0].text.clone();
    let query_object_id = docs[0].object_id.clone();
    let center = path(fold_coords(&query));

    let mut expected: Option<Vec<String>> = None;
    {
        let doc_store =
            KvEntityStore::<6, Doc>::new(Box::new(FileOrigin::open(&doc_file).unwrap()), 16_384);
        let idx_store = KvEntityStore::<6, Vec<String>>::new(
            Box::new(FileOrigin::open(&idx_file).unwrap()),
            8192,
        );
        block_on(async {
            for doc in &docs {
                doc_store.insert(doc.object_id.clone(), doc.clone()).await;
            }
            assert_eq!(
                doc_store.len().await,
                n,
                "every document indexed without key collision"
            );

            for doc in &docs {
                let key = hangul_key(fold_coords(&doc.text));
                let mut list = idx_store.get(&key).await.unwrap_or_default();
                list.push(doc.object_id.clone());
                idx_store.insert(key, list).await;
            }

            // Proximity search: collect objectIDs from folds within the
            // radius, then resolve them through the document store.
            let near = idx_store.proximity::<2, 3>(&center, 2);
            let mut ids: Vec<String> = near.iter().flat_map(|(_, list)| list.clone()).collect();
            ids.sort();
            ids.dedup();
            assert!(!ids.is_empty(), "query finds matching entries");
            assert!(
                ids.contains(&query_object_id),
                "the query document itself is found"
            );
            for id in &ids {
                let doc = doc_store
                    .get(id)
                    .await
                    .unwrap_or_else(|| panic!("index references missing document {id}"));
                assert_eq!(&doc.object_id, id);
            }

            doc_store.flush().unwrap();
            idx_store.flush().unwrap();
            expected = Some(ids);
        });
    }

    // Reopen both stores: the corpus and the spatial layout are durable.
    {
        let doc_store =
            KvEntityStore::<6, Doc>::load(Box::new(FileOrigin::open(&doc_file2).unwrap()), 16_384)
                .unwrap();
        let idx_store = KvEntityStore::<6, Vec<String>>::load(
            Box::new(FileOrigin::open(&idx_file2).unwrap()),
            8192,
        )
        .unwrap();
        block_on(async {
            assert_eq!(doc_store.len().await, n, "corpus survives the reopen");
            let near = idx_store.proximity::<2, 3>(&center, 2);
            let mut ids: Vec<String> = near.iter().flat_map(|(_, list)| list.clone()).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(
                &ids,
                expected.as_ref().expect("query ran before the reopen"),
                "proximity results survive the reopen"
            );
        });
    }

    std::fs::remove_file(&doc_file).unwrap();
    std::fs::remove_file(&idx_file).unwrap();
}

#[test]
fn search_json_oversized_value_panics_without_poisoning() {
    // The largest fetched document exceeds a 120-byte value budget by
    // orders of magnitude, so the insert fails at the trait boundary.
    let docs = fetch_search_items().unwrap_or_default();
    if docs.is_empty() {
        eprintln!("search.json test skipped: no documents fetched");
        return;
    }
    let big = docs
        .iter()
        .max_by_key(|d| d.text.len())
        .expect("fetched documents")
        .clone();

    let store = KvEntityStore::<6, Doc>::new(Box::new(MemoryOrigin::new()), 128);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        block_on(store.insert(big.object_id.clone(), big))
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
                    object_id: "small".into(),
                    title: "ok".into(),
                    text: "ok".into(),
                },
            )
            .await;
        assert_eq!(store.len().await, 1, "store remains usable after the panic");
        assert_eq!(store.get("small").await.unwrap().object_id, "small");
    });
}
