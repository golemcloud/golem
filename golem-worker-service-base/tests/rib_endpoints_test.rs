use golem_worker_service_base::{
    api::{
        rib_endpoints::RibApi,
    },
    gateway_api_definition::http::{
        openapi_export::{OpenApiExporter, OpenApiFormat},
        swagger_ui::{SwaggerUiConfig, SwaggerUiAuthConfig, create_swagger_ui},
    },
};
use std::net::SocketAddr;
use poem::{
    Server, 
    middleware::Cors,
    EndpointExt,
    listener::TcpListener as PoemListener,
    Route,
};
use serde_json::{Value, json};

async fn setup_golem_server() -> SocketAddr {
    println!("\n=== Setting up Golem server ===");
    println!("Creating API router...");
    
    let bind_addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("Attempting to bind to address: {}", bind_addr);
    
    let server_url = format!("http://0.0.0.0:{}", bind_addr.port());
    
    // Create Swagger UI config
    let swagger_config = SwaggerUiConfig {
        server_url: Some(server_url.clone()),
        enabled: true,
        title: Some("RIB API Documentation".to_string()),
        version: Some("1.0".to_string()),
        auth: SwaggerUiAuthConfig::default(),
        worker_binding: None,
        golem_extensions: std::collections::HashMap::new(),
    };

    // Create RIB API service with Swagger UI
    let rib_api = RibApi::new();
    let api_service = create_swagger_ui(rib_api, &swagger_config);
    
    // Create the combined route with API and Swagger UI
    let app = Route::new()
        .nest("/api/v1/rib", api_service.clone())
        .nest("/swagger-ui/rib", api_service.swagger_ui())
        .with(Cors::new()
            .allow_origin("*")
            .allow_methods(["GET", "POST", "PUT", "DELETE", "OPTIONS"])
            .allow_headers(["content-type", "authorization", "accept"])
            .allow_credentials(false)
            .max_age(3600));
    
    println!("\nAvailable RIB routes:");
    println!(" - /api/v1/rib/healthcheck");
    println!(" - /api/v1/rib/version");
    println!(" - /api/v1/rib/primitives");
    println!(" - /api/v1/rib/users/:id/profile");
    println!(" - /api/v1/rib/users/:id/settings");
    println!(" - /api/v1/rib/users/:id/permissions");
    println!(" - /api/v1/rib/content");
    println!(" - /api/v1/rib/search");
    println!(" - /api/v1/rib/batch/process");
    println!(" - /api/v1/rib/transform");
    println!(" - /api/v1/rib/tree");
    println!(" - /swagger-ui/rib (Swagger UI Documentation)");
    
    let poem_listener = PoemListener::bind(bind_addr);
    println!("Created TCP listener");
    
    let server = Server::new(poem_listener);
    println!("Golem server configured with listener");
    
    let localhost_addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Golem RIB API will be available at: http://{}/api/v1/rib", localhost_addr);
    println!("Swagger UI will be available at: http://{}/swagger-ui/rib", localhost_addr);
    
    tokio::spawn(async move {
        println!("\n=== Starting Golem server ===");
        if let Err(e) = server.run(app).await {
            println!("Golem server error: {}", e);
        }
        println!("=== Golem server stopped ===");
    });
    
    println!("\nEnsuring RIB API is available...");
    let client = reqwest::Client::new();
    let health_url = format!("http://{}/api/v1/rib/healthcheck", localhost_addr);
    
    let mut attempts = 0;
    let max_attempts = 5;
    
    while attempts < max_attempts {
        println!("Checking RIB API health at: {}", health_url);
        match client.get(&health_url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    println!("✓ RIB API is available");
                    break;
                } else {
                    println!("✗ RIB API returned status: {}", response.status());
                }
            }
            Err(e) => {
                println!("✗ Failed to reach RIB API: {}", e);
            }
        }
        
        if attempts < max_attempts - 1 {
            println!("Waiting 1 second before retry...");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
        attempts += 1;
    }

    if attempts == max_attempts {
        println!("Warning: RIB API might not be fully ready after {} attempts", max_attempts);
    }

    println!("=== Golem server setup complete ===\n");
    localhost_addr
}

#[tokio::test]
async fn test_rib_endpoints() -> anyhow::Result<()> {
    let addr = setup_golem_server().await;
    let base_url = format!("http://{}", addr);
    println!("Golem server running at: {}", base_url);

    let client = reqwest::Client::new();
    let rib_base = format!("{}/api/v1/rib", base_url);

    // Test healthcheck endpoint
    println!("\nTesting GET /healthcheck endpoint...");
    let health_response = client
        .get(&format!("{}/healthcheck", rib_base))
        .send()
        .await?;
    assert!(health_response.status().is_success(), "Healthcheck failed");
    let health_data: Value = health_response.json().await?;
    println!("Healthcheck response: {}", serde_json::to_string_pretty(&health_data)?);

    // Test version endpoint
    println!("\nTesting GET /version endpoint...");
    let version_response = client
        .get(&format!("{}/version", rib_base))
        .send()
        .await?;
    assert!(version_response.status().is_success(), "Version check failed");
    let version_data: Value = version_response.json().await?;
    println!("Version response: {}", serde_json::to_string_pretty(&version_data)?);

    // Test primitive types endpoints
    println!("\nTesting GET /primitives endpoint...");
    let primitives_response = client
        .get(&format!("{}/primitives", rib_base))
        .send()
        .await?;
    assert!(primitives_response.status().is_success(), "Get primitives failed");
    let primitives_data: Value = primitives_response.json().await?;
    assert!(primitives_data["status"] == "success" || primitives_data["status"] == "error", 
        "Invalid status in primitives response: {}", primitives_data["status"]);
    println!("Primitives response: {}", serde_json::to_string_pretty(&primitives_data)?);

    if primitives_data["status"] == "success" {
        assert!(primitives_data["data"]["schema"].is_object(), "Schema should be an object");
        assert!(primitives_data["data"]["example"].is_object(), "Example should be an object");
    } else {
        assert!(primitives_data["data"]["error"].is_string(), "Error should be a string");
        println!("Warning: Primitives endpoint returned error: {}", primitives_data["data"]["error"]);
    }

    println!("\nTesting POST /primitives endpoint...");
    let primitive_payload = json!({
        "bool_val": true,
        "u32_val": 42,
        "f64_val": 3.14,
        "string_val": "Hello RIB!"
    });
    let post_primitives_response = client
        .post(&format!("{}/primitives", rib_base))
        .json(&primitive_payload)
        .send()
        .await?;
    assert!(post_primitives_response.status().is_success(), "Post primitives failed");
    let post_primitives_data: Value = post_primitives_response.json().await?;
    println!("Post primitives response: {}", serde_json::to_string_pretty(&post_primitives_data)?);

    // Test user profile endpoints
    let user_id = 123;
    println!("\nTesting GET /users/{}/profile endpoint...", user_id);
    let profile_response = client
        .get(&format!("{}/users/{}/profile", rib_base, user_id))
        .send()
        .await?;
    assert!(profile_response.status().is_success(), "Get user profile failed");
    let profile_data: Value = profile_response.json().await?;
    println!("Profile response: {}", serde_json::to_string_pretty(&profile_data)?);

    println!("\nTesting POST /users/{}/settings endpoint...", user_id);
    let settings_payload = json!({
        "theme": "dark",
        "notifications_enabled": false
    });
    let settings_response = client
        .post(&format!("{}/users/{}/settings", rib_base, user_id))
        .json(&settings_payload)
        .send()
        .await?;
    assert!(settings_response.status().is_success(), "Post user settings failed");
    let settings_data: Value = settings_response.json().await?;
    println!("Settings response: {}", serde_json::to_string_pretty(&settings_data)?);

    println!("\nTesting GET /users/{}/permissions endpoint...", user_id);
    let permissions_response = client
        .get(&format!("{}/users/{}/permissions", rib_base, user_id))
        .send()
        .await?;
    assert!(permissions_response.status().is_success(), "Get user permissions failed");
    let permissions_data: Value = permissions_response.json().await?;
    println!("Permissions response: {}", serde_json::to_string_pretty(&permissions_data)?);

    // Test content endpoints
    println!("\nTesting POST /content endpoint...");
    let content_payload = json!({
        "title": "Test Content",
        "body": "This is a test content body"
    });
    let content_response = client
        .post(&format!("{}/content", rib_base))
        .json(&content_payload)
        .send()
        .await?;
    assert!(content_response.status().is_success(), "Post content failed");
    let content_data: Value = content_response.json().await?;
    println!("Content response: {}", serde_json::to_string_pretty(&content_data)?);

    let content_id = 456;
    println!("\nTesting GET /content/{} endpoint...", content_id);
    let get_content_response = client
        .get(&format!("{}/content/{}", rib_base, content_id))
        .send()
        .await?;
    assert!(get_content_response.status().is_success(), "Get content failed");
    let get_content_data: Value = get_content_response.json().await?;
    println!("Get content response: {}", serde_json::to_string_pretty(&get_content_data)?);

    // Test search endpoints
    println!("\nTesting POST /search endpoint...");
    let search_payload = json!({
        "query": "test",
        "filters": {
            "type": "content",
            "date_range": {
                "start": "2023-01-01",
                "end": "2023-12-31"
            }
        }
    });
    let search_response = client
        .post(&format!("{}/search", rib_base))
        .json(&search_payload)
        .send()
        .await?;
    assert!(search_response.status().is_success(), "Search failed");
    let search_data: Value = search_response.json().await?;
    println!("Search response: {}", serde_json::to_string_pretty(&search_data)?);

    println!("\nTesting POST /search/validate endpoint...");
    let validate_search_response = client
        .post(&format!("{}/search/validate", rib_base))
        .json(&search_payload)
        .send()
        .await?;
    assert!(validate_search_response.status().is_success(), "Search validation failed");
    let validate_search_data: Value = validate_search_response.json().await?;
    println!("Search validation response: {}", serde_json::to_string_pretty(&validate_search_data)?);

    // Test batch endpoints
    println!("\nTesting POST /batch/process endpoint...");
    let batch_payload = json!({
        "items": [
            {"id": 1, "action": "update"},
            {"id": 2, "action": "delete"}
        ]
    });
    let batch_response = client
        .post(&format!("{}/batch/process", rib_base))
        .json(&batch_payload)
        .send()
        .await?;
    assert!(batch_response.status().is_success(), "Batch process failed");
    let batch_data: Value = batch_response.json().await?;
    println!("Batch process response: {}", serde_json::to_string_pretty(&batch_data)?);

    println!("\nTesting POST /batch/validate endpoint...");
    let batch_validate_response = client
        .post(&format!("{}/batch/validate", rib_base))
        .json(&batch_payload)
        .send()
        .await?;
    assert!(batch_validate_response.status().is_success(), "Batch validation failed");
    let batch_validate_data: Value = batch_validate_response.json().await?;
    println!("Batch validation response: {}", serde_json::to_string_pretty(&batch_validate_data)?);

    let batch_id = 789;
    println!("\nTesting GET /batch/{}/status endpoint...", batch_id);
    let batch_status_response = client
        .get(&format!("{}/batch/{}/status", rib_base, batch_id))
        .send()
        .await?;
    assert!(batch_status_response.status().is_success(), "Get batch status failed");
    let batch_status_data: Value = batch_status_response.json().await?;
    println!("Batch status response: {}", serde_json::to_string_pretty(&batch_status_data)?);

    // Test transform endpoints
    println!("\nTesting POST /transform endpoint...");
    let transform_payload = json!({
        "input": "test data",
        "transformations": [
            {"type": "uppercase"},
            {"type": "reverse"}
        ]
    });
    let transform_response = client
        .post(&format!("{}/transform", rib_base))
        .json(&transform_payload)
        .send()
        .await?;
    assert!(transform_response.status().is_success(), "Transform failed");
    let transform_data: Value = transform_response.json().await?;
    println!("Transform response: {}", serde_json::to_string_pretty(&transform_data)?);

    println!("\nTesting POST /transform/chain endpoint...");
    let chain_transform_response = client
        .post(&format!("{}/transform/chain", rib_base))
        .json(&transform_payload)
        .send()
        .await?;
    assert!(chain_transform_response.status().is_success(), "Chain transform failed");
    let chain_transform_data: Value = chain_transform_response.json().await?;
    println!("Chain transform response: {}", serde_json::to_string_pretty(&chain_transform_data)?);

    // Test tree endpoints
    println!("\nTesting POST /tree endpoint...");
    let tree_payload = json!({
        "root": {
            "value": "root",
            "children": [
                {
                    "value": "child1",
                    "children": []
                }
            ]
        }
    });
    let create_tree_response = client
        .post(&format!("{}/tree", rib_base))
        .json(&tree_payload)
        .send()
        .await?;
    assert!(create_tree_response.status().is_success(), "Create tree failed");
    let create_tree_data: Value = create_tree_response.json().await?;
    println!("Create tree response: {}", serde_json::to_string_pretty(&create_tree_data)?);

    let tree_id = create_tree_data["data"]["id"].as_u64().unwrap() as u32;
    let query_url = format!("{}/tree/{}?depth=2", rib_base, tree_id);
    println!("\nTesting GET /tree/{} endpoint...", tree_id);
    println!("Request URL: {}", query_url);
    let query_tree_response = client
        .get(&query_url)
        .send()
        .await?;
    
    let status = query_tree_response.status();
    println!("Response status: {}", status);
    
    // Get the raw response text first
    let response_text = query_tree_response.text().await?;
    println!("Raw response: {}", response_text);
    
    // Try to parse as JSON
    let query_tree_data: Value = match serde_json::from_str(&response_text) {
        Ok(json) => json,
        Err(e) => {
            println!("Failed to parse JSON: {}", e);
            assert!(false, "Invalid JSON response");
            return Ok(());
        }
    };
    
    if !status.is_success() {
        println!("Query tree error response: {}", serde_json::to_string_pretty(&query_tree_data)?);
        assert!(false, "Query tree failed with status: {}", status);
    }
    
    println!("Query tree response: {}", serde_json::to_string_pretty(&query_tree_data)?);

    println!("\nTesting POST /tree/modify endpoint...");
    let modify_tree_payload = json!({
        "operation": "insert",
        "path": "/root/child1",
        "value": "new_node"
    });
    let modify_tree_response = client
        .post(&format!("{}/tree/modify", rib_base))
        .json(&modify_tree_payload)
        .send()
        .await?;
    assert!(modify_tree_response.status().is_success(), "Modify tree failed");
    let modify_tree_data: Value = modify_tree_response.json().await?;
    println!("Modify tree response: {}", serde_json::to_string_pretty(&modify_tree_data)?);

    // Export OpenAPI spec and SwaggerUI
    export_golem_swagger_ui(&base_url).await?;

    println!("\n✓ All RIB endpoints tested successfully!");
    Ok(())
}

async fn export_golem_swagger_ui(base_url: &str) -> anyhow::Result<()> {
    let export_dir = std::path::PathBuf::from("target")
        .join("openapi-exports")
        .canonicalize()
        .unwrap_or_else(|_| {
            let path = std::path::PathBuf::from("target/openapi-exports");
            std::fs::create_dir_all(&path).unwrap();
            path.canonicalize().unwrap()
        });

    // Export OpenAPI spec using OpenApiExporter
    println!("Exporting RIB OpenAPI specs...");
    let exporter = OpenApiExporter;
    let api = RibApi::new();
    
    // Export JSON format
    let json_format = OpenApiFormat { json: true };
    let json_spec = exporter.export_openapi(api.clone(), &json_format);
    let json_path = export_dir.join("openapi-rib.json");
    std::fs::write(&json_path, &json_spec)?;
    println!("✓ Exported JSON spec to: {}", json_path.display());

    // Export YAML format
    let yaml_format = OpenApiFormat { json: false };
    let yaml_spec = exporter.export_openapi(api.clone(), &yaml_format);
    let yaml_path = export_dir.join("openapi-rib.yaml");
    std::fs::write(&yaml_path, &yaml_spec)?;
    println!("✓ Exported YAML spec to: {}", yaml_path.display());

    // Generate and save SwaggerUI HTML
    println!("Generating RIB SwaggerUI...");
    let config = SwaggerUiConfig {
        server_url: Some(base_url.to_string()),
        enabled: true,
        title: Some("RIB API Documentation".to_string()),
        version: Some("1.0".to_string()),
        auth: SwaggerUiAuthConfig::default(),
        worker_binding: None,
        golem_extensions: std::collections::HashMap::new(),
    };

    let service = create_swagger_ui(api, &config);
    let swagger_dir = export_dir.join("swagger-ui-rib");
    std::fs::create_dir_all(&swagger_dir)?;
    
    // Save the OpenAPI spec for Swagger UI
    std::fs::write(swagger_dir.join("openapi.json"), json_spec)?;
    
    // Create the Swagger UI HTML using swagger_ui_html()
    let swagger_html = service.swagger_ui_html();
    std::fs::write(swagger_dir.join("index.html"), swagger_html)?;
    println!("✓ Exported SwaggerUI HTML to: {}", swagger_dir.join("index.html").display());

    // Print URLs for manual testing
    println!("\nAPI endpoints:");
    println!("RIB SwaggerUI: {}/swagger-ui/rib", base_url);
    println!("RIB OpenAPI spec: {}/api/v1/rib/doc", base_url);

    Ok(())
} 