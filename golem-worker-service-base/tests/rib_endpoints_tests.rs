use poem::{test::TestClient, Route};
use serde_json::Value;
use golem_worker_service_base::api::rib_endpoints::rib_routes;
use golem_worker_service_base::api::swagger_ui::{SwaggerUiConfig, SwaggerUiAuthConfig};

#[tokio::test]
async fn test_healthcheck() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/api/healthcheck").send().await;
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

    let resp = cli.get("/api/version").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("version_str")).is_some());
}

#[tokio::test]
async fn test_get_primitive_types() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/api/primitives").send().await;
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

    let resp = cli.post("/api/primitives")
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

    let resp = cli.get("/api/users/1/profile").send().await;
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

    let resp = cli.post("/api/users/1/settings")
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
    let swagger_config = SwaggerUiConfig {
        enabled: true,
        title: Some("RIB API".to_string()),
        version: Some("1.0".to_string()),
        server_url: Some("http://localhost:3000".to_string()),
        auth: SwaggerUiAuthConfig::default(),
        worker_binding: None,
        golem_extensions: std::collections::HashMap::new(),
    };

    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test the Swagger UI endpoint
    let resp = cli.get("/swagger-ui/rib")
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let html = body.into_string().await.unwrap();
    
    // Verify key elements are present in the Swagger UI HTML
    assert!(html.contains("swagger-ui"), "Response should contain swagger-ui");
    assert!(html.contains("RIB API"), "Response should contain API title");
    assert!(html.contains("http://localhost:3000"), "Response should contain server URL");

    // Add debug output
    if !html.contains("swagger-ui") || !html.contains("RIB API") || !html.contains("http://localhost:3000") {
        println!("Swagger UI response HTML: {}", html);
    }
}

#[tokio::test]
async fn test_error_response() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test with invalid user ID to trigger error
    let resp = cli.get("/api/users/999999/profile").send().await;
    
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
    let test_node = serde_json::json!({
        "root": create_test_tree_node()
    });

    let resp = cli.post("/api/tree")
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
    let resp = cli.get("/api/tree/1?depth=2")
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

    // Test batch process
    let batch_items = serde_json::json!({
        "items": [
            {"id": 1, "action": "update"},
            {"id": 2, "action": "delete"}
        ]
    });

    let resp = cli.post("/api/batch/process")
        .body_json(&batch_items)
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

    // Test the API spec endpoint
    let resp = cli.get("/api/openapi")
        .header("Accept", "application/json")
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    
    // The OpenAPI spec is returned directly
    assert!(body.get("openapi").is_some(), "OpenAPI spec should contain 'openapi' field");
    assert!(body.get("info").is_some(), "OpenAPI spec should contain 'info' field");
    assert!(body.get("paths").is_some(), "OpenAPI spec should contain 'paths' field");
    
    // Verify that our endpoints are documented
    let paths = body.get("paths").unwrap().as_object().unwrap();
    assert!(!paths.is_empty(), "OpenAPI spec should contain API paths");
    
    // Verify that the components section exists
    let components = body.get("components").unwrap().as_object().unwrap();
    assert!(!components.is_empty(), "OpenAPI spec should contain components");
}

#[tokio::test]
async fn test_user_permissions() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    let resp = cli.get("/api/users/1/permissions").send().await;
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

    let resp = cli.post("/api/content")
        .body_json(&test_content)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());

    // Test get content
    let resp = cli.get("/api/content/1").send().await;
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
            "type": "content",
            "date_range": {
                "start": "2023-01-01",
                "end": "2023-12-31"
            }
        }
    });

    let resp = cli.post("/api/search")
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
    let resp = cli.post("/api/search/validate")
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

    // Test batch validate
    let batch_items = serde_json::json!({
        "items": [
            {"id": 1, "action": "update"},
            {"id": 2, "action": "delete"}
        ]
    });

    let resp = cli.post("/api/batch/validate")
        .body_json(&batch_items)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("valid")).is_some());

    // Test batch status
    let resp = cli.get("/api/batch/1/status").send().await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("status")).is_some());
    assert!(body.get("data").and_then(|d| d.get("progress")).is_some());
}

#[tokio::test]
async fn test_transform_operations() {
    let app = Route::new().nest("/", rib_routes());
    let cli = TestClient::new(app);

    // Test transform
    let transform_request = serde_json::json!({
        "input": "test input",
        "transformations": [
            {"type": "uppercase"}
        ]
    });

    let resp = cli.post("/api/transform")
        .body_json(&transform_request)
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

    // Test transform chain
    let resp = cli.post("/api/transform/chain")
        .body_json(&transform_request)
        .send()
        .await;
    assert!(resp.0.status().is_success());
    
    let (_, body) = resp.0.into_parts();
    let response_str = body.into_string().await.unwrap();
    let body: Value = serde_json::from_str(&response_str).unwrap();
    assert!(body.get("status").is_some());
    assert!(body.get("data").and_then(|d| d.get("success")).is_some());
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

    let resp = cli.post("/api/tree/modify")
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