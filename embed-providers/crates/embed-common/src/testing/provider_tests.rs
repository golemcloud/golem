use super::*;
use std::collections::HashMap;

pub async fn test_provider_interop<T1: EmbeddingProvider, T2: EmbeddingProvider>(
    provider1: &T1,
    provider2: &T2,
) -> Result<(), EmbeddingError> {
    // Test cross-provider embedding compatibility
    let test_texts = vec![
        "This is a test sentence".to_string(),
        "Another test sentence".to_string(),
    ];

    // Get embeddings from both providers
    let embeddings1 = provider1.generate_embeddings(test_texts.clone()).await?;
    let embeddings2 = provider2.generate_embeddings(test_texts.clone()).await?;

    // Verify basic embedding properties
    assert_eq!(embeddings1.len(), embeddings2.len(), "Providers should return same number of embeddings");
    assert_eq!(
        embeddings1[0].len(),
        embeddings2[0].len(),
        "Embedding dimensions should match when using default models"
    );

    // Test cross-provider reranking compatibility
    let query = "test query".to_string();
    let documents = vec![
        "First relevant document".to_string(),
        "Second relevant document".to_string(),
        "Completely unrelated document".to_string(),
    ];

    // Get rankings from both providers
    let rankings1 = provider1.rerank(query.clone(), documents.clone()).await?;
    let rankings2 = provider2.rerank(query, documents).await?;

    // Verify ranking properties
    assert_eq!(rankings1.len(), rankings2.len(), "Providers should rank same number of documents");
    
    // Compare relative rankings (should roughly agree on most/least relevant)
    let first_rankings = rankings_to_map(&rankings1);
    let second_rankings = rankings_to_map(&rankings2);

    // Most relevant document in first ranking should be in top 2 of second ranking
    let top_doc_idx = rankings1[0].0;
    let top_doc_rank_in_second = get_rank_in_list(&rankings2, top_doc_idx);
    assert!(
        top_doc_rank_in_second < 2,
        "Top document from first provider should be ranked highly by second provider"
    );

    Ok(())
}

fn rankings_to_map(rankings: &[(usize, f32)]) -> HashMap<usize, usize> {
    rankings
        .iter()
        .enumerate()
        .map(|(rank, (idx, _))| (*idx, rank))
        .collect()
}

fn get_rank_in_list(rankings: &[(usize, f32)], idx: usize) -> usize {
    rankings
        .iter()
        .position(|(i, _)| *i == idx)
        .unwrap_or(rankings.len())
}