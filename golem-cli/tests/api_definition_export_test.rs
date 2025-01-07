use anyhow::Result;
use golem_cli::model::{ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion};
use golem_cli::test_helpers::TestCli;
use std::fs;
use tempfile::TempDir;
use webbrowser;

#[tokio::test]
async fn test_api_definition_export_and_swagger() -> Result<()> {
    // Create a test CLI instance
    let cli = TestCli::new()?;

    // Create a temporary directory for test files
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Test parameters
    let api_id = ApiDefinitionId("test-api".to_string());
    let version = ApiDefinitionVersion("1.0.0".to_string());

    // First, let's create a simple API definition to work with
    let api_def = r#"{
        "id": "test-api",
        "version": "1.0.0",
        "routes": [
            {
                "path": "/test",
                "method": "GET",
                "binding": {
                    "response_mapping_output": {
                        "types": {
                            "response": {
                                "type": "record",
                                "fields": [
                                    {
                                        "name": "message",
                                        "type": "string"
                                    }
                                ]
                            }
                        }
                    }
                }
            }
        ]
    }"#;

    // Write the API definition to a temporary file
    let api_def_path = temp_path.join("test-api.json");
    fs::write(&api_def_path, api_def)?;

    // Import the API definition
    cli.run(&["api-definition", "import", api_def_path.to_str().unwrap()])?;

    // Test the export command with JSON format
    let json_result = cli.run(&[
        "api-definition",
        "export",
        "--id",
        "test-api",
        "--version",
        "1.0.0",
        "--format",
        "json",
    ])?;

    // Verify JSON export contains expected OpenAPI elements
    assert!(json_result.contains("openapi"));
    assert!(json_result.contains("/test"));
    assert!(json_result.contains("GET"));

    // Test the export command with YAML format
    let yaml_result = cli.run(&[
        "api-definition",
        "export",
        "--id",
        "test-api",
        "--version",
        "1.0.0",
        "--format",
        "yaml",
    ])?;

    // Verify YAML export contains expected OpenAPI elements
    assert!(yaml_result.contains("openapi:"));
    assert!(yaml_result.contains("/test:"));
    assert!(yaml_result.contains("get:"));

    // Test the swagger command
    // Note: We can't actually open a browser in tests, but we can verify the URL is correct
    let swagger_result = cli.run(&[
        "api-definition",
        "swagger",
        "--id",
        "test-api",
        "--version",
        "1.0.0",
    ])?;

    // Verify the swagger result contains a valid URL
    assert!(swagger_result.contains("/swagger-ui/api-definitions/test-api/1.0.0"));

    Ok(())
} 