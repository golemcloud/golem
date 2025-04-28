use crate::OpenAIProvider;
use embed_common::wit::{generate_embeddings, rerank};
use golem_embed::embed;
use once_cell::sync::OnceCell;
use std::sync::Arc;

static PROVIDER: OnceCell<Arc<OpenAIProvider>> = OnceCell::new();

fn get_provider() -> &'static Arc<OpenAIProvider> {
    PROVIDER.get_or_init(|| {
        let provider = OpenAIProvider::new()
            .expect("Failed to initialize OpenAI provider");
        Arc::new(provider)
    })
}

/// Generate embeddings for the given inputs
#[export_name = "golem:embed/embed#generate-embeddings"]
pub async fn wit_generate_embeddings(
    inputs: Vec<embed::ContentPart>,
    config: embed::Config,
) -> Result<embed::EmbeddingResponse, embed::Error> {
    let provider = get_provider();
    generate_embeddings(provider, inputs, config).await
}

/// Rerank documents based on query relevance
#[export_name = "golem:embed/embed#rerank"]
pub async fn wit_rerank(
    query: String,
    documents: Vec<String>,
    config: embed::Config,
) -> Result<embed::RerankResponse, embed::Error> {
    let provider = get_provider();
    rerank(provider, query, documents, config).await
}