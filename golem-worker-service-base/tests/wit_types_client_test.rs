use golem_worker_service_base::{
    api::{
        routes::create_api_router,
        wit_types_api::WitTypesApi,
    },
    gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat},
    service::component::ComponentService,
    repo::api_definition::ApiDefinitionRepo,
    repo::api_deployment::ApiDeploymentRepo,
    service::gateway::security_scheme::SecuritySchemeService,
    service::gateway::api_definition_validator::ApiDefinitionValidatorService,
    gateway_api_definition::http::HttpApiDefinition,
    repo::api_definition::ApiDefinitionRecord,
    repo::api_deployment::ApiDeploymentRecord,
    service::component::ComponentServiceError,
    service::gateway::api_definition_validator::ValidationErrors,
    gateway_security::{SecurityScheme, SecuritySchemeWithProviderMetadata, SecuritySchemeIdentifier},
    service::gateway::security_scheme::SecuritySchemeServiceError,
};
use std::net::SocketAddr;
use poem::{
    Server, 
    middleware::Cors,
    EndpointExt,
    listener::TcpListener as PoemListener,
};
use serde_json::{Value, json};
use std::sync::Arc;
use async_trait::async_trait;
use golem_service_base::{
    auth::DefaultNamespace,
    model::Component,
    repo::RepoError,
};
use golem_common::model::{ComponentId, component_constraint::FunctionConstraintCollection};

// Mock implementations
struct MockComponentService;
#[async_trait]
impl ComponentService<DefaultNamespace> for MockComponentService {
    async fn get_by_version(
        &self,
        _component_id: &ComponentId,
        _version: u64,
        _auth_ctx: &DefaultNamespace,
    ) -> Result<Component, ComponentServiceError> {
        unimplemented!()
    }

    async fn get_latest(
        &self,
        _component_id: &ComponentId,
        _auth_ctx: &DefaultNamespace,
    ) -> Result<Component, ComponentServiceError> {
        unimplemented!()
    }

    async fn create_or_update_constraints(
        &self,
        _component_id: &ComponentId,
        _constraints: FunctionConstraintCollection,
        _auth_ctx: &DefaultNamespace,
    ) -> Result<FunctionConstraintCollection, ComponentServiceError> {
        unimplemented!()
    }
}

struct MockApiDefinitionRepo;
#[async_trait]
impl ApiDefinitionRepo for MockApiDefinitionRepo {
    async fn create(&self, _definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        unimplemented!()
    }

    async fn update(&self, _definition: &ApiDefinitionRecord) -> Result<(), RepoError> {
        unimplemented!()
    }

    async fn set_draft(
        &self,
        _namespace: &str,
        _id: &str,
        _version: &str,
        _draft: bool,
    ) -> Result<(), RepoError> {
        unimplemented!()
    }

    async fn get(
        &self,
        _namespace: &str,
        _id: &str,
        _version: &str,
    ) -> Result<Option<ApiDefinitionRecord>, RepoError> {
        unimplemented!()
    }

    async fn get_draft(
        &self,
        _namespace: &str,
        _id: &str,
        _version: &str,
    ) -> Result<Option<bool>, RepoError> {
        unimplemented!()
    }

    async fn delete(&self, _namespace: &str, _id: &str, _version: &str) -> Result<bool, RepoError> {
        unimplemented!()
    }

    async fn get_all(&self, _namespace: &str) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        unimplemented!()
    }

    async fn get_all_versions(
        &self,
        _namespace: &str,
        _id: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        unimplemented!()
    }
}

struct MockApiDeploymentRepo;
#[async_trait]
impl ApiDeploymentRepo for MockApiDeploymentRepo {
    async fn create(&self, _deployments: Vec<ApiDeploymentRecord>) -> Result<(), RepoError> {
        unimplemented!()
    }

    async fn delete(&self, _deployments: Vec<ApiDeploymentRecord>) -> Result<bool, RepoError> {
        unimplemented!()
    }

    async fn get_by_id(
        &self,
        _namespace: &str,
        _definition_id: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        unimplemented!()
    }

    async fn get_by_id_and_version(
        &self,
        _namespace: &str,
        _definition_id: &str,
        _definition_version: &str,
    ) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        unimplemented!()
    }

    async fn get_by_site(&self, _site: &str) -> Result<Vec<ApiDeploymentRecord>, RepoError> {
        unimplemented!()
    }

    async fn get_definitions_by_site(
        &self,
        _site: &str,
    ) -> Result<Vec<ApiDefinitionRecord>, RepoError> {
        unimplemented!()
    }
}

struct MockSecuritySchemeService;
#[async_trait]
impl SecuritySchemeService<DefaultNamespace> for MockSecuritySchemeService {
    async fn get(
        &self,
        _security_scheme_name: &SecuritySchemeIdentifier,
        _namespace: &DefaultNamespace,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        unimplemented!()
    }

    async fn create(
        &self,
        _namespace: &DefaultNamespace,
        _scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        unimplemented!()
    }
}

struct MockApiDefinitionValidatorService;
impl ApiDefinitionValidatorService<HttpApiDefinition> for MockApiDefinitionValidatorService {
    fn validate(
        &self,
        _api: &HttpApiDefinition,
        _components: &[Component],
    ) -> Result<(), ValidationErrors> {
        Ok(())
    }
}

async fn setup_golem_server() -> SocketAddr {
    println!("\n=== Setting up Golem server ===");
    println!("Creating API router...");
    
    // Bind to all interfaces (0.0.0.0)
    let bind_addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("Attempting to bind to address: {}", bind_addr);
    
    // Create mock services
    let component_service = Arc::new(MockComponentService);
    let definition_repo = Arc::new(MockApiDefinitionRepo);
    let deployment_repo = Arc::new(MockApiDeploymentRepo);
    let security_scheme_service = Arc::new(MockSecuritySchemeService);
    let api_definition_validator = Arc::new(MockApiDefinitionValidatorService);
    
    // Create base router with CORS and proper server URL
    let server_url = format!("http://0.0.0.0:{}", bind_addr.port());
    let app = create_api_router(
        Some(server_url),
        component_service,
        definition_repo,
        deployment_repo,
        security_scheme_service,
        api_definition_validator,
        None,
    ).await.expect("Failed to create API router")
        .with(Cors::new()
            .allow_origin("*")
            .allow_methods(["GET", "POST", "PUT", "DELETE", "OPTIONS"])
            .allow_headers(["content-type", "authorization", "accept"])
            .allow_credentials(false)
            .max_age(3600));
    
    // Debug: Print available routes
    println!("\nAvailable routes:");
    println!(" - /api/v1/doc (Main API spec)");
    println!(" - /api/wit-types/doc (WIT Types API spec)");
    println!(" - /swagger-ui (Main API docs)");
    println!(" - /swagger-ui/wit-types (WIT Types API docs)");
    println!(" - /api/wit-types/test");
    println!(" - /api/wit-types/sample");
    
    // Create Poem TCP listener
    let poem_listener = PoemListener::bind(bind_addr);
    println!("Created TCP listener");
    
    let server = Server::new(poem_listener);
    println!("Golem server configured with listener");
    
    // Use localhost for displaying the URL and health checks
    let localhost_addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Golem Swagger UI will be available at: http://{}/swagger-ui", localhost_addr);
    println!("WIT Types Swagger UI will be available at: http://{}/swagger-ui/wit-types", localhost_addr);
    
    // Start the server in a background task
    tokio::spawn(async move {
        println!("\n=== Starting Golem server ===");
        if let Err(e) = server.run(app).await {
            println!("Golem server error: {}", e);
        }
        println!("=== Golem server stopped ===");
    });
    
    // Wait for the server to be ready by checking the OpenAPI spec
    println!("\nEnsuring OpenAPI spec is available...");
    let client = reqwest::Client::new();
    let api_doc_url = format!("http://{}/api/wit-types/doc", localhost_addr);
    
    let mut attempts = 0;
    let max_attempts = 5;
    
    while attempts < max_attempts {
        println!("Checking API spec at: {}", api_doc_url);
        match client.get(&api_doc_url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    println!("✓ API spec is available");
                    break;
                } else {
                    println!("✗ API spec returned status: {}", response.status());
                }
            }
            Err(e) => {
                println!("✗ Failed to reach API spec: {}", e);
            }
        }
        
        if attempts < max_attempts - 1 {
            println!("Waiting 1 second before retry...");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
        attempts += 1;
    }

    if attempts == max_attempts {
        println!("Warning: API spec might not be fully ready after {} attempts", max_attempts);
    }

    println!("=== Golem server setup complete ===\n");
    localhost_addr
}

#[tokio::test]
async fn test_wit_types_client() -> anyhow::Result<()> {
    // Set up Golem server
    let addr = setup_golem_server().await;
    let base_url = format!("http://{}", addr);
    println!("Golem server running at: {}", base_url);

    // Create HTTP client
    let client = reqwest::Client::new();

    // First get and validate OpenAPI spec
    println!("\nValidating OpenAPI spec...");
    let api_response = client
        .get(&format!("{}/api/wit-types/doc", base_url))
        .send()
        .await?;
    println!("OpenAPI spec status: {}", api_response.status());
    assert!(api_response.status().is_success(), "Failed to get OpenAPI spec");
    
    let openapi_spec: Value = api_response.json().await?;
    validate_openapi_spec(&openapi_spec);
    println!("✓ OpenAPI spec validated successfully");

    // Test POST /primitives endpoint
    println!("\nTesting POST /api/wit-types/primitives endpoint...");
    let primitive_test_data = json!({
        "value": {
            "bool_val": true,
            "u8_val": 42,
            "u16_val": 1000,
            "u32_val": 100000,
            "u64_val": 1000000,
            "s8_val": -42,
            "s16_val": -1000,
            "s32_val": -100000,
            "s64_val": -1000000,
            "f32_val": 3.14,
            "f64_val": 3.14159,
            "char_val": 65,  // ASCII code for 'A' as a number
            "string_val": "test string"
        }
    });
    
    // Debug: Print the request payload
    println!("Request payload:");
    println!("{}", serde_json::to_string_pretty(&primitive_test_data).unwrap());
    
    let primitives_response = client
        .post(&format!("{}/api/wit-types/primitives", base_url))
        .json(&primitive_test_data)
        .send()
        .await?;
    println!("Primitives response status: {}", primitives_response.status());
    
    if !primitives_response.status().is_success() {
        let error_text = primitives_response.text().await?;
        println!("Error response: {}", error_text);
        
        // Try to parse the error as JSON for better formatting
        if let Ok(error_json) = serde_json::from_str::<Value>(&error_text) {
            println!("Parsed error response:");
            println!("{}", serde_json::to_string_pretty(&error_json).unwrap());
        }
        
        assert!(false, "Primitives request failed");
    } else {
        let primitives_json = primitives_response.json::<Value>().await?;
        validate_primitive_response(&primitives_json);
    }

    // Test POST /users/profile endpoint
    println!("\nTesting POST /api/wit-types/users/profile endpoint...");
    let profile_test_data = json!({
        "value": {
            "id": 1,
            "username": "testuser",
            "settings": {
                "theme": "dark",
                "notifications_enabled": true,
                "email_frequency": "daily"
            },
            "permissions": {
                "can_read": true,
                "can_write": true,
                "can_delete": false,
                "is_admin": false
            }
        }
    });
    
    let profile_response = client
        .post(&format!("{}/api/wit-types/users/profile", base_url))
        .json(&profile_test_data)
        .send()
        .await?;
    println!("Profile response status: {}", profile_response.status());
    
    if !profile_response.status().is_success() {
        let error_text = profile_response.text().await?;
        println!("Error response: {}", error_text);
        
        // Try to parse the error as JSON for better formatting
        if let Ok(error_json) = serde_json::from_str::<Value>(&error_text) {
            println!("Parsed error response:");
            println!("{}", serde_json::to_string_pretty(&error_json).unwrap());
        }
        
        assert!(false, "Profile request failed");
    } else {
        let profile_json = profile_response.json::<Value>().await?;
        validate_profile_response(&profile_json);
    }

    // Test POST /search endpoint
    println!("\nTesting POST /api/wit-types/search endpoint...");
    let search_test_data = json!({
        "value": {
            "matches": [
                {
                    "id": 1,
                    "score": 0.95,
                    "context": "Sample context 1"
                },
                {
                    "id": 2,
                    "score": 0.85,
                    "context": "Sample context 2"
                }
            ],
            "total_count": 2,
            "execution_time_ms": 100,
            "query": "test search",
            "filters": {
                "categories": ["category1", "category2"],
                "date_range": {
                    "start": 1000000,
                    "end": 2000000
                },
                "flags": {
                    "case_sensitive": true,
                    "whole_word": false,
                    "regex_enabled": true
                }
            },
            "pagination": {
                "page": 1,
                "items_per_page": 10
            }
        }
    });
    
    let search_response = client
        .post(&format!("{}/api/wit-types/search", base_url))
        .json(&search_test_data)
        .send()
        .await?;
    println!("Search response status: {}", search_response.status());
    
    if !search_response.status().is_success() {
        let error_text = search_response.text().await?;
        println!("Error response: {}", error_text);
        
        // Try to parse the error as JSON for better formatting
        if let Ok(error_json) = serde_json::from_str::<Value>(&error_text) {
            println!("Parsed error response:");
            println!("{}", serde_json::to_string_pretty(&error_json).unwrap());
        }
        
        assert!(false, "Search request failed");
    } else {
        let search_json = search_response.json::<Value>().await?;
        validate_search_response(&search_json);
    }

    // Test POST /batch endpoint
    println!("\nTesting POST /api/wit-types/batch endpoint...");
    let batch_test_data = json!({
        "value": {
            "successful": 5,
            "failed": 1,
            "errors": ["Error processing item 3"]
        }
    });
    
    let batch_response = client
        .post(&format!("{}/api/wit-types/batch", base_url))
        .json(&batch_test_data)
        .send()
        .await?;
    println!("Batch response status: {}", batch_response.status());
    
    if !batch_response.status().is_success() {
        let error_text = batch_response.text().await?;
        println!("Error response: {}", error_text);
        
        // Try to parse the error as JSON for better formatting
        if let Ok(error_json) = serde_json::from_str::<Value>(&error_text) {
            println!("Parsed error response:");
            println!("{}", serde_json::to_string_pretty(&error_json).unwrap());
        }
        
        assert!(false, "Batch request failed");
    } else {
        let batch_json = batch_response.json::<Value>().await?;
        validate_batch_response(&batch_json);
    }

    // Test POST /tree endpoint
    println!("\nTesting POST /api/wit-types/tree endpoint...");
    let tree_test_data = json!({
        "value": {
            "id": 1,
            "value": "root",
            "children": [
                {
                    "id": 2,
                    "value": "child1",
                    "children": [],
                    "metadata": {
                        "created_at": 1000000,
                        "modified_at": 1000000,
                        "tags": ["tag1"]
                    }
                }
            ],
            "metadata": {
                "created_at": 1000000,
                "modified_at": 1000000,
                "tags": ["root-tag"]
            }
        }
    });
    
    let tree_response = client
        .post(&format!("{}/api/wit-types/tree", base_url))
        .json(&tree_test_data)
        .send()
        .await?;
    println!("Tree response status: {}", tree_response.status());
    
    if !tree_response.status().is_success() {
        let error_text = tree_response.text().await?;
        println!("Error response: {}", error_text);
        
        // Try to parse the error as JSON for better formatting
        if let Ok(error_json) = serde_json::from_str::<Value>(&error_text) {
            println!("Parsed error response:");
            println!("{}", serde_json::to_string_pretty(&error_json).unwrap());
        }
        
        assert!(false, "Tree request failed");
    } else {
        let tree_json = tree_response.json::<Value>().await?;
        validate_tree_response(&tree_json);
    }

    // Test GET endpoints
    println!("\nTesting GET endpoints...");
    
    // Test GET /success
    let success_response = client
        .get(&format!("{}/api/wit-types/success", base_url))
        .send()
        .await?;
    println!("Success response status: {}", success_response.status());
    
    if !success_response.status().is_success() {
        let error_text = success_response.text().await?;
        println!("Error response: {}", error_text);
        assert!(false, "Success request failed");
    } else {
        let success_json = success_response.json::<Value>().await?;
        validate_success_response(&success_json);
    }
    
    // Test GET /error
    let error_response = client
        .get(&format!("{}/api/wit-types/error", base_url))
        .send()
        .await?;
    println!("Error response status: {}", error_response.status());
    
    if !error_response.status().is_success() {
        let error_text = error_response.text().await?;
        println!("Error response: {}", error_text);
        assert!(false, "Error request failed");
    } else {
        let error_json = error_response.json::<Value>().await?;
        validate_error_response(&error_json);
    }
    
    // Test GET /search/sample
    let search_sample_response = client
        .get(&format!("{}/api/wit-types/search/sample", base_url))
        .send()
        .await?;
    println!("Search sample response status: {}", search_sample_response.status());
    
    if !search_sample_response.status().is_success() {
        let error_text = search_sample_response.text().await?;
        println!("Error response: {}", error_text);
        assert!(false, "Search sample request failed");
    } else {
        let search_sample_json = search_sample_response.json::<Value>().await?;
        validate_search_sample_response(&search_sample_json);
    }
    
    // Test GET /batch/sample
    let batch_sample_response = client
        .get(&format!("{}/api/wit-types/batch/sample", base_url))
        .send()
        .await?;
    println!("Batch sample response status: {}", batch_sample_response.status());
    
    if !batch_sample_response.status().is_success() {
        let error_text = batch_sample_response.text().await?;
        println!("Error response: {}", error_text);
        assert!(false, "Batch sample request failed");
    } else {
        let batch_sample_json = batch_sample_response.json::<Value>().await?;
        validate_batch_sample_response(&batch_sample_json);
    }

    // Test existing GET /sample endpoint
    let sample_response = client
        .get(&format!("{}/api/wit-types/sample", base_url))
        .send()
        .await?;
    println!("Sample response status: {}", sample_response.status());
    assert!(sample_response.status().is_success(), "Sample request failed");

    // Export OpenAPI spec and SwaggerUI
    export_golem_swagger_ui(&base_url).await?;

    Ok(())
}

fn validate_openapi_spec(spec: &Value) {
    // Validate OpenAPI version
    assert_eq!(spec.get("openapi").and_then(|v| v.as_str()), Some("3.0.0"), 
        "Invalid OpenAPI version");

    // Validate info section
    let info = spec.get("info").expect("Missing info section");
    assert!(info.get("title").is_some(), "Missing API title");
    assert!(info.get("version").is_some(), "Missing API version");

    // Validate paths
    let paths = spec.get("paths").expect("Missing paths section");
    
    // Validate /primitives endpoint
    let primitives_path = paths.get("/api/wit-types/primitives").expect("Missing /primitives endpoint");
    let post = primitives_path.get("post").expect("Missing POST method for /primitives");
    assert!(post.get("requestBody").is_some(), "Missing request body schema");
    assert!(post.get("responses").is_some(), "Missing response schema");

    // Validate /search endpoint
    let search_path = paths.get("/api/wit-types/search").expect("Missing /search endpoint");
    let post = search_path.get("post").expect("Missing POST method for /search");
    assert!(post.get("requestBody").is_some(), "Missing request body schema");
    assert!(post.get("responses").is_some(), "Missing response schema");

    // Validate /batch endpoint
    let batch_path = paths.get("/api/wit-types/batch").expect("Missing /batch endpoint");
    let post = batch_path.get("post").expect("Missing POST method for /batch");
    assert!(post.get("requestBody").is_some(), "Missing request body schema");
    assert!(post.get("responses").is_some(), "Missing response schema");

    // Validate /tree endpoint
    let tree_path = paths.get("/api/wit-types/tree").expect("Missing /tree endpoint");
    let post = tree_path.get("post").expect("Missing POST method for /tree");
    assert!(post.get("requestBody").is_some(), "Missing request body schema");
    assert!(post.get("responses").is_some(), "Missing response schema");

    // Validate /success endpoint
    let success_path = paths.get("/api/wit-types/success").expect("Missing /success endpoint");
    assert!(success_path.get("get").is_some(), "Missing GET method for /success");

    // Validate /error endpoint
    let error_path = paths.get("/api/wit-types/error").expect("Missing /error endpoint");
    assert!(error_path.get("get").is_some(), "Missing GET method for /error");

    // Validate /search/sample endpoint
    let search_sample_path = paths.get("/api/wit-types/search/sample").expect("Missing /search/sample endpoint");
    assert!(search_sample_path.get("get").is_some(), "Missing GET method for /search/sample");

    // Validate /batch/sample endpoint
    let batch_sample_path = paths.get("/api/wit-types/batch/sample").expect("Missing /batch/sample endpoint");
    assert!(batch_sample_path.get("get").is_some(), "Missing GET method for /batch/sample");

    // Validate /sample endpoint
    let sample_path = paths.get("/api/wit-types/sample").expect("Missing /sample endpoint");
    assert!(sample_path.get("get").is_some(), "Missing GET method for /sample");

    // Validate components section
    let components = spec.get("components").expect("Missing components section");
    let schemas = components.get("schemas").expect("Missing schemas section");

    // Debug: Print available schemas
    println!("\nAvailable schemas:");
    if let Some(schemas_obj) = schemas.as_object() {
        for schema_name in schemas_obj.keys() {
            println!(" - {}", schema_name);
        }
    }

    // Validate required schemas
    assert!(schemas.get("WitInput").is_some(), "Missing WitInput schema");
    assert!(schemas.get("BatchOptions").is_some(), "Missing BatchOptions schema");
    assert!(schemas.get("BatchResult").is_some(), "Missing BatchResult schema");
    assert!(schemas.get("ComplexNestedTypes").is_some(), "Missing ComplexNestedTypes schema");
    assert!(schemas.get("NestedData").is_some(), "Missing NestedData schema");
    assert!(schemas.get("ValueObject").is_some(), "Missing ValueObject schema");
    assert!(schemas.get("PrimitiveTypes").is_some(), "Missing PrimitiveTypes schema");
    assert!(schemas.get("SearchMatch").is_some(), "Missing SearchMatch schema");
    assert!(schemas.get("SearchResult").is_some(), "Missing SearchResult schema");
    assert!(schemas.get("TreeNode").is_some(), "Missing TreeNode schema");
    assert!(schemas.get("NodeMetadata").is_some(), "Missing NodeMetadata schema");
}

fn validate_primitive_response(data: &Value) {
    assert!(data.get("bool_val").is_some(), "Missing bool_val field");
    assert!(data.get("u8_val").is_some(), "Missing u8_val field");
    assert!(data.get("u16_val").is_some(), "Missing u16_val field");
    assert!(data.get("u32_val").is_some(), "Missing u32_val field");
    assert!(data.get("u64_val").is_some(), "Missing u64_val field");
    assert!(data.get("s8_val").is_some(), "Missing s8_val field");
    assert!(data.get("s16_val").is_some(), "Missing s16_val field");
    assert!(data.get("s32_val").is_some(), "Missing s32_val field");
    assert!(data.get("s64_val").is_some(), "Missing s64_val field");
    assert!(data.get("f32_val").is_some(), "Missing f32_val field");
    assert!(data.get("f64_val").is_some(), "Missing f64_val field");
    
    // For char_val, we expect it to be a number in the response
    let char_val = data.get("char_val").expect("Missing char_val field");
    assert!(char_val.is_number(), "char_val should be a number in the response");
    
    assert!(data.get("string_val").is_some(), "Missing string_val field");
}

fn validate_profile_response(data: &Value) {
    assert!(data.get("id").is_some(), "Missing id field");
    assert!(data.get("username").is_some(), "Missing username field");
    
    let settings = data.get("settings").expect("Missing settings field");
    if settings.is_object() {
        let settings_obj = settings.as_object().unwrap();
        assert!(settings_obj.get("theme").is_some(), "Missing theme in settings");
        assert!(settings_obj.get("notifications_enabled").is_some(), "Missing notifications_enabled in settings");
        assert!(settings_obj.get("email_frequency").is_some(), "Missing email_frequency in settings");
    }
    
    let permissions = data.get("permissions").expect("Missing permissions field");
    let permissions_obj = permissions.as_object().unwrap();
    assert!(permissions_obj.get("can_read").is_some(), "Missing can_read in permissions");
    assert!(permissions_obj.get("can_write").is_some(), "Missing can_write in permissions");
    assert!(permissions_obj.get("can_delete").is_some(), "Missing can_delete in permissions");
    assert!(permissions_obj.get("is_admin").is_some(), "Missing is_admin in permissions");
}

fn validate_search_response(data: &Value) {
    assert!(data.get("matches").is_some(), "Missing matches field");
    assert!(data.get("total_count").is_some(), "Missing total_count field");
    assert!(data.get("execution_time_ms").is_some(), "Missing execution_time_ms field");
    
    let matches = data.get("matches").unwrap().as_array().unwrap();
    if !matches.is_empty() {
        let first_match = &matches[0];
        assert!(first_match.get("id").is_some(), "Missing id in match");
        assert!(first_match.get("score").is_some(), "Missing score in match");
        assert!(first_match.get("context").is_some(), "Missing context in match");
    }
}

fn validate_batch_response(data: &Value) {
    assert!(data.get("successful").is_some(), "Missing successful field");
    assert!(data.get("failed").is_some(), "Missing failed field");
    assert!(data.get("errors").is_some(), "Missing errors field");
}

fn validate_tree_response(data: &Value) {
    assert!(data.get("id").is_some(), "Missing id field");
    assert!(data.get("value").is_some(), "Missing value field");
    assert!(data.get("children").is_some(), "Missing children field");
    
    let metadata = data.get("metadata").expect("Missing metadata field");
    let metadata_obj = metadata.as_object().unwrap();
    assert!(metadata_obj.get("created_at").is_some(), "Missing created_at in metadata");
    assert!(metadata_obj.get("modified_at").is_some(), "Missing modified_at in metadata");
    assert!(metadata_obj.get("tags").is_some(), "Missing tags in metadata");
}

fn validate_success_response(data: &Value) {
    assert!(data.get("code").is_some(), "Missing code field");
    assert!(data.get("message").is_some(), "Missing message field");
    assert!(data.get("data").is_some(), "Missing data field");
}

fn validate_error_response(data: &Value) {
    assert!(data.get("code").is_some(), "Missing code field");
    assert!(data.get("message").is_some(), "Missing message field");
    assert!(data.get("details").is_some(), "Missing details field");
}

fn validate_search_sample_response(data: &Value) {
    assert!(data.get("query").is_some(), "Missing query field");
    
    let filters = data.get("filters").expect("Missing filters field");
    let filters_obj = filters.as_object().unwrap();
    assert!(filters_obj.get("categories").is_some(), "Missing categories in filters");
    assert!(filters_obj.get("date_range").is_some(), "Missing date_range in filters");
    assert!(filters_obj.get("flags").is_some(), "Missing flags in filters");
    
    if let Some(pagination) = data.get("pagination") {
        let pagination_obj = pagination.as_object().unwrap();
        assert!(pagination_obj.get("page").is_some(), "Missing page in pagination");
        assert!(pagination_obj.get("items_per_page").is_some(), "Missing items_per_page in pagination");
    }
}

fn validate_batch_sample_response(data: &Value) {
    assert!(data.get("parallel").is_some(), "Missing parallel field");
    assert!(data.get("retry_count").is_some(), "Missing retry_count field");
    assert!(data.get("timeout_ms").is_some(), "Missing timeout_ms field");
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

    let client = reqwest::Client::new();
    let exporter = OpenApiExporter;

    // Export WIT Types API spec in both JSON and YAML
    println!("\nExporting OpenAPI specs...");
    
    // JSON format
    let json_format = OpenApiFormat { json: true };
    let wit_types_json = exporter.export_openapi(WitTypesApi, &json_format);
    let json_path = export_dir.join("wit_types_api.json");
    std::fs::write(&json_path, wit_types_json)?;
    println!("✓ Exported JSON spec to: {}", json_path.display());

    // YAML format
    let yaml_format = OpenApiFormat { json: false };
    let wit_types_yaml = exporter.export_openapi(WitTypesApi, &yaml_format);
    let yaml_path = export_dir.join("wit_types_api.yaml");
    std::fs::write(&yaml_path, wit_types_yaml)?;
    println!("✓ Exported YAML spec to: {}", yaml_path.display());

    // Export Swagger UI
    let swagger_ui_response = client
        .get(&format!("{}/swagger-ui/wit-types", base_url))
        .send()
        .await?;
    
    let swagger_ui_html = swagger_ui_response.text().await?;
    let swagger_ui_path = export_dir.join("swagger_ui.html");
    std::fs::write(&swagger_ui_path, swagger_ui_html)?;
    println!("✓ Exported Swagger UI to: {}", swagger_ui_path.display());

    Ok(())
} 