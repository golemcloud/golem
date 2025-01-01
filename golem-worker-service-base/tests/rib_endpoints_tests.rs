use poem::{test::TestClient, Route};
use serde_json::Value;
use golem_worker_service_base::api::rib_endpoints::rib_routes;
use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{SwaggerUiConfig, generate_swagger_ui};

#[tokio::test]
async fn test_healthcheck() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/healthcheck").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
}

#[tokio::test]
async fn test_version() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/version").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("version")).is_some());
}

#[tokio::test]
async fn test_get_primitive_types() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/primitives").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("schema")).is_some());
    assert!(body.get("data").and_then(|d| d.get("example")).is_some());
}

#[tokio::test]
async fn test_create_primitive_types() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let test_data = serde_json::json!({
        "bool_val": true,
        "u32_val": 42,
        "f64_val": 3.14,
        "string_val": "Test"
    });

    let resp = cli.post("/primitives")
        .body_json(&test_data)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
}

#[tokio::test]
async fn test_get_user_profile() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/users/1/profile").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("schema")).is_some());
    assert!(body.get("data").and_then(|d| d.get("profile")).is_some());
}

#[tokio::test]
async fn test_update_user_settings() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let test_settings = serde_json::json!({
        "theme": "dark",
        "notifications_enabled": true
    });

    let resp = cli.post("/users/1/settings")
        .body_json(&test_settings)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
}

#[tokio::test]
async fn test_swagger_ui_integration() {
    let config = SwaggerUiConfig {
        enabled: true,
        path: "/api-docs".to_string(),
        title: Some("RIB API Documentation".to_string()),
        theme: Some("dark".to_string()),
        api_id: "rib-api".to_string(),
        version: "1.0".to_string(),
    };

    let html = generate_swagger_ui(&config);
    assert!(html.contains("RIB API Documentation"));
    assert!(html.contains("swagger-ui"));
    assert!(html.contains("background-color: #1a1a1a"));
    assert!(html.contains("filter: invert(88%) hue-rotate(180deg)"));
    assert!(html.contains(r#"syntaxHighlight: { theme: "monokai" }"#));
}

#[tokio::test]
async fn test_error_response() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test with invalid user ID to trigger error
    let resp = cli.get("/users/999999/profile").send().await;
    
    if !resp.0.status().is_success() {
        let (_, body) = resp.0.into_parts();
        let response_str = body.into_string().await.unwrap();
        let body: Value = serde_json::from_str(&response_str).unwrap();
        assert!(body.get("status").is_some());
        assert_eq!(body.get("status").unwrap().as_str().unwrap(), "error");
    }
}

// Helper function to create test data
fn create_test_tree_node() -> Value {
    serde_json::json!({
        "id": 1,
        "value": "root",
        "children": [],
        "metadata": {
            "created_at": 1234567890,
            "modified_at": 1234567890,
            "tags": ["test"]
        }
    })
}

#[tokio::test]
async fn test_tree_operations() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test create tree
    let test_node = create_test_tree_node();
    let resp = cli.post("/tree")
        .body_json(&test_node)
        .send()
        .await;
    
    // Add debug info
    println!("Create tree response status: {:?}", resp.0.status());
    println!("Create tree response headers: {:?}", resp.0.headers());
    let (status, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    println!("Create tree response body: {}", response_str);
    assert!(status.status.is_success(), "Create tree failed with status: {:?} and body: {}", status.status, response_str);

    // Test query tree
    let resp = cli.get(&format!("/tree/1?depth=2"))
        .send()
        .await;
    
    // Add debug info
    println!("Query tree response status: {:?}", resp.0.status());
    println!("Query tree response headers: {:?}", resp.0.headers());
    let (status, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    println!("Query tree response body: {}", response_str);
    assert!(status.status.is_success(), "Query tree failed with status: {:?} and body: {}", status.status, response_str);
}

#[tokio::test]
async fn test_batch_operations() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let batch_data = serde_json::json!({
        "items": ["item1", "item2"],
        "options": {
            "parallel": true,
            "retry_count": 3,
            "timeout_ms": 5000
        }
    });

    let resp = cli.post("/batch/process")
        .body_json(&batch_data)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("successful")).is_some());
    assert!(body.get("data").and_then(|d| d.get("failed")).is_some());
}

#[tokio::test]
async fn test_export_api_definition() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/v1/api/definitions/test-api/version/1.0/export").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("openapi")).is_some());
    assert!(body.get("data").and_then(|d| d.get("info")).is_some());
}

#[tokio::test]
async fn test_user_permissions() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/users/1/permissions").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("permissions")).is_some());
}

#[tokio::test]
async fn test_content_operations() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test create content
    let test_content = serde_json::json!({
        "title": "Test Content",
        "body": "This is test content"
    });

    let resp = cli.post("/content")
        .body_json(&test_content)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());

    // Test get content
    let resp = cli.get("/content/1").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("content")).is_some());
}

#[tokio::test]
async fn test_search_operations() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test search
    let search_query = serde_json::json!({
        "query": "test",
        "filters": {
            "categories": ["test"],
            "date_range": {
                "start": 1234567890,
                "end": 1234567899
            },
            "flags": {
                "case_sensitive": true,
                "whole_word": false,
                "regex_enabled": false
            }
        },
        "pagination": {
            "page": 1,
            "items_per_page": 10
        }
    });

    let resp = cli.post("/search")
        .body_json(&search_query)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("matches")).is_some());
    assert!(body.get("data").and_then(|d| d.get("total_count")).is_some());
    assert!(body.get("data").and_then(|d| d.get("execution_time_ms")).is_some());

    // Test search validation
    let resp = cli.post("/search/validate")
        .body_json(&search_query)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("valid")).is_some());
}

#[tokio::test]
async fn test_batch_validation_and_status() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test batch validation
    let batch_data = serde_json::json!([
        "item1",
        "item2"
    ]);

    let resp = cli.post("/batch/validate")
        .body_json(&batch_data)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());

    // Test batch status
    let resp = cli.get("/batch/1/status").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("status")).is_some());
    assert!(body.get("data").and_then(|d| d.get("progress")).is_some());
    assert!(body.get("data").and_then(|d| d.get("successful")).is_some());
    assert!(body.get("data").and_then(|d| d.get("failed")).is_some());
}

#[tokio::test]
async fn test_transform_operations() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test single transformation
    let transform_data = serde_json::json!({
        "data": ["item1", "item2"],
        "transformation": {
            "Sort": {
                "field": "name",
                "ascending": true
            }
        }
    });

    let resp = cli.post("/transform")
        .body_json(&transform_data)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("success")).is_some());
    assert!(body.get("data").and_then(|d| d.get("output")).is_some());
    assert!(body.get("data").and_then(|d| d.get("metrics")).is_some());

    // Test transformation chain
    let chain_data = serde_json::json!({
        "data": ["item1", "item2"],
        "transformations": [
            {
                "Sort": {
                    "field": "name",
                    "ascending": true
                }
            },
            {
                "Filter": {
                    "predicate": "length > 0"
                }
            }
        ]
    });

    let resp = cli.post("/transform/chain")
        .body_json(&chain_data)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("success")).is_some());
    assert!(body.get("data").and_then(|d| d.get("output")).is_some());
    assert!(body.get("data").and_then(|d| d.get("metrics")).is_some());
}

#[tokio::test]
async fn test_tree_modify() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let modify_operation = serde_json::json!({
        "Insert": {
            "parent_id": 1,
            "node": {
                "id": 2,
                "value": "child",
                "children": [],
                "metadata": {
                    "created_at": 1234567890,
                    "modified_at": 1234567890,
                    "tags": ["test-child"]
                }
            }
        }
    });

    let resp = cli.post("/tree/modify")
        .body_json(&modify_operation)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("success")).is_some());
    assert!(body.get("data").and_then(|d| d.get("operation_type")).is_some());
    assert!(body.get("data").and_then(|d| d.get("nodes_affected")).is_some());
} 