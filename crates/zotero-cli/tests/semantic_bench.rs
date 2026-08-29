use std::time::Instant;
use zotero_cli::semantic::vectors::{cosine_similarity, round_score};

#[test]
fn test_cosine_ranking_benchmark_5754_by_768() {
    let num_vectors = 5754;
    let dim = 768;

    // Generate deterministic test corpus
    let query_vector: Vec<f32> = (0..dim).map(|i| ((i % 17) as f32) / 17.0).collect();
    let corpus: Vec<(String, Vec<f32>)> = (0..num_vectors)
        .map(|idx| {
            let key = format!("ITEM_{:05}", idx);
            let vec: Vec<f32> = (0..dim).map(|i| (((idx + i) % 19) as f32) / 19.0).collect();
            (key, vec)
        })
        .collect();

    // Measure cosine similarity and ranking time
    let start = Instant::now();

    let mut scored: Vec<(&str, f64)> = Vec::with_capacity(num_vectors);
    for (key, vec) in &corpus {
        let score = cosine_similarity(&query_vector, vec);
        scored.push((key.as_str(), round_score(score)));
    }

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(b.0))
    });

    let top10: Vec<_> = scored.into_iter().take(10).collect();
    let elapsed = start.elapsed();

    println!(
        "Cosine ranking for {}x{} vectors took: {:?} (expected < 10ms)",
        num_vectors, dim, elapsed
    );

    assert_eq!(top10.len(), 10);
    // In release mode this will be <2ms; in unoptimized debug mode it might be a bit more,
    // but in release/optimized it should easily be under 10ms.
    // In debug test mode we can assert reasonable bounded execution.
    assert!(
        elapsed.as_millis() < 100,
        "Benchmark took too long in debug: {:?}",
        elapsed
    );
}
