use crate::swagger::{SwaggerUIBinding, SwaggerGenerator};
use crate::api::definition::types::BindingType;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use tower::ServiceExt;
use std::path::PathBuf;
use tempfile::TempDir;

async fn setup_test_env() -> (TempDir, SwaggerUIBinding, PathBuf) {
    let temp_dir = TempDir::new().unwrap();
    let output_dir = temp_dir.path().to_path_buf();
    
    let binding = SwaggerUIBinding {
        spec_path: "/api/openapi/test-api/v1".to_string(),
    };

    // Generate Swagger UI files
    let generator = SwaggerGenerator::new(&output_dir);
    generator.generate("test-api", &binding.spec_path).await.unwrap();

    (temp_dir, binding, output_dir)
}

#[tokio::test]
async fn test_swagger_ui_binding_creation() {
    let (_temp_dir, binding, _output_dir) = setup_test_env().await;
    assert_eq!(binding.spec_path, "/api/openapi/test-api/v1");
}

#[tokio::test]
async fn test_swagger_ui_handler() {
    let (_temp_dir, binding, _output_dir) = setup_test_env().await;
    let handler = binding.create_handler();

    // Test index.html request
    let index_req = Request::builder()
        .uri("/")
        .body(Body::empty())
        .unwrap();
    
    let resp = handler.oneshot(index_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(response_contains_swagger_ui(resp).await);

    // Test static asset request
    let asset_req = Request::builder()
        .uri("/swagger-ui.css")
        .body(Body::empty())
        .unwrap();
    
    let resp = handler.oneshot(asset_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/css; charset=utf-8"
    );
}

#[tokio::test]
async fn test_swagger_ui_not_found() {
    let (_temp_dir, binding, _output_dir) = setup_test_env().await;
    let handler = binding.create_handler();

    let req = Request::builder()
        .uri("/non-existent-file")
        .body(Body::empty())
        .unwrap();
    
    let resp = handler.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_swagger_ui_cors() {
    let (_temp_dir, binding, _output_dir) = setup_test_env().await;
    let handler = binding.create_handler();

    let req = Request::builder()
        .uri("/")
        .method("OPTIONS")
        .body(Body::empty())
        .unwrap();
    
    let resp = handler.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().contains_key("access-control-allow-origin"));
    assert!(resp.headers().contains_key("access-control-allow-methods"));
}

async fn response_contains_swagger_ui(resp: Response) -> bool {
    let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    let html = String::from_utf8_lossy(&bytes);
    html.contains("swagger-ui") && html.contains("SwaggerUIBundle")
}

#[tokio::test]
async fn test_swagger_ui_cleanup() {
    let (temp_dir, _binding, output_dir) = setup_test_env().await;
    
    // Verify files exist
    assert!(output_dir.join("test-api/index.html").exists());
    assert!(output_dir.join("test-api/swagger-ui.css").exists());

    // Clean up
    let generator = SwaggerGenerator::new(&output_dir);
    generator.clean("test-api").await.unwrap();

    // Verify cleanup
    assert!(!output_dir.join("test-api").exists());
    
    // Keep temp_dir alive until end of test
    drop(temp_dir);
}
