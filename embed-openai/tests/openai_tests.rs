use golem_durability::mock::*;
use embed_openai::embed::*;
use serial_test::serial;

#[test]
#[serial]
fn test_generate_embeddings() {
    let mut mock = MockDurability::new();
    mock.expect_log_operation()
        .times(1)
        .returning(|_| Ok(()));

    let inputs = vec![ContentPart::text("test input".into())];
    let config = Config {
        model: Some("text-embedding-3-large".into()),
        dimensions: Some(1024),
        ..Default::default()
    };

    let result = EmbedComponent::generate(inputs, config);
    assert!(result.is_ok());
    let response = result.unwrap();
    assert!(!response.embeddings.is_empty());
}

#[test]
#[serial]
fn test_missing_api_key() {
    let mut mock = MockDurability::new();
    mock.expect_log_operation()
        .times(0);

    env::remove_var("OPENAI_API_KEY");
    
    let result = EmbedComponent::generate(
        vec![ContentPart::text("test".into())],
        Config::default()
    );
    
    assert!(matches!(result, Err(Error { code: ErrorCode::InvalidRequest, .. })));
}