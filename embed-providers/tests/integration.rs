#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use golem_api_grpc::proto::grpc::EmbeddingDurabilityApi;

    #[test]
    #[serial]
    fn test_all_providers() {
        let providers = vec![
            ("OPENAI", "text-embedding-3-large", env::var("OPENAI_API_KEY")),
            ("COHERE", "embed-english-v3.0", env::var("COHERE_API_KEY")),
            // ... other providers
        ];

        for (provider, model, api_key) in providers {
            // Environment validation
            if api_key.is_err() {
                panic!("Missing {} API key in environment variables", provider);
            }

            // Test embedding generation
            let result = EmbedComponent::generate(
                vec![ContentPart::text("test")],
                Config {
                    model: Some(model.into()),
                    ..Default::default()
                }
            );
            
            assert!(result.is_ok(), "{} failed: {:?}", provider, result);

            // Durability check through Golem API
            let embedding_id = result.unwrap().id;
            let durability_check = EmbeddingDurabilityApi::new()
                .verify_embedding(embedding_id)
                .await;
            
            assert!(durability_check.is_ok(), "{} durability failed: {:?}", provider, durability_check);
        }
    }
}