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
        let api_key = env::var("VOYAGEAI_API_KEY")
            .map_err(|_| embed::Error::invalid("Missing VOYAGEAI_API_KEY"))?;

        let client = Client::new();
        let response = durability::wrap(|| {
            client.post("https://api.voyageai.com/v1/embeddings")
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&VoyageAIRequest {
                    input: inputs.iter().map(|p| p.to_string()).collect(),
                    model: config.model.unwrap_or("voyage-large-2".into()),
                    input_type: config.task_type.map(|t| t.to_string()),
                    truncation: config.truncation.unwrap_or(true)
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
        // ... Voyage AI rerank implementation ...
    }
}

#[derive(Serialize)]
struct VoyageAIRequest {
    input: Vec<String>,
    model: String,
    input_type: Option<String>,
    truncation: bool
}

#[derive(Deserialize)]
struct VoyageAIResponse {
    data: Vec<VoyageAIEmbedding>,
    model: String
}

#[derive(Deserialize)]
struct VoyageAIEmbedding {
    index: u32,
    embedding: Vec<f32>
}