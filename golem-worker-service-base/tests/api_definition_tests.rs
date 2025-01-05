use poem_openapi::{
    OpenApi,
    OpenApiService,
    Object,
    payload::Json,
    registry::Registry,
    types::Type,
};
use golem_worker_service_base::gateway_api_definition::http::swagger_ui::SwaggerUiConfig;
use golem_worker_service_base::gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat};

// Test API structures
#[derive(Debug, Object)]
struct TestSearchQuery {
    query: String,
    filters: Option<TestSearchFilters>,
}

#[derive(Debug, Object)]
struct TestSearchFilters {
    date_range: Option<TestDateRange>,
    pagination: Option<TestPagination>,
}

#[derive(Debug, Object)]
struct TestDateRange {
    start: String,
    end: String,
}

#[derive(Debug, Object)]
struct TestPagination {
    page: i32,
    per_page: i32,
}

// Test API implementation
#[derive(Clone)]
struct TestApi;

#[OpenApi]
impl TestApi {
    #[oai(path = "/healthcheck", method = "get")]
    async fn get_health_check(&self) -> Json<String> {
        Json("OK".to_string())
    }

    #[oai(path = "/version", method = "get")]
    async fn get_version(&self) -> Json<String> {
        Json("1.0.0".to_string())
    }

    #[oai(path = "/search", method = "post")]
    async fn search(&self, _payload: Json<TestSearchQuery>) -> Json<String> {
        Json("Search results".to_string())
    }
}

#[tokio::test]
async fn test_api_definition_to_openapi() -> anyhow::Result<()> {
    let api = TestApi;
    let service = OpenApiService::new(api, "Test API", "1.0.0");
    let spec = service.spec();
    
    // Validate OpenAPI spec
    assert!(spec.contains("openapi"), "OpenAPI version should be specified");
    assert!(spec.contains("Test API"), "API title should be present");
    assert!(spec.contains("/healthcheck"), "Healthcheck endpoint should be present");
    assert!(spec.contains("/version"), "Version endpoint should be present");
    assert!(spec.contains("/search"), "Search endpoint should be present");

    Ok(())
}

#[tokio::test]
async fn test_openapi_schema_generation() -> anyhow::Result<()> {
    let api = TestApi;
    let exporter = OpenApiExporter;
    let format = OpenApiFormat { json: true };
    
    let json_content = exporter.export_openapi(api, &format);
    
    // Validate schema content
    assert!(json_content.contains("TestSearchQuery"), "TestSearchQuery schema should be present");
    assert!(json_content.contains("TestSearchFilters"), "TestSearchFilters schema should be present");
    assert!(json_content.contains("TestDateRange"), "TestDateRange schema should be present");
    assert!(json_content.contains("TestPagination"), "TestPagination schema should be present");
    
    Ok(())
}

#[tokio::test]
async fn test_swagger_ui_integration() -> anyhow::Result<()> {
    let _swagger_config = SwaggerUiConfig {
        server_url: Some("/docs".to_string()),
        enabled: true,
        title: Some("Test API Documentation".to_string()),
        version: Some("1.0.0".to_string()),
    };

    let api = TestApi;
    let service = OpenApiService::new(api, "Test API", "1.0.0")
        .server("http://localhost:8080");
    
    assert!(service.spec().contains("servers"), "OpenAPI spec should include servers");
    assert!(service.spec().contains("http://localhost:8080"), "Server URL should be present");

    Ok(())
}

#[tokio::test]
async fn test_api_endpoints_and_methods() -> anyhow::Result<()> {
    let api = TestApi;
    let service = OpenApiService::new(api, "Test API", "1.0.0");
    let spec = service.spec();
    
    // Test endpoint presence and methods
    assert!(spec.contains(r#""/healthcheck""#), "Healthcheck endpoint should be present");
    assert!(spec.contains(r#""get""#), "GET method should be present");
    assert!(spec.contains(r#""/version""#), "Version endpoint should be present");
    assert!(spec.contains(r#""/search""#), "Search endpoint should be present");
    assert!(spec.contains(r#""post""#), "POST method should be present");
    
    Ok(())
}

#[tokio::test]
async fn test_schema_definitions() -> anyhow::Result<()> {
    let mut registry = Registry::new();
    
    // Register test schemas
    <TestSearchQuery as Type>::register(&mut registry);
    <TestSearchFilters as Type>::register(&mut registry);
    <TestDateRange as Type>::register(&mut registry);
    <TestPagination as Type>::register(&mut registry);
    
    let schemas = registry.schemas;
    
    // Validate schema registration
    assert!(schemas.contains_key("TestSearchQuery"), "TestSearchQuery schema should be registered");
    assert!(schemas.contains_key("TestSearchFilters"), "TestSearchFilters schema should be registered");
    assert!(schemas.contains_key("TestDateRange"), "TestDateRange schema should be registered");
    assert!(schemas.contains_key("TestPagination"), "TestPagination schema should be registered");
    
    Ok(())
} 