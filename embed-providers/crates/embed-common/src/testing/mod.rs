mod provider_tests;

pub use provider_tests::test_provider_interop;

use crate::{EmbeddingError, EmbeddingProvider};
use std::collections::HashMap;

pub struct MockEmbeddingProvider {
    pub embeddings: HashMap<String, Vec<f32>>,
    pub rerank_scores: HashMap<String, f32>,
}

// Re-export testing utilities
pub use super::testing::provider_tests::*;