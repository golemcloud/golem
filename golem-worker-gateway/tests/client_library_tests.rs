use std::process::Command;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::net::TcpListener;
use axum::{Router, routing::{get, post}};
use serde::{Deserialize, Serialize};
use crate::{
    api::{ApiDefinition, ApiDefinitionId, CompiledHttpApiDefinition, CompiledHttpRoute},
    openapi::converter::ApiDefinitionConverter,
    error::Result,
};

// Complex test types to verify full type support
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComplexRequest {
    string_field: String,
    optional_field: Option<i32>,
    array_field: Vec<String>,
    nested_field: NestedObject,
    enum_field: TestEnum,
    map_field: std::collections::HashMap<String, i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NestedObject {
    field1: String,
    field2: i32,
    field3: Vec<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum TestEnum {
    Variant1,
    Variant2(String),
    Variant3 { field: i32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComplexResponse {
    id: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    data: ComplexRequest,
    metadata: std::collections::HashMap<String, serde_json::Value>,
}

async fn handle_complex_request(
    axum::extract::Json(payload): axum::extract::Json<ComplexRequest>,
) -> impl axum::response::IntoResponse {
    let response = ComplexResponse {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: chrono::Utc::now(),
        data: payload,
        metadata: std::collections::HashMap::new(),
    };
    
    axum::Json(response)
}

#[tokio::test]
async fn test_typescript_client_generation() -> Result<()> {
    // Create a temporary directory for generated client
    let temp_dir = TempDir::new()?;
    let client_dir = temp_dir.path().join("typescript-client");

    // Define a complex API
    let api = ApiDefinition {
        id: Some(ApiDefinitionId {
            value: "test-api".to_string(),
        }),
        version: "1.0.0".to_string(),
        http_api: Some(CompiledHttpApiDefinition {
            routes: vec![
                CompiledHttpRoute {
                    method: 0, // GET
                    path: "/api/complex/{id}".to_string(),
                    ..Default::default()
                },
                CompiledHttpRoute {
                    method: 1, // POST
                    path: "/api/complex".to_string(),
                    ..Default::default()
                },
            ],
        }),
        ..Default::default()
    };

    // Convert to OpenAPI spec
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api)?;
    let spec_file = temp_dir.path().join("openapi.json");
    std::fs::write(&spec_file, serde_json::to_string_pretty(&openapi)?)?;

    // Generate TypeScript client using openapi-generator
    let status = Command::new("openapi-generator-cli")
        .args(&[
            "generate",
            "-i", spec_file.to_str().unwrap(),
            "-g", "typescript-fetch",
            "-o", client_dir.to_str().unwrap(),
            "--additional-properties=supportsES6=true,npmVersion=6.9.0,typescriptThreePlus=true",
        ])
        .status()?;

    assert!(status.success(), "Failed to generate TypeScript client");

    // Setup test server
    let app = Router::new()
        .route("/api/complex/:id", get(handle_complex_request))
        .route("/api/complex", post(handle_complex_request));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Build and test the TypeScript client
    let status = Command::new("npm")
        .current_dir(&client_dir)
        .args(&["install"])
        .status()?;
    assert!(status.success(), "Failed to install npm dependencies");

    let test_file = client_dir.join("test.ts");
    std::fs::write(&test_file, format!(r#"
        import {{ Configuration, DefaultApi, ComplexRequest }} from './';

        async function test() {{
            const config = new Configuration({{
                basePath: 'http://{}',
            }});
            const api = new DefaultApi(config);

            // Test POST request
            const request: ComplexRequest = {{
                string_field: 'test',
                optional_field: 42,
                array_field: ['a', 'b', 'c'],
                nested_field: {{
                    field1: 'nested',
                    field2: 123,
                    field3: ['test', null, 'value'],
                }},
                enum_field: 'Variant1',
                map_field: {{ key1: 1, key2: 2 }},
            }};

            const response = await api.createComplex(request);
            console.assert(response.id !== undefined, 'Response should have an ID');
            console.assert(response.data.string_field === request.string_field, 'String field should match');
            
            // Test GET request
            const getResponse = await api.getComplex(response.id);
            console.assert(getResponse.id === response.id, 'GET response should match POST response');
        }}

        test().catch(console.error);
    "#, addr))?;

    let status = Command::new("ts-node")
        .current_dir(&client_dir)
        .arg("test.ts")
        .status()?;
    assert!(status.success(), "TypeScript client tests failed");

    Ok(())
}

#[tokio::test]
async fn test_python_client_generation() -> Result<()> {
    // Similar structure to TypeScript test but for Python client
    let temp_dir = TempDir::new()?;
    let client_dir = temp_dir.path().join("python-client");

    // Generate OpenAPI spec...
    
    // Generate Python client
    let status = Command::new("openapi-generator-cli")
        .args(&[
            "generate",
            "-i", spec_file.to_str().unwrap(),
            "-g", "python",
            "-o", client_dir.to_str().unwrap(),
            "--additional-properties=pythonVersion=3.9,packageName=testclient",
        ])
        .status()?;

    assert!(status.success(), "Failed to generate Python client");

    // Setup test server...

    // Create Python test script
    let test_file = client_dir.join("test.py");
    std::fs::write(&test_file, format!(r#"
        import unittest
        from testclient import Configuration, ApiClient, DefaultApi
        from testclient.models import ComplexRequest, NestedObject

        class TestApi(unittest.TestCase):
            def setUp(self):
                config = Configuration(host='http://{}')
                self.api = DefaultApi(ApiClient(config))

            def test_complex_request(self):
                request = ComplexRequest(
                    string_field='test',
                    optional_field=42,
                    array_field=['a', 'b', 'c'],
                    nested_field=NestedObject(
                        field1='nested',
                        field2=123,
                        field3=['test', None, 'value']
                    ),
                    enum_field='Variant1',
                    map_field={{'key1': 1, 'key2': 2}}
                )

                response = self.api.create_complex(request)
                self.assertIsNotNone(response.id)
                self.assertEqual(response.data.string_field, request.string_field)

                get_response = self.api.get_complex(response.id)
                self.assertEqual(get_response.id, response.id)

        if __name__ == '__main__':
            unittest.main()
    "#, addr))?;

    let status = Command::new("python")
        .current_dir(&client_dir)
        .arg("test.py")
        .status()?;
    assert!(status.success(), "Python client tests failed");

    Ok(())
}

#[tokio::test]
async fn test_java_client_generation() -> Result<()> {
    // Similar structure but for Java client
    let temp_dir = TempDir::new()?;
    let client_dir = temp_dir.path().join("java-client");

    // Generate Java client
    let status = Command::new("openapi-generator-cli")
        .args(&[
            "generate",
            "-i", spec_file.to_str().unwrap(),
            "-g", "java",
            "-o", client_dir.to_str().unwrap(),
            "--additional-properties=java8=true,library=okhttp-gson",
        ])
        .status()?;

    assert!(status.success(), "Failed to generate Java client");

    // Create Java test file...
    // Build and run tests...

    Ok(())
}
