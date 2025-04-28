use golem_durability::prelude::*;
use serde::{Deserialize, Serialize};
use reqwest::Client;
use std::env;
use wit_bindgen::generate;

generate!({
    world: "embed",
    path: "wit/embed.wit",
    with: {
        "golem:embed/embed": embed_component
    }
});

struct EmbedComponent;

#[durable]
impl embed::Embed for EmbedComponent {
    fn generate(
        inputs: Vec<embed::ContentPart>,
        config: embed::Config
    ) -> Result<embed::EmbeddingResponse, embed::Error> {
        let api_key = env::var("COHERE_API_KEY")
            .map_err(|_| embed::Error::invalid("Missing COHERE_API_KEY"))?;

        let client = Client::new();
        let response = durability::wrap(|| {
            client.post("https://api.cohere.ai/v1/embed")
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&CohereRequest {
                    texts: inputs.iter().map(|p| p.to_string()).collect(),
                    model: config.model.unwrap_or("embed-english-v3.0".into()),
                    input_type: config.task_type.map(|t| t.to_string()),
                    embedding_types: vec!["float".into()]
                })
                .send()
        }).await??;

        // ... response parsing and conversion to WIT types ...
    }

    fn rerank(
        query: String,
        documents: Vec<String>,
        config: embed::Config
    ) -> Result<embed::RerankResponse, embed::Error> {
        // ... Cohere rerank implementation ...
    }
}

#[derive(Serialize)]
struct CohereRequest {
    texts: Vec<String>,
    model: String,
    input_type: Option<String>,
    embedding_types: Vec<String>
}

#[derive(Deserialize)]
struct CohereResponse {
    embeddings: Vec<Vec<f32>>,
    meta: CohereMetadata
}

#[derive(Deserialize)]
struct CohereMetadata {
    api_version: String
}