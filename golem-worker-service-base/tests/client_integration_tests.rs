use golem_worker_service_base::{
    service::component::ComponentService,
    service::gateway::api_definition_validator::{ApiDefinitionValidatorService, ValidationErrors},
    service::gateway::security_scheme::SecuritySchemeService,
    repo::api_definition::ApiDefinitionRepo,
    repo::api_deployment::ApiDeploymentRepo,
    gateway_api_definition::http::HttpApiDefinition,
    gateway_api_definition::http::client_generator::ClientGenerator,
    api::create_api_router,
    api::routes::create_cors_middleware,
    gateway_security::{SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata, Provider, GolemIdentityProviderMetadata},
};
use golem_common::model::{ComponentId, HasAccountId, AccountId};
use golem_service_base::{model::Component, repo::RepoError};
use async_trait::async_trait;
use std::{net::SocketAddr, fs};
use tokio;
use poem_openapi::{
    OpenApi,
    Object,
    Tags,
    payload::Json,
    OpenApiService,
};
use reqwest;
use tempfile::TempDir;
use serde::{Serialize, Deserialize};
use poem::{Server, listener::TcpListener as PoemTcpListener, EndpointExt};
use std::sync::Arc;
use std::fmt::Display;
use golem_service_base::auth::DefaultNamespace;
use golem_worker_service_base::service::gateway::security_scheme::SecuritySchemeServiceError;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::collections::HashMap;

// Simple namespace type for testing
#[derive(Debug, Clone, Default)]
struct TestNamespace;

impl Display for TestNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "test")
    }
}

impl TryFrom<String> for TestNamespace {
    type Error = String;
    fn try_from(_value: String) -> Result<Self, Self::Error> {
        Ok(TestNamespace)
    }
}

impl HasAccountId for TestNamespace {
    fn account_id(&self) -> AccountId {
        AccountId::generate()
    }
}

// Mock implementations for test services
#[derive(Default)]
struct TestComponentService;

#[async_trait]
impl<AuthCtx> ComponentService<AuthCtx> for TestComponentService 
where 
    AuthCtx: Send + Sync + Default + 'static 
{
    async fn get_by_version(&self, _id: &ComponentId, _version: u64, _auth_ctx: &AuthCtx) -> Result<Component, golem_worker_service_base::service::component::ComponentServiceError> {
        use golem_common::model::component_metadata::ComponentMetadata;
        use golem_service_base::model::{ComponentName, VersionedComponentId};
        use chrono::Utc;

        let id = VersionedComponentId {
            component_id: ComponentId::try_from("urn:uuid:12345678-1234-5678-1234-567812345678").unwrap(),
            version: 0,
        };

        Ok(Component {
            versioned_component_id: id,
            component_name: ComponentName("test".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
                memories: vec![],
                dynamic_linking: HashMap::new(),
            },
            created_at: Some(Utc::now()),
            component_type: None,
            files: vec![],
            installed_plugins: vec![],
        })
    }

    async fn get_latest(&self, _id: &ComponentId, _auth_ctx: &AuthCtx) -> Result<Component, golem_worker_service_base::service::component::ComponentServiceError> {
        self.get_by_version(_id, 0, _auth_ctx).await
    }

    async fn create_or_update_constraints(
        &self,
        _id: &ComponentId,
        _constraints: golem_common::model::component_constraint::FunctionConstraintCollection,
        _auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::component_constraint::FunctionConstraintCollection, golem_worker_service_base::service::component::ComponentServiceError> {
        Ok(golem_common::model::component_constraint::FunctionConstraintCollection {
            function_constraints: vec![]
        })
    }
}

#[derive(Default)]
struct TestApiDefinitionRepo;

#[async_trait]
impl ApiDefinitionRepo for TestApiDefinitionRepo {
    async fn create(&self, _record: &golem_worker_service_base::repo::api_definition::ApiDefinitionRecord) -> Result<(), RepoError> {
        Ok(())
    }

    async fn update(&self, _record: &golem_worker_service_base::repo::api_definition::ApiDefinitionRecord) -> Result<(), RepoError> {
        Ok(())
    }

    async fn set_draft(&self, _namespace: &str, _id: &str, _version: &str, _is_draft: bool) -> Result<(), RepoError> {
        Ok(())
    }

    async fn get(&self, _namespace: &str, _id: &str, _version: &str) -> Result<Option<golem_worker_service_base::repo::api_definition::ApiDefinitionRecord>, RepoError> {
        Ok(None)
    }

    async fn get_draft(&self, _namespace: &str, _id: &str, _version: &str) -> Result<Option<bool>, RepoError> {
        Ok(None)
    }

    async fn delete(&self, _namespace: &str, _id: &str, _version: &str) -> Result<bool, RepoError> {
        Ok(true)
    }

    async fn get_all(&self, _namespace: &str) -> Result<Vec<golem_worker_service_base::repo::api_definition::ApiDefinitionRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_all_versions(&self, _namespace: &str, _id: &str) -> Result<Vec<golem_worker_service_base::repo::api_definition::ApiDefinitionRecord>, RepoError> {
        Ok(vec![])
    }
}

#[derive(Default)]
struct TestApiDeploymentRepo;

#[async_trait]
impl ApiDeploymentRepo for TestApiDeploymentRepo {
    async fn create(&self, _records: Vec<golem_worker_service_base::repo::api_deployment::ApiDeploymentRecord>) -> Result<(), RepoError> {
        Ok(())
    }

    async fn delete(&self, _records: Vec<golem_worker_service_base::repo::api_deployment::ApiDeploymentRecord>) -> Result<bool, RepoError> {
        Ok(true)
    }

    async fn get_by_id(&self, _namespace: &str, _id: &str) -> Result<Vec<golem_worker_service_base::repo::api_deployment::ApiDeploymentRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_by_id_and_version(&self, _namespace: &str, _id: &str, _version: &str) -> Result<Vec<golem_worker_service_base::repo::api_deployment::ApiDeploymentRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_by_site(&self, _site: &str) -> Result<Vec<golem_worker_service_base::repo::api_deployment::ApiDeploymentRecord>, RepoError> {
        Ok(vec![])
    }

    async fn get_definitions_by_site(&self, _site: &str) -> Result<Vec<golem_worker_service_base::repo::api_definition::ApiDefinitionRecord>, RepoError> {
        Ok(vec![])
    }
}

#[derive(Default)]
struct TestSecuritySchemeService;

#[async_trait]
impl SecuritySchemeService<DefaultNamespace> for TestSecuritySchemeService {
    async fn get(
        &self,
        security_scheme_name: &SecuritySchemeIdentifier,
        _namespace: &DefaultNamespace,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        // Create a test security scheme with Google provider
        let security_scheme = SecurityScheme::new(
            Provider::Google,
            security_scheme_name.clone(),
            ClientId::new("test_client_id".to_string()),
            ClientSecret::new("test_client_secret".to_string()),
            RedirectUrl::new("http://localhost:3000/auth/callback".to_string())
                .map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?,
            vec![
                Scope::new("openid".to_string()),
                Scope::new("user".to_string()),
                Scope::new("email".to_string()),
            ],
        );

        // Create provider metadata
        let provider_metadata = serde_json::from_str::<GolemIdentityProviderMetadata>(r#"{
            "issuer": "https://accounts.google.com",
            "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
            "token_endpoint": "https://oauth2.googleapis.com/token",
            "userinfo_endpoint": "https://openidconnect.googleapis.com/v1/userinfo",
            "jwks_uri": "https://www.googleapis.com/oauth2/v3/certs"
        }"#).map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?;

        Ok(SecuritySchemeWithProviderMetadata {
            security_scheme,
            provider_metadata,
        })
    }

    async fn create(
        &self,
        _namespace: &DefaultNamespace,
        security_scheme: &SecurityScheme,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        // For testing, just wrap the input scheme with test provider metadata
        let provider_metadata = serde_json::from_str::<GolemIdentityProviderMetadata>(r#"{
            "issuer": "https://accounts.google.com",
            "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
            "token_endpoint": "https://oauth2.googleapis.com/token",
            "userinfo_endpoint": "https://openidconnect.googleapis.com/v1/userinfo",
            "jwks_uri": "https://www.googleapis.com/oauth2/v3/certs"
        }"#).map_err(|e| SecuritySchemeServiceError::InternalError(e.to_string()))?;

        Ok(SecuritySchemeWithProviderMetadata {
            security_scheme: security_scheme.clone(),
            provider_metadata,
        })
    }
}

#[derive(Default)]
struct TestApiDefinitionValidatorService;

impl ApiDefinitionValidatorService<HttpApiDefinition> for TestApiDefinitionValidatorService {
    fn validate(&self, _api: &HttpApiDefinition, _components: &[Component]) -> Result<(), ValidationErrors> {
        Ok(())
    }
}

#[derive(Object, Serialize, Deserialize)]
struct HealthcheckResponse {
    status: String,
    data: VersionData,
}

#[derive(Object, Serialize, Deserialize)]
struct VersionData {
    version: String,
}

#[derive(Tags)]
enum ApiTags {
    /// Test API operations
    Test,
}

/// API Documentation
#[derive(Default, Clone)]
struct ApiDoc;

#[OpenApi]
impl ApiDoc {
    /// Get service health status
    #[oai(path = "/healthcheck", method = "get", tag = "ApiTags::Test")]
    async fn healthcheck(&self) -> Json<HealthcheckResponse> {
        Json(HealthcheckResponse {
            status: "ok".to_string(),
            data: VersionData {
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })
    }
}

async fn setup_test_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    println!("Setting up test server...");

    let server_url = "http://localhost:8080".to_string();
    
    // Create OpenAPI service with proper server URL
    let api_doc = ApiDoc::default();
    let api_service = OpenApiService::new(api_doc.clone(), "Test API", "1.0.0")
        .server(server_url.clone())
        .url_prefix("/api/v1");

    // Create UI endpoint using Poem's built-in Swagger UI
    let ui = api_service.swagger_ui();
    
    // Create Poem route using the OpenAPI service and apply CORS
    let app = poem::Route::new()
        .nest("/api/v1", api_service.with(create_cors_middleware()))
        .nest("/swagger-ui", ui.with(create_cors_middleware()))
        .with(create_cors_middleware());

    // Start server using Poem
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    println!("Binding to address: {}", addr);
    let listener = PoemTcpListener::bind(addr);
    let server = Server::new(listener);
    println!("Server bound to: {}", addr);
    
    let handle = tokio::spawn(async move {
        println!("Starting server...");
        if let Err(e) = server.run(app).await {
            println!("Server error: {}", e);
        }
        println!("Server stopped.");
    });
    
    // Give the server a moment to start up
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    (addr, handle)
}

async fn setup_golem_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    println!("\n=== Setting up Golem server ===");
    println!("Creating API router...");
    
    let bind_addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("Attempting to bind to address: {}", bind_addr);
    
    let server_url = format!("http://127.0.0.1:{}", bind_addr.port());
    println!("Setting server URL to: {}", server_url);

    let component_service = Arc::new(TestComponentService::default());
    let definition_repo = Arc::new(TestApiDefinitionRepo::default());
    let deployment_repo = Arc::new(TestApiDeploymentRepo::default());
    let security_scheme_service = Arc::new(TestSecuritySchemeService::default());
    let api_definition_validator = Arc::new(TestApiDefinitionValidatorService::default());
    
    let app = create_api_router::<golem_service_base::auth::EmptyAuthCtx>(
        Some(server_url.clone()),
        component_service,
        definition_repo,
        deployment_repo,
        security_scheme_service,
        api_definition_validator,
        None,
    ).await.expect("Failed to create API router");

    // Configure CORS for Swagger UI and API endpoints
    let app = app.with(create_cors_middleware());

    // Create Poem TCP listener
    let listener = PoemTcpListener::bind(bind_addr);
    println!("Created TCP listener");
    
    println!("Configuring server with routes:");
    println!(" - /api/v1/swagger-ui -> Health/RIB API Swagger UI");
    println!(" - /api/wit-types/swagger-ui -> WIT Types API Swagger UI");
    println!(" - /api/openapi -> RIB API spec");
    println!(" - /api/v1/doc/openapi.json -> Health API spec");
    println!(" - /api/wit-types/doc -> WIT Types API spec");
    
    let server = Server::new(listener);
    println!("Golem server configured with listener");
    
    // Use localhost for displaying the URL and health checks
    let localhost_addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Golem server will be available at: http://{}", localhost_addr);
    println!("Golem Swagger UIs will be available at:");
    println!(" - http://{}/api/v1/swagger-ui", localhost_addr);
    println!(" - http://{}/api/wit-types/swagger-ui", localhost_addr);
    
    // Start the server in a background task
    let handle = tokio::spawn(async move {
        println!("\n=== Starting Golem server ===");
        if let Err(e) = server.run(app).await {
            println!("Golem server error: {}", e);
        }
        println!("=== Golem server stopped ===");
    });
    
    // Wait for the server to be ready by attempting to connect
    println!("Waiting for server to be ready...");
    let client = reqwest::Client::new();
    let mut attempts = 0;
    let max_attempts = 5;
    
    while attempts < max_attempts {
        match client.get(format!("http://{}/api/v1/doc/openapi.json", localhost_addr)).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    println!("Server is ready! Health API spec is accessible.");
                    // Try to get the actual content
                    match response.text().await {
                        Ok(content) => {
                            println!("Health API spec content length: {} bytes", content.len());
                            if content.len() < 100 {
                                println!("Warning: Health API spec content seems too small: {}", content);
                            }
                        }
                        Err(e) => println!("Warning: Could not read Health API spec content: {}", e)
                    }
                    break;
                } else {
                    println!("Health API spec returned status: {}", response.status());
                }
            }
            Err(e) => {
                println!("Attempt {} failed: {}", attempts + 1, e);
                if attempts < max_attempts - 1 {
                    println!("Retrying in 1 second...");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
        attempts += 1;
    }
    
    if attempts == max_attempts {
        println!("Warning: Server might not be fully ready after {} attempts", max_attempts);
    }
    
    println!("=== Golem server setup complete ===\n");
    (localhost_addr, handle)
}

#[tokio::test]
async fn test_generated_clients() -> anyhow::Result<()> {
    // Initialize tracing for debugging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .init();
    
    tracing::info!("Starting test_generated_clients...");

    // Start Golem server first
    let (golem_addr, golem_handle) = setup_golem_server().await;
    tracing::info!("Golem server running at: http://{}", golem_addr);
    
    // Set up test server
    let (addr, server_handle) = setup_test_server().await;
    let base_url = format!("http://{}", addr);
    println!("Test server running at: {}", base_url);

    // Check if test API definition exists
    let api_def_path = std::path::Path::new("tests/fixtures/test_api_definition.yaml");
    if !api_def_path.exists() {
        return Err(anyhow::anyhow!(
            "Test API definition file not found at: {}",
            api_def_path.display()
        ));
    }
    
    // Create output directory in the project workspace for OpenAPI specs
    println!("Creating output directory...");
    let output_dir = std::path::Path::new("generated_clients");
    fs::create_dir_all(output_dir)?;
    println!("Output directory created at: {:?}", output_dir);

    // Create temporary directory for client generation
    println!("Creating temporary directory for client generation...");
    let temp_dir = TempDir::new()?;
    println!("Temporary directory created at: {:?}", temp_dir.path());

    // Fetch OpenAPI specs from all endpoints
    println!("\nFetching OpenAPI specs from all endpoints...");
    let client = reqwest::Client::new();
    
    // Define API endpoints
    let api_endpoints = [
        ("Health API", format!("http://{}/api/v1/doc", golem_addr)),
        ("RIB API", format!("http://{}/api/openapi", golem_addr)),
        ("WIT Types API", format!("http://{}/api/wit-types/doc", golem_addr))
    ];

    // Test each endpoint
    let mut server_openapi = None;
    for (name, url) in &api_endpoints {
        println!("\nFetching {} spec from: {}", name, url);
        match client.get(url).send().await {
            Ok(response) => {
                println!("{} status: {}", name, response.status());
                if response.status().is_success() {
                    let spec: serde_json::Value = response.json().await?;
                    println!("✓ {} spec fetched successfully", name);
                    
                    // Store the Health API spec for client generation
                    if *name == "Health API" {
                        server_openapi = Some(spec);
                    }
                } else {
                    println!("✗ {} returned error status", name);
                    println!("Response body: {}", response.text().await?);
                }
            }
            Err(e) => {
                println!("✗ Failed to fetch {} spec: {}", name, e);
            }
        }
    }

    let server_openapi = server_openapi.ok_or_else(|| 
        anyhow::anyhow!("Failed to fetch Health API spec"))?;

    // Create OpenAPI service for testing
    println!("\nCreating OpenAPI service...");
    let api_doc = ApiDoc::default();
    let api_service = OpenApiService::new(api_doc.clone(), "Test API", "1.0.0")
        .server(&base_url);

    // Save OpenAPI specs
    println!("Saving OpenAPI specs...");
    let json_string = serde_json::to_string_pretty(&server_openapi)
        .map_err(|e| anyhow::anyhow!("Failed to serialize OpenAPI spec: {}", e))?;
    fs::write(
        output_dir.join("server_openapi.json"),
        json_string
    )?;
    println!("Exported server JSON spec to: {:?}", output_dir.join("server_openapi.json"));

    fs::write(
        output_dir.join("server_openapi.yaml"),
        api_service.spec_yaml()
    )?;
    println!("Exported server YAML spec to: {:?}", output_dir.join("server_openapi.yaml"));

    // Set up client generator with temp directory
    println!("Setting up client generator...");
    let generator = ClientGenerator::new(temp_dir.path());

    // Generate Rust client
    println!("Generating Rust client...");
    let rust_client_result = generator
        .generate_rust_client("test-api", "1.0.0", api_doc.clone(), "test_client")
        .await;
    
    match rust_client_result {
        Ok(rust_client_dir) => {
            println!("Rust client generated at: {:?}", rust_client_dir);

            // Create test package for Rust client
            println!("Creating test package...");
            let test_dir = rust_client_dir.join("integration-tests");
            fs::create_dir_all(&test_dir)?;
            fs::create_dir_all(test_dir.join("src"))?;
            fs::create_dir_all(test_dir.join("tests"))?;
            println!("Test directories created");

            // Verify Rust client structure
            println!("Verifying Rust client structure...");
            assert!(rust_client_dir.exists());
            assert!(rust_client_dir.join("Cargo.toml").exists());
            assert!(rust_client_dir.join("src/lib.rs").exists());
            assert!(rust_client_dir.join("src/apis").exists());
            assert!(rust_client_dir.join("src/models").exists());
            println!("Rust client structure verified");
        }
        Err(e) => {
            println!("Error generating Rust client: {:?}", e);
            // Print the OpenAPI spec for debugging
            println!("OpenAPI spec:");
            println!("{}", api_service.spec());
            return Err(e.into());
        }
    }

    // Generate TypeScript client
    println!("Generating TypeScript client...");
    let ts_client_result = generator
        .generate_typescript_client("test-api", "1.0.0", api_doc.clone(), "@test/client")
        .await;
    
    match ts_client_result {
        Ok(ts_client_dir) => {
            println!("TypeScript client generated at: {:?}", ts_client_dir);

            // Verify TypeScript client structure
            println!("Verifying TypeScript client structure...");
            assert!(ts_client_dir.exists());
            assert!(ts_client_dir.join("package.json").exists());
            assert!(ts_client_dir.join("src").exists());
            assert!(ts_client_dir.join("src/apis").exists());
            assert!(ts_client_dir.join("src/models").exists());
            println!("TypeScript client structure verified");
        }
        Err(e) => {
            println!("Error generating TypeScript client: {:?}", e);
            // Print the OpenAPI spec for debugging
            println!("OpenAPI spec:");
            println!("{}", api_service.spec());
            return Err(e.into());
        }
    }

    // Test CORS and middleware configuration
    println!("\nTesting CORS and middleware configuration...");
    
    // Test endpoints to check CORS
    let cors_test_endpoints = [
        ("/api/openapi", "RIB API"),
        ("/api/v1/doc/openapi.json", "Health API"),
        ("/api/wit-types/doc", "WIT Types API"),
    ];

    // Test API requests with enhanced debugging
    println!("\n=== Testing API Requests with Enhanced Debugging ===");

    let api_test_requests = [
        (
            "/api/v1/healthcheck",
            "GET",
            None,
            "Health API healthcheck"
        ),
        (
            "/api/version",
            "GET",
            None,
            "RIB API version"
        ),
        (
            "/api/wit-types/test",
            "POST",
            Some(r#"{"value": {"optional_numbers": [1, 2, null, 3], "feature_flags": 42, "nested_data": {"name": "test_name", "values": [{"string_val": "value1"}, {"string_val": "value2"}], "metadata": "optional metadata"}}}"#),
            "WIT Types test endpoint"
        ),
    ];

    for (endpoint, method, payload, description) in &api_test_requests {
        println!("\n=== Testing {} request: {} ===", method, description);
        println!("Endpoint: {}", endpoint);
        if let Some(p) = payload {
            println!("Payload: {}", p);
        }

        // First test preflight request with enhanced debugging
        println!("\n1. Testing OPTIONS preflight for {} request", method);
        let preflight_url = format!("http://{}{}", golem_addr, endpoint);
        println!("Preflight URL: {}", preflight_url);
        
        // Debug CORS configuration
        println!("\nCORS Configuration Debug:");
        println!("Expected CORS headers to be set:");
        println!(" - Access-Control-Allow-Origin: *");
        println!(" - Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS, HEAD, PATCH");
        println!(" - Access-Control-Allow-Headers: authorization, content-type, accept, *, request-origin, origin");
        println!(" - Access-Control-Max-Age: 3600");

        println!("\nSending preflight request with headers:");
        let preflight_headers = [
            ("Origin", "http://localhost:3000"),
            ("Access-Control-Request-Method", method),
            ("Access-Control-Request-Headers", "content-type"),
            ("Host", &format!("{}", golem_addr)),
            ("Accept", "*/*"),
            ("Connection", "keep-alive"),
            ("User-Agent", "Mozilla/5.0 (Swagger UI Test Client)"),
        ];

        // Print all request headers
        for (name, value) in &preflight_headers {
            println!(" - {}: {}", name, value);
        }

        let mut request = client.request(reqwest::Method::OPTIONS, &preflight_url);
        // Add headers individually
        for (name, value) in &preflight_headers {
            request = request.header(*name, *value);
        }

        let preflight_response = request.send().await?;

        println!("\nPreflight Response Details:");
        let preflight_status = preflight_response.status();
        println!("Status Code: {} ({})", preflight_status.as_u16(), preflight_status);
        println!("Status Success: {}", preflight_status.is_success());
        println!("Status Class: {}", preflight_status.as_u16() / 100);
        
        println!("\nPreflight Response Headers (Raw):");
        for (key, value) in preflight_response.headers().iter() {
            println!(" - {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }

        // Analyze CORS headers in detail
        println!("\nCORS Headers Analysis:");
        let cors_headers = [
            "access-control-allow-origin",
            "access-control-allow-methods",
            "access-control-allow-headers",
            "access-control-max-age",
            "access-control-expose-headers",
            "vary",
        ];

        for &header in &cors_headers {
            match preflight_response.headers().get(header) {
                Some(value) => {
                    println!("✓ Found {}", header);
                    println!("  Value: {}", value.to_str().unwrap_or("<invalid>"));
                },
                None => println!("✗ Missing required header: {}", header)
            }
        }

        // Then test actual request
        println!("\n2. Testing actual {} request", method);
        println!("{} URL: {}", method, preflight_url);
        println!("\n{} Request Headers:", method);
        let request_headers = [
            ("Origin", "http://localhost:3000"),
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("Host", &format!("{}", golem_addr)),
            ("Connection", "keep-alive"),
            ("User-Agent", "Mozilla/5.0 (Swagger UI Test Client)"),
            ("Referer", &format!("http://{}/swagger-ui/health", golem_addr)),
        ];

        // Print all request headers
        for (name, value) in &request_headers {
            println!(" - {}: {}", name, value);
        }

        if let Some(p) = payload {
            println!("\n{} Request Body:", method);
            println!("{}", p);
        }

        let mut request = match *method {
            "GET" => client.get(&preflight_url),
            "POST" => client.post(&preflight_url),
            _ => panic!("Unsupported method: {}", method),
        };

        // Add headers individually
        for (name, value) in &request_headers {
            request = request.header(*name, *value);
        }

        // Add payload for POST requests
        if let Some(p) = payload {
            request = request.body(p.to_string());
        }

        let response = request.send().await?;

        println!("\n{} Response Details:", method);
        let status = response.status();
        println!("Status Code: {} ({})", status.as_u16(), status);
        println!("Status Success: {}", status.is_success());
        println!("Status Class: {}", status.as_u16() / 100);

        // Clone headers before consuming response
        let headers = response.headers().clone();
        println!("\n{} Response Headers (Raw):", method);
        for (key, value) in headers.iter() {
            println!(" - {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }

        let response_body = response.text().await?;
        println!("\n{} Response Body:", method);
        println!("{}", response_body);

        // Additional error context
        if !status.is_success() {
            println!("\nError Analysis:");
            println!("1. Status Code Category: {}", match status.as_u16() / 100 {
                4 => "Client Error (4xx) - The request contains bad syntax or cannot be fulfilled",
                5 => "Server Error (5xx) - The server failed to fulfill a valid request",
                _ => "Unexpected Status Category",
            });
            println!("2. Specific Status: {} - {}", status.as_u16(), status);
            println!("3. Response Type: {}", headers.get("content-type").map_or("Not specified", |v| v.to_str().unwrap_or("<invalid>")));
            println!("4. Error Body: {}", response_body);
            println!("5. CORS Headers Present: {}", headers.get("access-control-allow-origin").is_some());
        }

        // Verify response with detailed error message
        assert!(
            status.is_success(),
            "{} request failed for {}:\nStatus: {}\nBody: {}\nRequest Headers: {:#?}\nResponse Headers: {:#?}",
            method,
            endpoint,
            status,
            response_body,
            request_headers,
            headers
        );
    }

    // Test Swagger UI endpoints
    println!("\nTesting Swagger UI endpoints...");
    let swagger_endpoints = [
        "/api/v1/swagger-ui",
        "/api/wit-types/swagger-ui"
    ];

    for endpoint in swagger_endpoints {
        println!("\nTesting Swagger UI endpoint: {}", endpoint);
        let swagger_url = format!("http://{}{}", golem_addr, endpoint);
        
        // Test GET request to Swagger UI
        let swagger_response = client
            .get(&swagger_url)
            .header("Origin", "http://localhost:3000")
            .send()
            .await?;

        println!("GET Response:");
        let status = swagger_response.status();
        println!("Status: {} ({})", status.as_u16(), status);
        println!("Headers:");
        for (key, value) in swagger_response.headers().iter() {
            println!(" - {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }

        let body = swagger_response.text().await?;
        println!("Response contains swagger-ui: {}", body.contains("swagger-ui"));
        
        assert!(
            status.is_success(),
            "Failed to access Swagger UI at {}: {}",
            endpoint,
            status
        );
    }

    // Test OpenAPI spec endpoints
    println!("\n=== Testing OpenAPI Spec Endpoints ===");
    let spec_endpoints = [
        "/api/v1/doc/openapi.json",
        "/api/openapi",
        "/api/wit-types/doc"
    ];

    for endpoint in spec_endpoints {
        println!("\nTesting OpenAPI spec endpoint: {}", endpoint);
        let spec_url = format!("http://{}{}", golem_addr, endpoint);
        
        let spec_response = client
            .get(&spec_url)
            .header("Origin", "http://localhost:3000")
            .send()
            .await?;

        println!("GET Response:");
        let status = spec_response.status();
        println!("Status: {} ({})", status.as_u16(), status);
        println!("Headers:");
        for (key, value) in spec_response.headers().iter() {
            println!(" - {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }

        let body = spec_response.text().await?;
        println!("Response body length: {} bytes", body.len());
        if body.len() < 1000 {
            println!("Full response body: {}", body);
        } else {
            println!("Response body preview: {}", &body[..1000]);
        }

        assert!(
            status.is_success(),
            "Failed to get OpenAPI spec from {}: {}",
            endpoint,
            status
        );
    }

    for (path, name) in &cors_test_endpoints {
        println!("\n=== Testing CORS for {} at {} ===", name, path);
        println!("1. Testing OPTIONS preflight request");
        
        // Test preflight request (OPTIONS)
        let preflight_url = format!("http://{}{}", golem_addr, path);
        println!("\n=== Testing OPTIONS preflight request to {} ===", preflight_url);
        
        let preflight_response = client
            .request(reqwest::Method::OPTIONS, &preflight_url)
            .header("Origin", "http://localhost:3000")
            .header("Access-Control-Request-Method", "GET")
            .header("Access-Control-Request-Headers", "content-type")
            .send()
            .await?;
        
        println!("\nPreflight Response Status: {}", preflight_response.status());
        println!("\nPreflight Response Headers:");
        for (key, value) in preflight_response.headers() {
            println!("  {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }
        
        // Check CORS headers and provide diagnostics
        let required_headers = [
            "access-control-allow-origin",
            "access-control-allow-methods",
            "access-control-allow-headers",
            "access-control-max-age",
            "access-control-expose-headers",
            "vary"
        ];
        
        println!("\nCORS Headers Analysis for {}:", path);
        let mut missing_headers = Vec::new();
        
        for &header in &required_headers {
            match preflight_response.headers().get(header) {
                Some(value) => println!("✓ {} = {}", header, value.to_str().unwrap_or("<invalid>")),
                None => {
                    missing_headers.push(header);
                    println!("✗ {} is missing", header);
                }
            }
        }
        
        if !missing_headers.is_empty() {
            println!("\nDiagnostics for missing headers:");
            println!("Route: {}", path);
            
            // Analyze which file might be responsible
            if path.starts_with("/api/v1/healthcheck") {
                println!("This route is defined in src/api/healthcheck.rs");
                println!("Check if create_cors_middleware() is properly applied in the healthcheck_routes() function");
            } else if path.starts_with("/api/openapi") || path.starts_with("/api/v1/swagger-ui") {
                println!("This route is defined in src/api/routes.rs");
                println!("Check if create_cors_middleware() is properly applied to the OpenAPI spec endpoints");
            } else if path.starts_with("/api/wit-types") {
                println!("This route is defined in src/api/wit_types_api.rs");
                println!("Check if create_cors_middleware() is properly applied to the WIT Types API endpoints");
            } else if path.starts_with("/api") {
                println!("This route is defined in src/api/rib_endpoints.rs");
                println!("Check if create_cors_middleware() is properly applied to the RIB API endpoints");
            }
            
            println!("\nPossible fixes:");
            println!("1. Ensure create_cors_middleware() is applied at the route level");
            println!("2. Check if the CORS middleware is being overridden by another middleware");
            println!("3. Verify the order of middleware application in src/api/routes.rs");
        }
        
        // Also test actual request
        println!("\n=== Testing actual GET request ===");
        let actual_response = client
            .get(&preflight_url)
            .header("Origin", "http://localhost:3000")
            .send()
            .await?;
        
        println!("\nActual Response Status: {}", actual_response.status());
        println!("\nActual Response Headers:");
        for (key, value) in actual_response.headers() {
            println!("  {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }
        
        println!("\nCORS Headers Analysis for Actual Response:");
        let mut missing_headers_actual = Vec::new();
        
        for &header in &required_headers {
            match actual_response.headers().get(header) {
                Some(value) => println!("✓ {} = {}", header, value.to_str().unwrap_or("<invalid>")),
                None => {
                    missing_headers_actual.push(header);
                    println!("✗ {} is missing", header);
                }
            }
        }
        
        if !missing_headers_actual.is_empty() {
            println!("\nDiagnostics for missing headers in actual response:");
            println!("Route: {}", path);
            println!("Missing headers might indicate CORS middleware is not being applied to the actual route handler");
            println!("Check the route definition and middleware order in the corresponding API file");
        }

        // Get preflight response headers for assertions
        let preflight_headers = preflight_response.headers();

        // Verify CORS headers in preflight response
        assert!(preflight_headers.contains_key("access-control-allow-origin"), 
            "Missing CORS allow-origin header in preflight response for {} endpoint", path);
        assert!(preflight_headers.contains_key("access-control-allow-methods"), 
            "Missing CORS allow-methods header in preflight response for {} endpoint", path);
        assert!(preflight_headers.contains_key("access-control-allow-headers"), 
            "Missing CORS allow-headers header in preflight response for {} endpoint", path);
        assert!(preflight_headers.contains_key("access-control-max-age"), 
            "Missing CORS max-age header in preflight response for {} endpoint", path);
        println!("✓ Preflight request passed CORS checks");

        println!("\n2. Testing actual request with CORS headers");
        // Test actual request with CORS headers
        let actual_response = client
            .get(&format!("http://{}{}", golem_addr, path))
            .header("Origin", "http://localhost:3000")
            .send()
            .await?;
        
        println!("\nActual Response Details:");
        println!("Status: {} ({})", actual_response.status().as_u16(), actual_response.status());
        
        // Get headers from actual response
        let actual_headers = actual_response.headers().clone();
        println!("Headers:");
        for (key, value) in actual_headers.iter() {
            println!(" - {}: {}", key, value.to_str().unwrap_or("<invalid>"));
        }
        
        if let Ok(body) = actual_response.text().await {
            if body.len() > 1000 {
                println!("\nResponse Body: (truncated) {}", &body[..1000]);
            } else {
                println!("\nResponse Body: {}", body);
            }
        }
        
        // Verify CORS headers in actual response
        assert!(actual_headers.contains_key("access-control-allow-origin"), 
            "Missing CORS allow-origin header in actual response for {} endpoint", path);
        
        if let Some(origin) = actual_headers.get("access-control-allow-origin") {
            assert_eq!(
                origin.to_str().unwrap_or(""),
                "http://localhost:3000",
                "Incorrect CORS allow-origin value for {} endpoint", path
            );
        }
        println!("✓ Actual request passed CORS checks");
        
        println!("\n=== Completed CORS tests for {} ===\n", name);
    }

    println!("✓ All CORS and middleware tests completed");

    // Clean up both servers
    println!("\nServer will remain running for 5 minutes to allow Swagger UI interaction...");
    println!("You can access the Swagger UI endpoints at:");
    println!(" - http://{}/api/v1/swagger-ui", golem_addr);
    println!(" - http://{}/api/wit-types/swagger-ui", golem_addr);
    
    // Wait for 5 minutes
    tokio::time::sleep(tokio::time::Duration::from_secs(5 * 60)).await;
    
    println!("\nShutting down servers...");
    server_handle.abort();
    golem_handle.abort();
    println!("Test completed successfully");
    Ok(())
} 