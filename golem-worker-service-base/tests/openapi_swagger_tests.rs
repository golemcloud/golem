use golem_worker_service_base::gateway_api_definition::http::{
    openapi_export::{OpenApiExporter, OpenApiFormat},
    openapi_converter::OpenApiConverter,
    swagger_ui::{SwaggerUiConfig, generate_swagger_ui},
    OpenApiHttpApiDefinitionRequest,
};
use utoipa::openapi::{OpenApi, Info, PathItem, Operation, Server, Tag, Components, SecurityScheme, Schema, Response, RequestBody};
use utoipa::openapi::schema::{Object, ObjectBuilder};
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use serde::{Serialize, Deserialize};
use std::sync::Arc;

// Complex input/output types for API testing
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComplexRequest {
    user_id: String,
    metadata: RequestMetadata,
    payload: Vec<RequestPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestMetadata {
    timestamp: i64,
    version: String,
    tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestPayload {
    operation_type: String,
    parameters: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComplexResponse {
    request_id: String,
    status: ResponseStatus,
    results: Vec<OperationResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResponseStatus {
    code: i32,
    message: String,
    details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationResult {
    success: bool,
    data: Option<serde_json::Value>,
    error: Option<String>,
}

#[test]
fn test_openapi_export_formats() {
    let exporter = OpenApiExporter;
    let mut openapi = OpenApi::new();
    openapi.info = Info::new("Test API", "1.0.0");

    // Test JSON export
    let json_format = OpenApiFormat { json: true };
    let exported_json = exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi.clone(),
        &json_format,
    );
    assert!(!exported_json.is_empty());
    assert!(exported_json.contains("test-api API"));
    assert!(exported_json.contains("1.0.0"));
    assert!(exported_json.starts_with("{"));  // JSON format check

    // Test YAML export
    let yaml_format = OpenApiFormat { json: false };
    let exported_yaml = exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi.clone(),
        &yaml_format,
    );
    assert!(!exported_yaml.is_empty());
    assert!(exported_yaml.contains("test-api API"));
    assert!(exported_yaml.contains("1.0.0"));
    assert!(!exported_yaml.starts_with("{"));  // YAML format check
}

#[test]
fn test_swagger_ui_generation() {
    // Test default configuration
    let default_config = SwaggerUiConfig::default();
    assert!(!default_config.enabled);
    assert_eq!(default_config.path, "/docs");
    assert!(default_config.title.is_none());
    assert!(default_config.theme.is_none());

    // Test enabled configuration with light theme
    let light_config = SwaggerUiConfig {
        enabled: true,
        path: "/api-docs".to_string(),
        title: Some("Test API".to_string()),
        theme: None,
        api_id: "test-api".to_string(),
        version: "1.0.0".to_string(),
    };
    let light_html = generate_swagger_ui(&light_config);
    assert!(light_html.contains("<!DOCTYPE html>"));
    assert!(light_html.contains("<title>Test API</title>"));
    assert!(!light_html.contains("background-color: #1a1a1a"));
    assert!(light_html.contains("/v1/api/definitions/test-api/version/1.0.0/export"));

    // Test dark theme
    let dark_config = SwaggerUiConfig {
        enabled: true,
        theme: Some("dark".to_string()),
        ..light_config.clone()
    };
    let dark_html = generate_swagger_ui(&dark_config);
    assert!(dark_html.contains("background-color: #1a1a1a"));
    assert!(dark_html.contains("filter: invert(88%) hue-rotate(180deg)"));
}

#[test]
fn test_openapi_converter_merge() {
    // Create base OpenAPI spec
    let mut base = OpenApi::new();
    base.paths.paths.insert(
        "/test".to_string(),
        PathItem::new().get(Operation::new().description("Test GET")),
    );
    base.servers = Some(vec![Server::new("/base")]);
    base.tags = Some(vec![Tag::new("base-tag")]);

    // Create second OpenAPI spec
    let mut other = OpenApi::new();
    other.paths.paths.insert(
        "/other".to_string(),
        PathItem::new().post(Operation::new().description("Test POST")),
    );
    other.servers = Some(vec![Server::new("/other")]);
    other.tags = Some(vec![Tag::new("other-tag")]);

    // Test merging
    let merged = OpenApiConverter::merge_openapi(base, other);
    
    // Verify paths merged correctly
    assert!(merged.paths.paths.contains_key("/test"));
    assert!(merged.paths.paths.contains_key("/other"));
    
    // Verify servers merged
    let servers = merged.servers.unwrap();
    assert_eq!(servers.len(), 2);
    assert!(servers.iter().any(|s| s.url == "/base"));
    assert!(servers.iter().any(|s| s.url == "/other"));
    
    // Verify tags merged
    let tags = merged.tags.unwrap();
    assert_eq!(tags.len(), 2);
    assert!(tags.iter().any(|t| t.name == "base-tag"));
    assert!(tags.iter().any(|t| t.name == "other-tag"));
}

#[test]
fn test_openapi_export_path_generation() {
    let api_id = "test-api";
    let version = "1.0.0";
    let path = OpenApiExporter::get_export_path(api_id, version);
    assert_eq!(path, "/v1/api/definitions/test-api/version/1.0.0/export");
}

#[test]
fn test_swagger_ui_disabled() {
    let config = SwaggerUiConfig {
        enabled: false,
        path: "/docs".to_string(),
        title: Some("Test API".to_string()),
        theme: Some("dark".to_string()),
        api_id: "test-api".to_string(),
        version: "1.0.0".to_string(),
    };
    let html = generate_swagger_ui(&config);
    assert!(html.is_empty());
}

#[test]
fn test_openapi_converter_empty_merge() {
    let base = OpenApi::new();
    let other = OpenApi::new();
    let merged = OpenApiConverter::merge_openapi(base, other);
    assert!(merged.paths.paths.is_empty());
    assert!(merged.servers.is_none());
    assert!(merged.tags.is_none());
    assert!(merged.components.is_none());
}

#[test]
fn test_openapi_security_schemes() {
    let mut base = OpenApi::new();
    base.components = Some(Components::new()
        .security_scheme("api_key", SecurityScheme::ApiKey(
            utoipa::openapi::security::ApiKey::Header("X-API-Key".to_string())
        ))
    );

    let mut other = OpenApi::new();
    other.components = Some(Components::new()
        .security_scheme("oauth2", SecurityScheme::OAuth2(
            utoipa::openapi::security::OAuth2::new()
        ))
    );

    let merged = OpenApiConverter::merge_openapi(base, other);
    let components = merged.components.unwrap();
    assert!(components.security_schemes.contains_key("api_key"));
    assert!(components.security_schemes.contains_key("oauth2"));
}

#[test]
fn test_openapi_format_default() {
    let format = OpenApiFormat::default();
    assert!(format.json);
}

#[test]
fn test_openapi_converter_new() {
    let converter = OpenApiConverter::new();
    assert!(Arc::strong_count(&converter.exporter) == 1);
}

#[test]
fn test_complex_api_schema_generation() {
    let mut openapi = OpenApi::new();
    
    // Define complex request schema
    let request_schema = ObjectBuilder::new()
        .property("user_id", Schema::String(Default::default()))
        .property("metadata", ObjectBuilder::new()
            .property("timestamp", Schema::Integer(Default::default()))
            .property("version", Schema::String(Default::default()))
            .property("tags", Schema::Array(Default::default()))
            .build())
        .property("payload", Schema::Array(Default::default()))
        .build();

    // Define complex response schema
    let response_schema = ObjectBuilder::new()
        .property("request_id", Schema::String(Default::default()))
        .property("status", ObjectBuilder::new()
            .property("code", Schema::Integer(Default::default()))
            .property("message", Schema::String(Default::default()))
            .property("details", Schema::String(Default::default()))
            .build())
        .property("results", Schema::Array(Default::default()))
        .build();

    // Add components
    openapi.components = Some(Components::new()
        .schema("ComplexRequest", Schema::Object(request_schema.clone()))
        .schema("ComplexResponse", Schema::Object(response_schema.clone())));

    // Add complex endpoint
    let operation = Operation::new()
        .request_body(Some(RequestBody::new()
            .content("application/json", utoipa::openapi::Content::new(Schema::Object(request_schema)))))
        .response("200", Response::new()
            .content("application/json", utoipa::openapi::Content::new(Schema::Object(response_schema))))
        .description("Complex API endpoint");

    openapi.paths.paths.insert("/api/v1/complex-operation".to_string(),
        PathItem::new().post(operation));

    // Export and verify schema
    let exporter = OpenApiExporter;
    
    // Export JSON
    let exported_json = exporter.export_openapi(
        "complex-api",
        "1.0.0",
        openapi.clone(),
        &OpenApiFormat { json: true },
    );

    // Export YAML
    let exported_yaml = exporter.export_openapi(
        "complex-api",
        "1.0.0",
        openapi.clone(),
        &OpenApiFormat { json: false },
    );

    // Create output directory in the workspace root
    let output_dir = std::path::Path::new("openapi_exports");
    std::fs::create_dir_all(output_dir).expect("Failed to create output directory");

    // Save exports with full paths
    let json_path = output_dir.join("complex-api.json");
    let yaml_path = output_dir.join("complex-api.yaml");
    
    std::fs::write(&json_path, &exported_json).expect("Failed to write JSON export");
    std::fs::write(&yaml_path, &exported_yaml).expect("Failed to write YAML export");

    println!("\nExported OpenAPI schemas to:");
    println!("- {}", json_path.display());
    println!("- {}", yaml_path.display());

    // Print the contents for verification
    println!("\nJSON Content:");
    println!("{}", exported_json);
    println!("\nYAML Content:");
    println!("{}", exported_yaml);

    // Verify schema contains all complex types
    assert!(exported_json.contains("ComplexRequest"));
    assert!(exported_json.contains("ComplexResponse"));
    assert!(exported_json.contains("metadata"));
    assert!(exported_json.contains("payload"));
    assert!(exported_json.contains("results"));
}

#[test]
fn test_api_interaction() {
    // Create test request data
    let request = ComplexRequest {
        user_id: "test-user".to_string(),
        metadata: RequestMetadata {
            timestamp: 1234567890,
            version: "1.0".to_string(),
            tags: vec!["test".to_string(), "integration".to_string()],
        },
        payload: vec![RequestPayload {
            operation_type: "test-op".to_string(),
            parameters: {
                let mut map = std::collections::HashMap::new();
                map.insert("key".to_string(), serde_json::Value::String("value".to_string()));
                map
            },
        }],
    };

    // Serialize request to JSON
    let request_json = serde_json::to_string(&request).unwrap();
    assert!(request_json.contains("test-user"));
    assert!(request_json.contains("test-op"));

    // Create test response
    let response = ComplexResponse {
        request_id: "test-123".to_string(),
        status: ResponseStatus {
            code: 200,
            message: "Success".to_string(),
            details: None,
        },
        results: vec![OperationResult {
            success: true,
            data: Some(serde_json::json!({"result": "ok"})),
            error: None,
        }],
    };

    // Verify response serialization
    let response_json = serde_json::to_string(&response).unwrap();
    assert!(response_json.contains("test-123"));
    assert!(response_json.contains("Success"));
    assert!(response_json.contains("result"));

    // Verify deserialization
    let deserialized_response: ComplexResponse = serde_json::from_str(&response_json).unwrap();
    assert_eq!(deserialized_response.request_id, "test-123");
    assert_eq!(deserialized_response.status.code, 200);
    assert!(deserialized_response.results[0].success);
} 