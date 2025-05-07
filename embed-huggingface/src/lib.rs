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
        let api_key = env::var("HUGGINGFACE_API_KEY")
            .map_err(|_| embed::Error::invalid("Missing HUGGINGFACE_API_KEY"))?;

        let client = Client::new();
        let response = durability::wrap(|| {
            client.post("https://api-inference.huggingface.co/pipeline/feature-extraction/sentence-transformers")
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&HuggingFaceRequest {
                    inputs: inputs.iter().map(|p| p.to_string()).collect(),
                    options: HuggingFaceOptions {
                        wait_for_model: true,
                        model: config.model.unwrap_or_else(|| "sentence-transformers/all-MiniLM-L6-v2".into())
                    }
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
        // ... Hugging Face rerank implementation ...
    }
}

#[derive(Serialize)]
struct HuggingFaceRequest {
    inputs: Vec<String>,
    options: HuggingFaceOptions
}

#[derive(Serialize)]
struct HuggingFaceOptions {
    wait_for_model: bool,
    model: String
}

#[derive(Deserialize)]
struct HuggingFaceResponse {
    embeddings: Vec<Vec<f32>>
}