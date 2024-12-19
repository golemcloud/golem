use std::process::Command;
use std::fs;
use std::path::PathBuf;
use golem_worker_gateway::openapi::ApiDefinitionConverter;

#[tokio::test]
async fn test_typescript_client_generation() {
    // 1. Create test directory
    let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("system_tests")
        .join("generated");
    fs::create_dir_all(&test_dir).unwrap();

    // 2. Generate OpenAPI spec
    let api = crate::integration_tests::create_test_api_definition();
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(&api).unwrap();
    let spec_path = test_dir.join("openapi.json");
    fs::write(&spec_path, serde_json::to_string_pretty(&openapi).unwrap()).unwrap();

    // 3. Generate TypeScript client using openapi-generator-cli
    let output_dir = test_dir.join("typescript-client");
    fs::create_dir_all(&output_dir).unwrap();

    let status = Command::new("npx")
        .args([
            "@openapitools/openapi-generator-cli",
            "generate",
            "-i",
            spec_path.to_str().unwrap(),
            "-g",
            "typescript-fetch",
            "-o",
            output_dir.to_str().unwrap(),
            "--additional-properties=supportsES6=true",
        ])
        .status()
        .expect("Failed to execute openapi-generator-cli");

    assert!(status.success());

    // 4. Create a test TypeScript file
    let test_file = output_dir.join("test.ts");
    fs::write(
        &test_file,
        r#"
import { Configuration, DefaultApi, CreateUserRequest } from './';

async function testApi() {
    const config = new Configuration({
        basePath: 'http://localhost:3000',
    });
    
    const api = new DefaultApi(config);
    
    try {
        const user = await api.createUser({
            name: 'Test User',
            age: 30,
        });
        
        console.log('Created user:', user);
        if (user.name !== 'Test User' || user.age !== 30) {
            process.exit(1);
        }
    } catch (error) {
        console.error('Error:', error);
        process.exit(1);
    }
}

testApi().catch(console.error);
"#,
    )
    .unwrap();

    // 5. Install dependencies and run the test
    Command::new("npm")
        .current_dir(&output_dir)
        .args(["install", "typescript", "@types/node", "ts-node"])
        .status()
        .expect("Failed to install dependencies");

    let status = Command::new("npx")
        .current_dir(&output_dir)
        .args(["ts-node", "test.ts"])
        .status()
        .expect("Failed to run TypeScript test");

    assert!(status.success());
}
