use golem_worker_service_base::gateway_api_definition::http::client_generator::ClientGenerator;
use serde_json::json;
use std::fs;
use tempfile::tempdir;
use tokio;

#[tokio::test]
async fn test_rust_client_endpoints() {
    // Set up test client
    let temp_dir = tempdir().unwrap();
    let api_yaml = include_str!("fixtures/test_api_definition.yaml");
    let openapi = serde_yaml::from_str(api_yaml).unwrap();
    
    let generator = ClientGenerator::new(temp_dir.path());
    let client_dir = generator
        .generate_rust_client("test-api", "1.0.0", openapi, "test_client")
        .await
        .unwrap();

    // Create test file that exercises all endpoints
    let test_file_content = r#"
use test_client::apis::configuration::Configuration;
use test_client::apis::default_api::*;

#[tokio::test]
async fn test_all_endpoints() {
    let config = Configuration {
        base_path: "http://localhost:8080".to_string(),
        ..Default::default()
    };

    // Test healthcheck endpoint
    let health = get_health_check(&config).await.unwrap();
    assert!(health.is_object());
    assert!(health.as_object().unwrap().is_empty());

    // Test version endpoint
    let version = get_version(&config).await.unwrap();
    assert_eq!(version.version, "1.0.0");

    // Test API definition export
    let api_def = export_api_definition(&config, "test-api", "1.0.0").await.unwrap();
    assert_eq!(api_def.openapi, "3.1.0");
    assert_eq!(api_def.info.title, "test-api API");
    assert_eq!(api_def.info.version, "1.0.0");

    // Test search endpoint
    let search_result = perform_search(&config, json!({
        "query": "test",
        "filters": {
            "categories": ["test"],
            "date_range": {
                "start": 1234567890,
                "end": 1234567890
            },
            "flags": {
                "case_sensitive": true,
                "whole_word": true,
                "regex_enabled": false
            }
        },
        "pagination": {
            "page": 1,
            "items_per_page": 10
        }
    })).await.unwrap();
    
    assert!(!search_result.matches.is_empty());
    assert_eq!(search_result.total_count, 1);
    assert!(search_result.execution_time_ms > 0);

    // Test tree endpoint
    let tree = query_tree(&config, 1, Some(2)).await.unwrap();
    assert_eq!(tree.id, 1);
    assert_eq!(tree.value, "root");
    assert!(!tree.children.is_empty());
    assert!(tree.metadata.is_some());
    
    let metadata = tree.metadata.unwrap();
    assert_eq!(metadata.created_at, Some(1234567890));
    assert_eq!(metadata.modified_at, Some(1234567890));
    assert_eq!(metadata.tags, Some(vec!["test".to_string()]));

    // Test batch operations
    let batch_result = process_batch(&config, vec!["test1".to_string(), "test2".to_string()])
        .await
        .unwrap();
    assert_eq!(batch_result.successful, 1);
    assert_eq!(batch_result.failed, 0);
    assert!(batch_result.errors.is_empty());

    // Test batch validation
    let validation_result = validate_batch(&config, vec!["test1".to_string(), "test2".to_string()])
        .await
        .unwrap();
    assert!(!validation_result.is_empty());
    assert!(validation_result[0].ok);

    // Test batch status
    let status = get_batch_status(&config, 1).await.unwrap();
    assert_eq!(status.id, 1);
    assert!(status.progress >= 0);
    assert!(status.successful >= 0);
    assert!(status.failed >= 0);

    // Test transformation endpoints
    let transform_result = apply_transformation(&config, json!({
        "data": ["test1", "test2"],
        "transformation": {
            "Sort": {
                "field": "value",
                "ascending": true
            }
        }
    })).await.unwrap();
    
    assert!(transform_result.success);
    assert!(!transform_result.output.is_empty());
    assert!(transform_result.metrics.input_size > 0);
    assert!(transform_result.metrics.output_size > 0);
    assert!(transform_result.metrics.duration_ms >= 0);

    // Test chain transformations
    let chain_result = chain_transformations(&config, json!({
        "data": ["test1", "test2"],
        "transformations": [
            {
                "Sort": {
                    "field": "value",
                    "ascending": true
                }
            },
            {
                "Filter": {
                    "predicate": "length > 0"
                }
            }
        ]
    })).await.unwrap();
    
    assert!(chain_result.success);
    assert!(!chain_result.output.is_empty());
    assert!(chain_result.metrics.input_size > 0);
    assert!(chain_result.metrics.output_size > 0);
    assert!(chain_result.metrics.duration_ms >= 0);

    println!("All Rust client tests passed successfully!");
}
"#;

    fs::write(client_dir.join("tests/integration_test.rs"), test_file_content).unwrap();

    // Create test directory if it doesn't exist
    fs::create_dir_all(client_dir.join("tests")).unwrap();

    // Run the tests
    let status = tokio::process::Command::new(if cfg!(windows) { "cargo.exe" } else { "cargo" })
        .args(["test", "--manifest-path"])
        .arg(client_dir.join("Cargo.toml"))
        .status()
        .await
        .unwrap();

    assert!(status.success(), "Rust client tests failed");
    println!("Rust client tests completed successfully!");
} 