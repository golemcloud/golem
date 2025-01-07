use golem_worker_service_base::gateway_api_definition::http::{
    openapi_export::{OpenApiExporter, OpenApiFormat},
    client_generator::ClientGenerator,
};
use poem_openapi::{
    payload::PlainText,
    param::Path,
    ApiResponse, OpenApi, OpenApiService,
};
use tempfile::tempdir;
use std::fs;

#[derive(ApiResponse)]
enum HealthCheckResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
}

#[derive(ApiResponse)]
enum VersionResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
}

#[derive(ApiResponse)]
enum ExportResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
}

#[derive(Clone)]
struct Api;

#[OpenApi]
impl Api {
    #[oai(path = "/healthcheck", method = "get")]
    async fn get_health_check(&self) -> HealthCheckResponse {
        HealthCheckResponse::Ok(PlainText("Healthy".to_string()))
    }

    #[oai(path = "/version", method = "get")]
    async fn get_version(&self) -> VersionResponse {
        VersionResponse::Ok(PlainText("1.0.0".to_string()))
    }

    #[oai(path = "/v1/api/definitions/{api_id}/version/{version}/export", method = "get")]
    async fn export_api_definition(
        &self,
        _api_id: Path<String>,
        _version: Path<String>,
    ) -> ExportResponse {
        ExportResponse::Ok(PlainText("API definition".to_string()))
    }
}

#[tokio::test]
async fn test_client_generation() -> anyhow::Result<()> {
    let api = Api;
    let _api_service = OpenApiService::new(api.clone(), "Test API", "1.0.0")
        .server("http://localhost:3000");
    
    // Export OpenAPI schema
    let temp_dir = tempdir()?;
    let openapi_exporter = OpenApiExporter;
    let format = OpenApiFormat { json: true };
    let json_content = openapi_exporter.export_openapi(
        api.clone(),
        &format
    );

    // Write OpenAPI schema to file
    let openapi_json_path = temp_dir.path().join("openapi.json");
    fs::write(&openapi_json_path, &json_content)?;

    // Generate Rust client
    let generator = ClientGenerator::new(temp_dir.path());
    let rust_client_dir = generator
        .generate_rust_client("test-api", "1.0.0", api.clone(), "test_client")
        .await?;

    // Verify Rust client
    assert!(rust_client_dir.exists());
    assert!(rust_client_dir.join("Cargo.toml").exists());
    assert!(rust_client_dir.join("src/lib.rs").exists());

    // Generate TypeScript client
    let ts_client_dir = generator
        .generate_typescript_client("test-api", "1.0.0", api, "@test/client")
        .await?;

    // Verify TypeScript client
    assert!(ts_client_dir.exists());
    assert!(ts_client_dir.join("package.json").exists());
    assert!(ts_client_dir.join("src").exists());

    Ok(())
} 