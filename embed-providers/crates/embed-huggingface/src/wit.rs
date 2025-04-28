use crate::HuggingFaceClient;
use embed_common::wit::{generate_embeddings, rerank};
use golem_embed::embed::{Config, ContentPart, EmbeddingResponse, Error, RerankResponse};

wit_bindgen::generate!({
    path: "../../wit",
    world: "embed",
});

struct EmbedComponent;

impl Guest for EmbedComponent {
    fn generate(inputs: Vec<ContentPart>, config: Config) -> Result<EmbeddingResponse, Error> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error {
                code: golem_embed::embed::ErrorCode::InternalError,
                message: e.to_string(),
                provider_error_json: None,
            })?;

        let client = rt.block_on(async {
            HuggingFaceClient::new().map_err(|e| Error {
                code: golem_embed::embed::ErrorCode::InternalError,
                message: e.to_string(),
                provider_error_json: None,
            })
        })?;

        rt.block_on(generate_embeddings(&client, inputs, config))
    }

    fn rerank(query: String, documents: Vec<String>, config: Config) -> Result<RerankResponse, Error> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error {
                code: golem_embed::embed::ErrorCode::InternalError,
                message: e.to_string(),
                provider_error_json: None,
            })?;

        let client = rt.block_on(async {
            HuggingFaceClient::new().map_err(|e| Error {
                code: golem_embed::embed::ErrorCode::InternalError,
                message: e.to_string(),
                provider_error_json: None,
            })
        })?;

        rt.block_on(rerank(&client, query, documents, config))
    }
}

export_embed!(EmbedComponent);