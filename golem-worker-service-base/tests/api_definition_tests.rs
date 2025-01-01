use std::path::PathBuf;
use serde_yaml;
use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{SwaggerUiConfig, generate_swagger_ui};
use golem_worker_service_base::gateway_api_definition::http::openapi_export::OpenApiExporter;
use utoipa::openapi::OpenApi;

#[tokio::test]
async fn test_api_definition_to_openapi() -> anyhow::Result<()> {
    // Load the API definition fixture
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test_api_definition.yaml");
    
    let api_def_yaml = std::fs::read_to_string(fixture_path)?;
    let api_def: serde_yaml::Value = serde_yaml::from_str(&api_def_yaml)?;

    // Validate the loaded API definition
    assert!(api_def.get("openapi").is_some(), "OpenAPI version should be specified");
    assert!(api_def.get("info").is_some(), "API info should be present");
    assert!(api_def.get("paths").is_some(), "API paths should be defined");
    assert!(api_def.get("components").is_some(), "Components should be defined");

    Ok(())
}

#[tokio::test]
async fn test_openapi_schema_generation() -> anyhow::Result<()> {
    // Load and parse the test API definition
    let api_yaml = include_str!("fixtures/test_api_definition.yaml");
    let openapi: OpenApi = serde_yaml::from_str(api_yaml)?;

    // Export OpenAPI schema using our exporter
    let openapi_exporter = OpenApiExporter;
    let json_content = openapi_exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi.clone(),
        &golem_worker_service_base::gateway_api_definition::http::openapi_export::OpenApiFormat { json: true }
    );

    // Parse the exported schema back to validate it
    let exported_schema: serde_json::Value = serde_json::from_str(&json_content)?;

    // Validate OpenAPI version
    assert_eq!(
        exported_schema.get("openapi").and_then(|v| v.as_str()),
        Some("3.1.0"),
        "OpenAPI version should be 3.1.0"
    );

    // Validate paths and their operations
    let paths = exported_schema.get("paths").expect("Paths should be present");

    // Core endpoints
    validate_endpoint(paths, "/healthcheck", "get", "getHealthCheck")?;
    validate_endpoint(paths, "/version", "get", "getVersion")?;
    validate_endpoint(paths, "/v1/api/definitions/{api_id}/version/{version}/export", "get", "exportApiDefinition")?;

    // RIB endpoints
    validate_endpoint(paths, "/api/v1/rib/healthcheck", "get", "getRibHealthCheck")?;
    validate_endpoint(paths, "/api/v1/rib/version", "get", "getRibVersion")?;

    // Primitive types endpoints
    validate_endpoint(paths, "/primitives", "get", "getPrimitiveTypes")?;
    validate_endpoint(paths, "/primitives", "post", "createPrimitiveTypes")?;

    // User management endpoints
    validate_endpoint(paths, "/users/{id}/profile", "get", "getUserProfile")?;
    validate_endpoint(paths, "/users/{id}/settings", "post", "updateUserSettings")?;
    validate_endpoint(paths, "/users/{id}/permissions", "get", "getUserPermissions")?;

    // Content endpoints
    validate_endpoint(paths, "/content", "post", "createContent")?;
    validate_endpoint(paths, "/content/{id}", "get", "getContent")?;

    // Search endpoints
    validate_endpoint(paths, "/search", "post", "performSearch")?;
    validate_endpoint(paths, "/search/validate", "post", "validateSearch")?;

    // Batch endpoints
    validate_endpoint(paths, "/batch/process", "post", "processBatch")?;
    validate_endpoint(paths, "/batch/validate", "post", "validateBatch")?;
    validate_endpoint(paths, "/batch/{id}/status", "get", "getBatchStatus")?;

    // Transform endpoints
    validate_endpoint(paths, "/transform", "post", "applyTransformation")?;
    validate_endpoint(paths, "/transform/chain", "post", "chainTransformations")?;

    // Tree endpoints
    validate_endpoint(paths, "/tree", "post", "createTree")?;
    validate_endpoint(paths, "/tree/{id}", "get", "queryTree")?;
    validate_endpoint(paths, "/tree/modify", "post", "modifyTree")?;

    Ok(())
}

fn validate_endpoint(paths: &serde_json::Value, path: &str, method: &str, operation_id: &str) -> anyhow::Result<()> {
    let endpoint = paths.get(path).expect(&format!("Endpoint {} should exist", path));
    let operation = endpoint.get(method).expect(&format!("Method {} should exist for {}", method, path));
    assert_eq!(
        operation.get("operationId").and_then(|v| v.as_str()),
        Some(operation_id),
        "Operation ID should be correct for {} {}", method, path
    );
    assert!(
        operation.get("responses").and_then(|r| r.get("200")).is_some(),
        "Endpoint {} should have 200 response", path
    );
    Ok(())
}

#[tokio::test]
async fn test_api_definition_completeness() -> anyhow::Result<()> {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test_api_definition.yaml");
    
    let api_def_yaml = std::fs::read_to_string(fixture_path)?;
    let api_def: serde_yaml::Value = serde_yaml::from_str(&api_def_yaml)?;

    let paths = api_def.get("paths").expect("API paths should be defined");
    
    // Verify all expected endpoints are present
    let expected_endpoints = vec![
        "/healthcheck",
        "/version",
        "/v1/api/definitions/{api_id}/version/{version}/export",
        "/api/v1/rib/healthcheck",
        "/api/v1/rib/version",
        "/primitives",
        "/users/{id}/profile",
        "/users/{id}/settings",
        "/users/{id}/permissions",
        "/content",
        "/content/{id}",
        "/search",
        "/search/validate",
        "/batch/process",
        "/batch/validate",
        "/batch/{id}/status",
        "/transform",
        "/transform/chain",
        "/tree",
        "/tree/{id}",
        "/tree/modify",
    ];

    for endpoint in expected_endpoints {
        assert!(
            paths.get(endpoint).is_some(),
            "Endpoint {} should be defined",
            endpoint
        );
    }

    // Verify components/schemas are present
    let components = api_def.get("components").expect("Components should be defined");
    let schemas = components.get("schemas").expect("Schemas should be defined");

    let expected_schemas = vec![
        "SearchQuery",
        "SearchFilters",
        "SearchFlags",
        "DateRange",
        "Pagination",
        "DataTransformation",
        "TreeNode",
        "NodeMetadata",
        "TreeOperation",
    ];

    for schema in expected_schemas {
        assert!(
            schemas.get(schema).is_some(),
            "Schema {} should be defined",
            schema
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_swagger_ui_integration() -> anyhow::Result<()> {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test_api_definition.yaml");
    
    let api_def_yaml = std::fs::read_to_string(fixture_path)?;
    let api_def: serde_yaml::Value = serde_yaml::from_str(&api_def_yaml)?;

    let info = api_def.get("info").expect("API info should be present");
    let swagger_config = SwaggerUiConfig {
        enabled: true,
        path: "/docs".to_string(),
        title: Some(info.get("title").and_then(|t| t.as_str()).unwrap_or("API Documentation").to_string()),
        theme: None,
        api_id: "test-component".to_string(),
        version: info.get("version").and_then(|v| v.as_str()).unwrap_or("1.0").to_string(),
    };

    let html = generate_swagger_ui(&swagger_config);

    let expected_spec_url = OpenApiExporter::get_export_path(&swagger_config.api_id, &swagger_config.version);

    assert!(html.contains("swagger-ui"), "Should include Swagger UI elements");
    assert!(html.contains(&expected_spec_url), "Should include OpenAPI spec URL");
    assert!(html.contains("SwaggerUIBundle"), "Should include Swagger UI bundle");
    assert!(html.contains(&swagger_config.title.unwrap_or_else(|| "API Documentation".to_string())), 
           "Should include API title");

    Ok(())
}

#[tokio::test]
async fn test_api_tags_and_servers() -> anyhow::Result<()> {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test_api_definition.yaml");
    
    let api_def_yaml = std::fs::read_to_string(fixture_path)?;
    let api_def: serde_yaml::Value = serde_yaml::from_str(&api_def_yaml)?;

    let servers = api_def.get("servers").expect("API servers should be defined");
    assert!(!servers.as_sequence().unwrap().is_empty(), "At least one server should be defined");

    let server = servers.as_sequence().unwrap().first().unwrap();
    assert_eq!(
        server.get("url").and_then(|v| v.as_str()),
        Some("http://localhost:8080"),
        "Default server URL should be correct"
    );

    Ok(())
}

#[tokio::test]
async fn test_schema_definitions() -> anyhow::Result<()> {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test_api_definition.yaml");
    
    let api_def_yaml = std::fs::read_to_string(fixture_path)?;
    let api_def: serde_yaml::Value = serde_yaml::from_str(&api_def_yaml)?;

    let components = api_def.get("components").expect("Components should be defined");
    let schemas = components.get("schemas").expect("Schemas should be defined");

    // Test SearchQuery schema
    let search_query = schemas.get("SearchQuery").expect("SearchQuery schema should exist");
    assert!(search_query.get("properties").is_some(), "SearchQuery should have properties");

    // Test DataTransformation schema
    let data_transformation = schemas.get("DataTransformation").expect("DataTransformation schema should exist");
    assert!(data_transformation.get("oneOf").is_some(), "DataTransformation should have oneOf");

    // Test TreeNode schema
    let tree_node = schemas.get("TreeNode").expect("TreeNode schema should exist");
    let tree_node_props = tree_node.get("properties").expect("TreeNode should have properties");
    assert!(tree_node_props.get("children").is_some(), "TreeNode should have children property");
    assert!(tree_node_props.get("metadata").is_some(), "TreeNode should have metadata property");

    Ok(())
}

#[tokio::test]
async fn test_wit_function_mappings() -> anyhow::Result<()> {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("test_api_definition.yaml");
    
    let api_def_yaml = std::fs::read_to_string(fixture_path)?;
    let api_def: serde_yaml::Value = serde_yaml::from_str(&api_def_yaml)?;

    let paths = api_def.get("paths").expect("API paths should be defined");

    // Define expected WIT function mappings for all endpoints
    let expected_mappings = vec![
        ("/healthcheck", "GET", "getHealthCheck"),
        ("/version", "GET", "getVersion"),
        ("/v1/api/definitions/{api_id}/version/{version}/export", "GET", "exportApiDefinition"),
        ("/api/v1/rib/healthcheck", "GET", "getRibHealthCheck"),
        ("/api/v1/rib/version", "GET", "getRibVersion"),
        ("/primitives", "GET", "getPrimitiveTypes"),
        ("/primitives", "POST", "createPrimitiveTypes"),
        ("/users/{id}/profile", "GET", "getUserProfile"),
        ("/users/{id}/settings", "POST", "updateUserSettings"),
        ("/users/{id}/permissions", "GET", "getUserPermissions"),
        ("/content", "POST", "createContent"),
        ("/content/{id}", "GET", "getContent"),
        ("/search", "POST", "performSearch"),
        ("/search/validate", "POST", "validateSearch"),
        ("/batch/process", "POST", "processBatch"),
        ("/batch/validate", "POST", "validateBatch"),
        ("/batch/{id}/status", "GET", "getBatchStatus"),
        ("/transform", "POST", "applyTransformation"),
        ("/transform/chain", "POST", "chainTransformations"),
        ("/tree", "POST", "createTree"),
        ("/tree/{id}", "GET", "queryTree"),
        ("/tree/modify", "POST", "modifyTree"),
    ];

    for (path, method, operation_id) in expected_mappings {
        let path_obj = paths.get(path).expect(&format!("Path {} should exist", path));
        let method_obj = path_obj.get(method.to_lowercase())
            .expect(&format!("Method {} should exist for path {}", method, path));
        let actual_operation_id = method_obj.get("operationId")
            .and_then(|v| v.as_str())
            .expect(&format!("operationId should exist for {}", path));
        
        assert_eq!(
            actual_operation_id, 
            operation_id,
            "Path {} should map to WIT function {}",
            path,
            operation_id
        );
    }

    Ok(())
} 