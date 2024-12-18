use crate::{
    api::{
        definition::types::{ApiDefinition, Route, HttpMethod, BindingType},
        openapi::{OpenAPIConverter, validate_openapi},
    },
    swagger::{SwaggerGenerator, SwaggerUIBinding},
    service::openapi_export::export_openapi,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::json;
use tempfile::TempDir;
use tower::ServiceExt;

async fn create_test_api() -> ApiDefinition {
    ApiDefinition {
        id: "test-api".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API for integration tests".to_string(),
        routes: vec![
            Route {
                path: "/users".to_string(),
                method: HttpMethod::Get,
                description: "List users".to_string(),
                template_name: "users".to_string(),
                binding: BindingType::Default {
                    input_type: "record{page: i32, limit: i32}".to_string(),
                    output_type: "list<record{id: string, name: string}>".to_string(),
                    function_name: "list_users".to_string(),
                },
            },
            Route {
                path: "/users/{id}".to_string(),
                method: HttpMethod::Post,
                description: "Update user".to_string(),
                template_name: "users".to_string(),
                binding: BindingType::Default {
                    input_type: "record{name: string, email: string}".to_string(),
                    output_type: "record{id: string, name: string, email: string}".to_string(),
                    function_name: "update_user".to_string(),
                },
            },
            Route {
                path: "/files/{path}".to_string(),
                method: HttpMethod::Get,
                description: "Serve static files".to_string(),
                template_name: "files".to_string(),
                binding: BindingType::FileServer {
                    root_dir: "/static".to_string(),
                },
            },
        ],
    }
}

#[tokio::test]
async fn test_openapi_export_flow() {
    let api = create_test_api().await;
    
    // Test OpenAPI conversion
    let spec = OpenAPIConverter::convert(&api);
    assert_eq!(spec.info.title, "Test API");
    assert_eq!(spec.info.version, "1.0");
    
    // Validate OpenAPI spec
    assert!(validate_openapi(&spec).is_ok());

    // Verify paths
    assert!(spec.paths.contains_key("/users"));
    assert!(spec.paths.contains_key("/users/{id}"));
    assert!(spec.paths.contains_key("/files/{path}"));

    // Verify CORS
    for (_, path_item) in spec.paths.iter() {
        assert!(path_item.options.is_some());
        if let Some(options) = &path_item.options {
            assert!(options.responses.contains_key("200"));
            let response = &options.responses["200"];
            assert!(response.headers.as_ref().unwrap().contains_key("Access-Control-Allow-Origin"));
        }
    }
}

#[tokio::test]
async fn test_swagger_ui_integration() {
    let temp_dir = TempDir::new().unwrap();
    let api = create_test_api().await;

    // Generate Swagger UI
    let generator = SwaggerGenerator::new(temp_dir.path());
    generator
        .generate("test-api", "/api/openapi/test-api/v1")
        .await
        .unwrap();

    // Create Swagger UI binding
    let binding = SwaggerUIBinding {
        spec_path: "/api/openapi/test-api/v1".to_string(),
    };

    // Test Swagger UI serving
    let handler = binding.create_handler();
    
    // Test index.html
    let req = Request::builder()
        .uri("/")
        .body(Body::empty())
        .unwrap();
    
    let resp = handler.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(response_contains_swagger_ui(resp).await);

    // Cleanup
    generator.clean("test-api").await.unwrap();
}

#[tokio::test]
async fn test_client_integration() {
    let api = create_test_api().await;
    let spec = OpenAPIConverter::convert(&api);

    // Simulate client usage
    let client_code = generate_test_client(&spec);
    assert!(client_code.contains("class TestApiClient"));
    assert!(client_code.contains("async listUsers"));
    assert!(client_code.contains("async updateUser"));
}

async fn response_contains_swagger_ui(resp: Response) -> bool {
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    html.contains("swagger-ui") && html.contains("SwaggerUIBundle")
}

fn generate_test_client(spec: &OpenAPISpec) -> String {
    // Mock client generation - in real implementation this would use a proper
    // OpenAPI client generator
    format!(
        r#"
class TestApiClient {{
    constructor(baseUrl) {{
        this.baseUrl = baseUrl;
    }}

    async listUsers(page, limit) {{
        // Implementation
    }}

    async updateUser(id, data) {{
        // Implementation
    }}
}}
        "#
    )
}

#[tokio::test]
async fn test_error_handling() {
    // Test invalid API definition
    let mut api = create_test_api().await;
    api.routes[0].path = "invalid//{path}".to_string(); // Invalid path format

    let spec = OpenAPIConverter::convert(&api);
    assert!(validate_openapi(&spec).is_err());

    // Test missing Swagger UI assets
    let temp_dir = TempDir::new().unwrap();
    let binding = SwaggerUIBinding {
        spec_path: "/non-existent".to_string(),
    };
    let handler = binding.create_handler();

    let req = Request::builder()
        .uri("/missing-asset.js")
        .body(Body::empty())
        .unwrap();
    
    let resp = handler.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
