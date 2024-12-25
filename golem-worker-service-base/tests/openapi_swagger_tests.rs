// Standard library imports
use std::collections::HashMap;
use std::sync::Arc;

// External crate imports
use serde::{Deserialize, Serialize};
use serde_json::{self};
use utoipa::openapi::{
    path::{Operation, PathItem},
    schema::{ObjectBuilder, Schema, Array, Object, OneOf, SchemaType},
    security::{ApiKey, ApiKeyLocation, OAuth2, SecurityScheme, Scopes, OAuthFlows, ImplicitFlow},
    Components,
    Content,
    Info,
    OpenApi,
    RefOr,
    RequestBody,
    Response,
    Server,
    Tag,
};

use golem_worker_service_base::gateway_api_definition::http::openapi_converter::OpenApiConverter;
use golem_worker_service_base::gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat};
use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{generate_swagger_ui, SwaggerUiConfig};
use golem_wasm_ast::analysis::AnalysedType;

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
    parameters: HashMap<String, serde_json::Value>,
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
    let mut openapi = OpenApi::new(Default::default(), ());
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
    let mut base = OpenApi::new(Default::default(), ());
    let mut base_path_item = PathItem::new(Default::default(), ());
    base_path_item.get = Some(Operation::new());
    if let Some(op) = &mut base_path_item.get {
        op.operation_id = Some("testGet".to_string());
        op.tags = Some(vec!["Test Operations".to_string()]);
    }
    base.paths.paths.insert("/test".to_string(), base_path_item);
    base.servers = Some(vec![Server::new("/base")]);
    base.tags = Some(vec![Tag::new("base-tag")]);

    // Create second OpenAPI spec
    let mut other = OpenApi::new(Default::default(), ());
    let mut other_path_item = PathItem::new(Default::default(), ());
    other_path_item.post = Some(Operation::new());
    if let Some(op) = &mut other_path_item.post {
        op.operation_id = Some("testPost".to_string());
        op.tags = Some(vec!["Test Operations".to_string()]);
    }
    other.paths.paths.insert("/other".to_string(), other_path_item);
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
    let base = OpenApi::new(Default::default(), ());
    let other = OpenApi::new(Default::default(), ());
    let merged = OpenApiConverter::merge_openapi(base, other);
    assert!(merged.paths.paths.is_empty());
    assert!(merged.servers.is_none());
    assert!(merged.tags.is_none());
    assert!(merged.components.is_none());
}

#[test]
fn test_openapi_security_schemes() {
    let mut base = OpenApi::new(Default::default(), ());
    base.components = Some(Components::new()
        .add_security_scheme("api_key", SecurityScheme::ApiKey(
            ApiKey::Header("X-API-Key".to_string())
        )));

    let mut other = OpenApi::new(Default::default(), ());
    other.components = Some(Components::new()
        .add_security_scheme("oauth2", SecurityScheme::OAuth2(
            OAuth2::with_flows(
                OAuthFlows::implicit(
                    ImplicitFlow::new(
                        "https://auth.example.com/authorize",
                        Scopes::from_iter([
                            ("read:items", "Read access"),
                            ("write:items", "Write access"),
                        ])
                    )
                )
            )
        )));

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
    assert_eq!(Arc::strong_count(&converter.exporter), 1);
}

#[test]
fn test_complex_api_schema_generation() {
    let mut openapi = OpenApi::new(Default::default(), ());
    
    // Define complex request schema
    let request_schema = ObjectBuilder::new()
        .property("user_id", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
        .property("metadata", ObjectBuilder::new()
            .property("timestamp", Schema::Object(ObjectBuilder::new().schema_type("integer").build()))
            .property("version", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
            .property("tags", Schema::Array(utoipa::openapi::schema::ArrayBuilder::new()
                .items(Schema::Object(ObjectBuilder::new().schema_type("string").build()))
                .build()))
            .build())
        .property("payload", Schema::Array(utoipa::openapi::schema::ArrayBuilder::new()
            .items(Schema::Object(ObjectBuilder::new()
                .property("operation_type", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
                .property("parameters", Schema::Object(ObjectBuilder::new().schema_type("object").build()))
                .build()))
            .build()))
        .build();

    // Define complex response schema
    let response_schema = ObjectBuilder::new()
        .property("request_id", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
        .property("status", Schema::Object(ObjectBuilder::new()
            .property("code", Schema::Object(ObjectBuilder::new().schema_type("integer").build()))
            .property("message", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
            .property("details", Schema::Object(ObjectBuilder::new()
                .schema_type("string")
                .required(false)
                .build()))
            .build()))
        .property("results", Schema::Array(utoipa::openapi::schema::ArrayBuilder::new()
            .items(Schema::Object(ObjectBuilder::new()
                .property("success", Schema::Object(ObjectBuilder::new().schema_type("boolean").build()))
                .property("data", Schema::Object(ObjectBuilder::new()
                    .schema_type("object")
                    .required(false)
                    .build()))
                .property("error", Schema::Object(ObjectBuilder::new()
                    .schema_type("string")
                    .required(false)
                    .build()))
                .build()))
            .build()))
        .build();

    // Add components
    let mut components = Components::new();
    components.schemas.insert(
        "ComplexRequest".to_string(),
        RefOr::T(Schema::Object(request_schema.clone()))
    );
    components.schemas.insert(
        "ComplexResponse".to_string(),
        RefOr::T(Schema::Object(response_schema.clone()))
    );
    openapi.components = Some(components);

    // Add complex endpoint
    let mut responses = utoipa::openapi::Responses::new();
    let mut response = Response::new();
    let mut content = std::collections::BTreeMap::new();
    content.insert(
        "application/json".to_string(),
        Content::new(Schema::Object(response_schema))
    );
    response.content = content;
    responses.responses.insert("200".to_string(), RefOr::T(response));

    let mut operation = Operation::new();
    operation.operation_id = Some("complexApiEndpoint".to_string());
    operation.tags = Some(vec!["Complex API".to_string()]);
    operation.request_body = Some(RefOr::T(
        RequestBody::new().content(
            "application/json",
            Content::new(Schema::Object(request_schema)),
        ),
    ));
    operation.responses = responses;

    let mut path_item = PathItem::new(Default::default(), ());
    path_item.post = Some(operation);
    
    openapi.paths.paths.insert("/api/v1/complex-operation".to_string(), path_item);

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
                let mut map = HashMap::new();
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