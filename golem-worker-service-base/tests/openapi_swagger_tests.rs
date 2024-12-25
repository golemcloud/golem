// Standard library imports
use std::collections::HashMap;
use std::collections::BTreeMap as IndexMap;
use std::sync::Arc;

// External crate imports
use serde::{Deserialize, Serialize};
use serde_json::{self};
use utoipa::openapi::{
    self,
    path::{Operation, PathItem},
    security::{ApiKey, SecurityScheme, ApiKeyValue, OAuth2, Scopes},
    Components,
    Server,
    Tag,
    Schema,
    schema::{Object, ObjectBuilder, ArrayBuilder},
    RefOr,
    request_body::RequestBody,
    Response,
    Content,
    Responses,
    Info,
};

// Internal crate imports
use golem_worker_service_base::gateway_api_definition::http::{
    openapi_export::{OpenApiExporter, OpenApiFormat},
    swagger_ui::{generate_swagger_ui, SwaggerUiConfig},
    openapi_converter::OpenApiConverter,
};

use golem_wasm_ast::analysis::{
    TypeStr, 
    TypeVariant, 
    NameTypePair, 
    TypeBool, 
    TypeList, 
    TypeRecord,
};

// Complex input/output types for API testing#[derive(Debug, Clone, Serialize, Deserialize)]
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
    let mut openapi = openapi::OpenApi::new(Default::default(), ());
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
    let mut base = openapi::OpenApi::new(Default::default(), ());
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
    let mut other = openapi::OpenApi::new(Default::default(), ());
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
    let base = openapi::OpenApi::new(Default::default(), ());
    let other = openapi::OpenApi::new(Default::default(), ());
    let merged = OpenApiConverter::merge_openapi(base, other);
    assert!(merged.paths.paths.is_empty());
    assert!(merged.servers.is_none());
    assert!(merged.tags.is_none());
    assert!(merged.components.is_none());
}

//noinspection RsUnresolvedPath
#[test]
fn test_openapi_security_schemes() {
    let mut base = openapi::OpenApi::new(Default::default(), ());
    let mut base_components = Components::new();
    base_components.security_schemes.insert(
        "api_key".to_string(),
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-API-Key")))
    );
    base.components = Some(base_components);

    let mut other = openapi::OpenApi::new(Default::default(), ());
    let mut other_components = Components::new();
    let scopes = Scopes::from_iter([
        ("read:items", "Read access"),
        ("write:items", "Write access"),
    ]);
    
    let mut flows = openapi::security::Flows::default();
    flows.implicit = Some(openapi::security::ImplicitFlow::new(
        "https://auth.example.com/authorize",
        scopes
    ));
    
    other_components.security_schemes.insert(
        "oauth2".to_string(),
        SecurityScheme::OAuth2(OAuth2::new(flows))
    );
    other.components = Some(other_components);

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
    let mut openapi = openapi::OpenApi::new(Default::default(), ());
    
    // Define complex request schema
    let request_schema = ObjectBuilder::new()
        .property("user_id", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
        .property("metadata", ObjectBuilder::new()
            .property("timestamp", Schema::Object(ObjectBuilder::new().schema_type("integer").build()))
            .property("version", Schema::Object(ObjectBuilder::new().schema_type("string").build()))
            .property("tags", Schema::Array(ArrayBuilder::new()
                .items(Schema::Object(ObjectBuilder::new().schema_type("string").build()))
                .build()))
            .build())
        .property("payload", Schema::Array(ArrayBuilder::new()
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
        .property("results", Schema::Array(ArrayBuilder::new()
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
    components.security_schemes.insert(
        "ApiKeyAuth".to_string(),
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-API-Key")))
    );
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
    let mut responses = Responses::new();
    let mut response = Response::new(());
    let mut content = IndexMap::new();
    content.insert(
        "application/json".to_string(),
        Content::new(Some(Schema::Object(response_schema)))
    );
    response.content = content;
    responses.responses.insert("200".to_string(), RefOr::T(response));

    let mut operation = Operation::new();
    operation.operation_id = Some("complexApiEndpoint".to_string());
    operation.tags = Some(vec!["Complex API".to_string()]);
    
    let mut request_body = RequestBody::new();
    let mut content = std::collections::BTreeMap::new();
    content.insert(
        "application/json".to_string(),
        Content::new(Some(Schema::Object(request_schema)))
    );
    request_body.content = content;
    operation.request_body = Some(request_body);
    
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

#[test]
fn test_swagger_ui_configuration_and_generation() {
    // Test various Swagger UI configurations
    let configs = vec![
        // Default configuration
        SwaggerUiConfig {
            enabled: true,
            path: "/docs".to_string(),
            title: None,
            theme: None,
            api_id: "test-api".to_string(),
            version: "1.0.0".to_string(),
        },
        // Custom configuration with light theme
        SwaggerUiConfig {
            enabled: true,
            path: "/api-docs".to_string(),
            title: Some("Test API Documentation".to_string()),
            theme: Some("light".to_string()),
            api_id: "test-api".to_string(),
            version: "1.0.0".to_string(),
        },
        // Custom configuration with dark theme
        SwaggerUiConfig {
            enabled: true,
            path: "/swagger".to_string(),
            title: Some("API Explorer".to_string()),
            theme: Some("dark".to_string()),
            api_id: "test-api".to_string(),
            version: "1.0.0".to_string(),
        },
    ];

    for config in configs {
        let html = generate_swagger_ui(&config);
        
        // Basic HTML structure checks
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<html"));
        assert!(html.contains("</html>"));
        
        // Check title
        if let Some(title) = &config.title {
            assert!(html.contains(&format!("<title>{}</title>", title)));
        }
        
        // Check theme-specific elements
        match config.theme.as_deref() {
            Some("dark") => {
                assert!(html.contains("background-color: #1a1a1a"));
                assert!(html.contains("filter: invert(88%) hue-rotate(180deg)"));
            }
            Some("light") => {
                assert!(!html.contains("background-color: #1a1a1a"));
                assert!(!html.contains("filter: invert(88%) hue-rotate(180deg)"));
            }
            _ => {
                // Default theme (light)
                assert!(!html.contains("background-color: #1a1a1a"));
                assert!(!html.contains("filter: invert(88%) hue-rotate(180deg)"));
            }
        }
        
        // Check OpenAPI spec URL
        let expected_url = format!("/v1/api/definitions/{}/version/{}/export", config.api_id, config.version);
        assert!(html.contains(&expected_url));
        
        // Check custom path
        assert!(html.contains(&config.path));
    }

    // Test disabled configuration
    let disabled_config = SwaggerUiConfig {
        enabled: false,
        path: "/docs".to_string(),
        title: Some("Should Not Appear".to_string()),
        theme: None,
        api_id: "test-api".to_string(),
        version: "1.0.0".to_string(),
    };
    let html = generate_swagger_ui(&disabled_config);
    assert!(html.is_empty(), "Disabled Swagger UI should return empty string");
}

#[test]
fn test_rib_type_conversion() {
    use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
    use golem_wasm_ast::analysis::{TypeStr, TypeVariant, NameTypePair, TypeBool, TypeList, TypeRecord};
    use utoipa::openapi::schema::Object;

    let converter = RibConverter;

    // Test string type
    let str_type = AnalysedType::Str(TypeStr);
    let schema = converter.convert_type(&str_type).unwrap();
    match &schema {
        Schema::Object(obj) => {
            // Verify object properties
            let obj_props: &Object = obj;
            assert_eq!(obj_props.schema_type, openapi::schema::SchemaType::Type(openapi::schema::Type::String));
            assert!(obj_props.format.is_none());
            assert!(obj_props.description.is_none());
        }
        _ => panic!("Expected object schema"),
    }

    // Test variant type with multiple cases
    let variant = AnalysedType::Variant(TypeVariant {
        cases: vec![
            NameOptionTypePair {
                name: "case1".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)),
            },
            NameOptionTypePair {
                name: "case2".to_string(),
                typ: Some(AnalysedType::Bool(TypeBool)),
            },
        ],
    });
    let schema = converter.convert_type(&variant).unwrap();
    match &schema {
        Schema::Object(obj) => {
            // Check discriminator and value properties
            assert!(obj.properties.contains_key("discriminator"));
            assert!(obj.properties.contains_key("value"));

            // Verify the value property is a OneOf schema
            if let Some(RefOr::T(Schema::OneOf(one_of))) = obj.properties.get("value") {
                assert_eq!(one_of.items.len(), 2);
            } else {
                panic!("Expected OneOf schema for value property");
            }
        }
        _ => panic!("Expected object schema"),
    }

    // Test list type
    let list_type = AnalysedType::List(TypeList {
        inner: Box::new(AnalysedType::Str(TypeStr)),
    });
    let schema = converter.convert_type(&list_type).unwrap();
    match &schema {
        Schema::Array(array) => {
            match &array.items {
                Some(RefOr::T(Schema::Object(obj))) => {
                    assert_eq!(obj.schema_type, openapi::schema::SchemaType::Type(openapi::schema::Type::String));
                }
                _ => panic!("Expected string type for array items"),
            }
        }
        _ => panic!("Expected array schema"),
    }

    // Test record type
    let record = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "field1".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "field2".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
        ],
    });
    let schema = converter.convert_type(&record).unwrap();
    match &schema {
        Schema::Object(obj) => {
            assert!(obj.properties.contains_key("field1"));
            assert!(obj.properties.contains_key("field2")); 
            assert_eq!(obj.required.len(), 2);
        }
        _ => panic!("Expected object schema"),
    }
}

#[test]
fn test_openapi_ordered_properties() {
    let mut openapi = openapi::OpenApi::new(Default::default(), ());
    let mut components = Components::new();

    // Create an ordered map of properties
    let mut properties = IndexMap::new();
    properties.insert("first".to_string(), RefOr::T(Schema::Object(ObjectBuilder::new()
        .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::String))
        .build())));
    properties.insert("second".to_string(), RefOr::T(Schema::Object(ObjectBuilder::new()
        .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::Integer))
        .build())));
    properties.insert("third".to_string(), RefOr::T(Schema::Object(ObjectBuilder::new()
        .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::Boolean))
        .build())));

    // Create a schema with ordered properties
    let mut obj_builder = ObjectBuilder::new();
    for (key, value) in properties {
        obj_builder = obj_builder.property(key, value);
    }
    let schema = Schema::Object(obj_builder
        .required(vec!["first".to_string(), "second".to_string(), "third".to_string()])
        .build());

    components.schemas.insert("OrderedObject".to_string(), RefOr::T(schema));
    openapi.components = Some(components);

    // Export to verify order preservation
    let exporter = OpenApiExporter;
    let json = exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi,
        &OpenApiFormat { json: true },
    );

    // Verify property order is maintained
    let property_positions = vec!["first", "second", "third"];
    let mut last_pos = 0;
    for prop in property_positions {
        let pos = json.find(prop).expect("Property not found in JSON");
        assert!(pos > last_pos, "Properties not in expected order");
        last_pos = pos;
    }
}

#[test]
fn test_shared_openapi_components() {
    // Create a shared schema that will be referenced multiple times
    let shared_schema = Arc::new(Schema::Object(ObjectBuilder::new()
        .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::Object))
        .property("name", Schema::Object(ObjectBuilder::new()
            .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::String))
            .build()))
        .property("age", Schema::Object(ObjectBuilder::new()
            .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::Integer))
            .build()))
        .build()));

    // Create multiple OpenAPI specs that share the same component
    let mut specs = Vec::new();
    for i in 1..=3 {
        let mut openapi = openapi::OpenApi::new(Default::default(), ());
        let mut components = Components::new();
        
        // Use the shared schema in different contexts
        let schema_ref = Arc::clone(&shared_schema);
        components.schemas.insert(
            format!("SharedObject{}", i),
            RefOr::T(Schema::Object(ObjectBuilder::new()
                .property("shared", RefOr::T((*schema_ref).clone()))
                .property("unique", Schema::Object(ObjectBuilder::new()
                    .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::String))
                    .build()))
                .build()))
        );
        
        openapi.components = Some(components);
        specs.push(openapi);
    }

    // Verify each spec has the shared component
    let exporter = OpenApiExporter;
    for (i, spec) in specs.iter().enumerate() {
        let json = exporter.export_openapi(
            &format!("test-api-{}", i + 1),
            "1.0.0",
            spec.clone(),
            &OpenApiFormat { json: true },
        );

        // Check that the shared schema properties exist in each spec
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"age\""));
        assert!(json.contains(&format!("SharedObject{}", i + 1)));
    }

    // Verify Arc is working as expected
    assert_eq!(Arc::strong_count(&shared_schema), 1);
}

//noinspection RsUnresolvedPath
#[test]
fn test_comprehensive_openapi_spec() {
    // Create a base OpenAPI spec
    let mut openapi = openapi::OpenApi::new(Default::default(), ());
    
    // Set basic info
    let mut info = Info::new("Comprehensive API", "1.0.0");
    info.description = Some("A test API using all OpenAPI components".to_string());
    openapi.info = info;

    // Add servers
    openapi.servers = Some(vec![
        Server::new("/api/v1"),
        {
            let mut server = Server::new("/api/v2");
            server.description = Some("Version 2".to_string());
            server
        },
    ]);

    // Add tags for operation grouping
    openapi.tags = Some(vec![
        Tag {
            name: "users".to_string(),
            description: Some("User operations".to_string()),
            external_docs: None,
            extensions: None,
        },
        Tag {
            name: "auth".to_string(),
            description: Some("Authentication operations".to_string()),
            external_docs: None,
            extensions: None,
        },
    ]);

    // Create components
    let mut components = Components::new();

    // Add security schemes
    components.security_schemes.insert(
        "api_key".to_string(),
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-API-Key")))
    );

    let mut flows = openapi::security::Flows::default();
    flows.implicit = Some(openapi::security::ImplicitFlow::new(
        "https://auth.example.com/authorize",
        Scopes::from_iter([
            ("read:users", "Read user data"),
            ("write:users", "Modify user data"),
        ])
    ));

    components.security_schemes.insert(
        "oauth2".to_string(),
        SecurityScheme::OAuth2(OAuth2::new(flows))
    );

    // Create reusable schemas
    let user_schema = ObjectBuilder::new()
        .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::Object))
        .property("id", Schema::Object(ObjectBuilder::new()
            .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::String))
            .build()))
        .property("roles", Schema::Array(ArrayBuilder::new()
            .items(Schema::Object(ObjectBuilder::new()
                .schema_type(openapi::schema::SchemaType::Type(openapi::schema::Type::String))
                .build()))
            .build()))
        .build();
    components.schemas.insert("User".to_string(), RefOr::T(Schema::Object(user_schema)));

    openapi.components = Some(components);

    // Add paths with operations
    let mut get_users = Operation::new();
    get_users.tags = Some(vec!["users".to_string()]);
    get_users.responses = {
        let mut responses = Responses::new();
        let mut content = IndexMap::new();
        content.insert(
            "application/json".to_string(),
            Content::new(Some(Schema::Array(ArrayBuilder::new()
                .items(Schema::Object(Object::new()))
                .build())))
        );
        let mut response = Response::new(());
        response.content = content;
        responses.responses.insert("200".to_string(), RefOr::T(response));
        responses
    };

    let mut create_user = Operation::new();
    create_user.tags = Some(vec!["users".to_string()]);
    let mut request_content = std::collections::BTreeMap::new();
    request_content.insert(
        "application/json".to_string(),
        Content::new(Some(Schema::Object(Object::new())))
    );
    let mut request_body = RequestBody::new();
    request_body.content = request_content;
    create_user.request_body = Some(request_body);

    let mut path_item = PathItem::new(Default::default(), ());
    path_item.get = Some(get_users);
    path_item.post = Some(create_user);

    openapi.paths.paths.insert("/users".to_string(), path_item);

    // Export and verify
    let exporter = OpenApiExporter;
    let json = exporter.export_openapi(
        "comprehensive-api",
        "1.0.0",
        openapi,
        &OpenApiFormat { json: true },
    );

    // Verify all components are present
    assert!(json.contains("Comprehensive API"));
    assert!(json.contains("/api/v1"));
    assert!(json.contains("/api/v2"));
    assert!(json.contains("users"));
    assert!(json.contains("auth"));
    assert!(json.contains("X-API-Key"));
    assert!(json.contains("oauth2"));
    assert!(json.contains("read:users"));
    assert!(json.contains("write:users"));
    assert!(json.contains("/users"));
    assert!(json.contains("application/json"));
}
