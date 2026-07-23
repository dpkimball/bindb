use bindb::vector::{VectorEntry, VectorFilter, VectorIndex};

fn unit(v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / norm).collect()
}

fn entry(object_id: &str, chunk_id: &str, model_id: &str, v: Vec<f32>) -> VectorEntry {
    VectorEntry {
        object_id: object_id.to_string(),
        object_version_id: format!("{object_id}-v1"),
        chunk_id: chunk_id.to_string(),
        chunk_index: 0,
        model_id: model_id.to_string(),
        dimensions: v.len() as u32,
        vector: v,
        content_hash: [0u8; 32],
        source_text: format!("text for {chunk_id}"),
        binder_id: None,
        app_id: None,
    }
}

fn no_filter() -> VectorFilter {
    VectorFilter::default()
}

// ── brute-force path (small index) ───────────────────────────────────────────

#[test]
fn brute_force_returns_nearest_neighbour() {
    let mut idx = VectorIndex::new();
    idx.insert(entry(
        "a://o1",
        "c1",
        "text-embedding-3",
        unit(vec![1.0, 0.0]),
    ));
    idx.insert(entry(
        "a://o2",
        "c2",
        "text-embedding-3",
        unit(vec![0.0, 1.0]),
    ));
    idx.insert(entry(
        "a://o3",
        "c3",
        "text-embedding-3",
        unit(vec![1.0, 1.0]),
    ));

    let query = unit(vec![1.0, 0.0]);
    let results = idx.knn_search(&query, &no_filter(), 1);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object_id, "a://o1");
}

#[test]
fn brute_force_top_k_ordered_by_score() {
    let mut idx = VectorIndex::new();
    // Build a 1-D space: o1 at 1.0, o2 at 0.5, o3 at 0.0.
    idx.insert(entry("a://o1", "c1", "m1", unit(vec![1.0, 0.0])));
    idx.insert(entry("a://o2", "c2", "m1", unit(vec![1.0, 1.0])));
    idx.insert(entry("a://o3", "c3", "m1", unit(vec![0.0, 1.0])));

    let query = unit(vec![1.0, 0.0]);
    let results = idx.knn_search(&query, &no_filter(), 3);
    assert_eq!(results.len(), 3);
    assert!(
        results[0].similarity_score >= results[1].similarity_score,
        "results must be ordered descending"
    );
    assert!(results[1].similarity_score >= results[2].similarity_score);
}

#[test]
fn tombstone_removes_object_from_search() {
    let mut idx = VectorIndex::new();
    idx.insert(entry("a://alive", "c1", "m1", unit(vec![1.0, 0.0])));
    idx.insert(entry("a://dead", "c2", "m1", unit(vec![1.0, 0.01])));

    idx.tombstone_object("a://dead");

    let query = unit(vec![1.0, 0.0]);
    let results = idx.knn_search(&query, &no_filter(), 5);
    assert!(
        results.iter().all(|r| r.object_id != "a://dead"),
        "tombstoned object must not appear in results"
    );
}

#[test]
fn model_filter_scopes_results_to_matching_model() {
    let mut idx = VectorIndex::new();
    idx.insert(entry("a://o1", "c1", "model-a", unit(vec![1.0, 0.0])));
    idx.insert(entry("a://o2", "c2", "model-b", unit(vec![1.0, 0.0])));

    let filter = VectorFilter {
        model_id: Some("model-a".into()),
        ..Default::default()
    };
    let results = idx.knn_search(&unit(vec![1.0, 0.0]), &filter, 10);
    assert!(
        results.iter().all(|r| r.model_id == "model-a"),
        "model filter must exclude model-b results"
    );
}

#[test]
fn chunk_update_replaces_previous_entry() {
    let mut idx = VectorIndex::new();
    // Insert chunk then replace with a different vector.
    idx.insert(entry("a://obj", "chunk-1", "m1", unit(vec![0.0, 1.0])));
    idx.insert(entry("a://obj", "chunk-1", "m1", unit(vec![1.0, 0.0])));

    let query = unit(vec![1.0, 0.0]);
    let results = idx.knn_search(&query, &no_filter(), 1);
    assert_eq!(results.len(), 1);
    // Score after update should be 1.0 (perfect match), not the old vector's score.
    assert!(
        results[0].similarity_score > 0.99,
        "updated chunk must reflect new vector"
    );
}

// ── HNSW path (large index) ───────────────────────────────────────────────────

fn make_random_unit_vector(seed: usize, dims: usize) -> Vec<f32> {
    // Deterministic pseudo-random unit vector using a simple hash.
    let mut v: Vec<f32> = (0..dims)
        .map(|i| {
            let h = (seed
                .wrapping_mul(2654435761)
                .wrapping_add(i.wrapping_mul(2246822519))) as f32;
            (h % 17.0) - 8.0
        })
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

/// Build a large index (above HNSW_THRESHOLD) and verify that knn_search
/// returns the exact nearest neighbor for an in-corpus query.
#[test]
fn hnsw_finds_exact_match_for_in_corpus_query() {
    let n = 1_200usize; // above HNSW_THRESHOLD (1_000)
    let dims = 32;
    let model = "emb-v1";

    let mut idx = VectorIndex::new();
    let vectors: Vec<Vec<f32>> = (0..n).map(|i| make_random_unit_vector(i, dims)).collect();

    for (i, v) in vectors.iter().enumerate() {
        idx.insert(VectorEntry {
            object_id: format!("a://o{i}"),
            object_version_id: format!("a://o{i}-v1"),
            chunk_id: format!("c{i}"),
            chunk_index: 0,
            model_id: model.to_string(),
            dimensions: dims as u32,
            vector: v.clone(),
            content_hash: [0u8; 32],
            source_text: format!("obj {i}"),
            binder_id: None,
            app_id: None,
        });
    }

    // Query is a copy of a corpus vector — perfect match must be #1.
    let target = 42usize;
    let results = idx.knn_search(
        &vectors[target],
        &VectorFilter {
            model_id: Some(model.into()),
            ..Default::default()
        },
        5,
    );

    assert!(!results.is_empty(), "HNSW must return results");
    assert_eq!(
        results[0].object_id,
        format!("a://o{target}"),
        "exact match must be top result"
    );
    assert!(
        results[0].similarity_score > 0.999,
        "self-similarity must be ~1.0, got {}",
        results[0].similarity_score
    );
}

#[test]
fn hnsw_recall_at_least_80_percent_top10() {
    let n = 1_200usize;
    let dims = 32;
    let model = "emb-v2";

    let mut idx = VectorIndex::new();
    let vectors: Vec<Vec<f32>> = (0..n)
        .map(|i| make_random_unit_vector(i + 100, dims))
        .collect();
    let mut brute_idx = VectorIndex::new();

    for (i, v) in vectors.iter().enumerate() {
        let e = VectorEntry {
            object_id: format!("b://o{i}"),
            object_version_id: format!("b://o{i}-v1"),
            chunk_id: format!("d{i}"),
            chunk_index: 0,
            model_id: model.to_string(),
            dimensions: dims as u32,
            vector: v.clone(),
            content_hash: [0u8; 32],
            source_text: format!("obj {i}"),
            binder_id: None,
            app_id: None,
        };
        idx.insert(e.clone());
        brute_idx.insert(e);
    }

    let filter = VectorFilter {
        model_id: Some(model.into()),
        ..Default::default()
    };

    // Sample 10 queries and measure recall@10 vs brute force.
    let k = 10usize;
    let mut total_correct = 0usize;
    let total_queries = 10usize;

    for q_seed in 200..200 + total_queries {
        let query = make_random_unit_vector(q_seed, dims);
        let hnsw_ids: std::collections::HashSet<String> = idx
            .knn_search(&query, &filter, k)
            .into_iter()
            .map(|r| r.object_id)
            .collect();
        let brute_ids: std::collections::HashSet<String> = brute_idx
            .knn_search(&query, &filter, k)
            .into_iter()
            .map(|r| r.object_id)
            .collect();
        total_correct += hnsw_ids.intersection(&brute_ids).count();
    }

    let recall = total_correct as f64 / (total_queries * k) as f64;
    assert!(
        recall >= 0.8,
        "HNSW recall@10 must be ≥ 80%, got {:.1}%",
        recall * 100.0
    );
}

#[test]
fn hnsw_tombstone_excludes_object_from_large_index() {
    let n = 1_200usize;
    let dims = 16;
    let model = "emb-v3";
    let victim = 7usize;

    let mut idx = VectorIndex::new();
    let vectors: Vec<Vec<f32>> = (0..n)
        .map(|i| make_random_unit_vector(i + 300, dims))
        .collect();

    for (i, v) in vectors.iter().enumerate() {
        idx.insert(VectorEntry {
            object_id: format!("c://o{i}"),
            object_version_id: format!("c://o{i}-v1"),
            chunk_id: format!("e{i}"),
            chunk_index: 0,
            model_id: model.to_string(),
            dimensions: dims as u32,
            vector: v.clone(),
            content_hash: [0u8; 32],
            source_text: String::new(),
            binder_id: None,
            app_id: None,
        });
    }

    idx.tombstone_object(&format!("c://o{victim}"));

    let results = idx.knn_search(
        &vectors[victim],
        &VectorFilter {
            model_id: Some(model.into()),
            ..Default::default()
        },
        20,
    );
    assert!(
        results
            .iter()
            .all(|r| r.object_id != format!("c://o{victim}")),
        "tombstoned object must not appear in HNSW results"
    );
}
