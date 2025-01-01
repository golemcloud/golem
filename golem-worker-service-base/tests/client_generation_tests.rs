use golem_worker_service_base::gateway_api_definition::http::{
    openapi_export::{OpenApiExporter, OpenApiFormat},
    client_generator::ClientGenerator,
};
use utoipa::openapi::{
    path::{PathItem, OperationBuilder, HttpMethod, PathsBuilder, Parameter, ParameterIn},
    response::Response,
    schema::{Schema, SchemaType, Type, ObjectBuilder},
    Info, OpenApi, OpenApiVersion, Required, RefOr,
};
use tempfile::tempdir;
use std::fs;

#[tokio::test]
async fn test_client_generation() -> anyhow::Result<()> {
    // Create a test OpenAPI spec
    let mut openapi = OpenApi::new(
        Info::new("test-api", "1.0.0"),
        PathsBuilder::new(),
    );

    // Add /healthcheck endpoint
    let healthcheck_op = OperationBuilder::new()
        .operation_id(Some("getHealthCheck"))
        .response("200", Response::new("Health check response"))
        .build();
    let healthcheck_path = PathItem::new(HttpMethod::Get, healthcheck_op);
    openapi.paths.paths.insert("/healthcheck".to_string(), healthcheck_path);

    // Add /version endpoint
    let version_op = OperationBuilder::new()
        .operation_id(Some("getVersion"))
        .response("200", Response::new("Version response"))
        .build();
    let version_path = PathItem::new(HttpMethod::Get, version_op);
    openapi.paths.paths.insert("/version".to_string(), version_path);

    // Add /v1/api/definitions/{api_id}/version/{version}/export endpoint
    let mut api_id_param = Parameter::new("api_id");
    api_id_param.required = Required::True;
    api_id_param.parameter_in = ParameterIn::Path;
    api_id_param.schema = Some(RefOr::T(Schema::Object(
        ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .build()
    )));

    let mut version_param = Parameter::new("version");
    version_param.required = Required::True;
    version_param.parameter_in = ParameterIn::Path;
    version_param.schema = Some(RefOr::T(Schema::Object(
        ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .build()
    )));

    let export_op = OperationBuilder::new()
        .operation_id(Some("exportApiDefinition"))
        .parameter(api_id_param)
        .parameter(version_param)
        .response("200", Response::new("API definition response"))
        .build();
    let export_path = PathItem::new(HttpMethod::Get, export_op);
    openapi.paths.paths.insert("/v1/api/definitions/{api_id}/version/{version}/export".to_string(), export_path);

    // Set OpenAPI version
    openapi.openapi = OpenApiVersion::Version31;

    // Export OpenAPI schema
    let temp_dir = tempdir()?;
    let openapi_exporter = OpenApiExporter;
    let format = OpenApiFormat { json: true };
    let json_content = openapi_exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi.clone(),
        &format
    );

    // Write OpenAPI schema to file
    let openapi_json_path = temp_dir.path().join("openapi.json");
    fs::write(&openapi_json_path, &json_content)?;

    // Generate Rust client
    let generator = ClientGenerator::new(temp_dir.path());
    let rust_client_dir = generator
        .generate_rust_client("test-api", "1.0.0", openapi.clone(), "test_client")
        .await?;

    // Verify Rust client
    assert!(rust_client_dir.exists());
    assert!(rust_client_dir.join("Cargo.toml").exists());
    assert!(rust_client_dir.join("src/lib.rs").exists());

    // Generate TypeScript client
    let ts_client_dir = generator
        .generate_typescript_client("test-api", "1.0.0", openapi.clone(), "@test/client")
        .await?;

    // Verify TypeScript client
    assert!(ts_client_dir.exists());
    assert!(ts_client_dir.join("package.json").exists());
    assert!(ts_client_dir.join("src").exists());

    Ok(())
} 