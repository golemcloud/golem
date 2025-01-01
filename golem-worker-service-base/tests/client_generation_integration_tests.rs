use golem_worker_service_base::gateway_api_definition::http::{
    client_generator::ClientGenerator,
    openapi_export::{OpenApiExporter, OpenApiFormat},
};
use tempfile::tempdir;
use tokio;
use utoipa::openapi::{
    path::{OperationBuilder, PathItem, HttpMethod, PathsBuilder},
    response::Response,
    Content, Info, OpenApi, RefOr, Schema, ResponsesBuilder,
};
use utoipa::openapi::schema::{Array, ObjectBuilder, Type};
use indexmap::IndexMap;
use std::fs;
use axum::{
    Router,
    routing,
    extract::Json,
};
use serde_json::json;
use serde_yaml;

#[tokio::test]
async fn test_client_generation_workflow() {
    // Load and parse the test API definition
    let api_yaml = include_str!("fixtures/test_api_definition.yaml");
    let openapi: OpenApi = serde_yaml::from_str(api_yaml).unwrap();

    // Export OpenAPI schema
    let temp_dir = tempdir().unwrap();
    let openapi_exporter = OpenApiExporter;
    let openapi_json_path = temp_dir.path().join("openapi.json");
    let json_content = openapi_exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi.clone(),
        &OpenApiFormat { json: true }
    );
    fs::write(&openapi_json_path, &json_content).unwrap();

    println!("\n=== Generated OpenAPI Schema ===\n{}\n", json_content);

    // Generate Rust client
    let generator = ClientGenerator::new(temp_dir.path());
    let rust_client_dir = match generator
        .generate_rust_client("test-api", "1.0.0", openapi.clone(), "test_client")
        .await
    {
        Ok(dir) => {
            println!("\n=== Rust Client Generated at {} ===", dir.display());
            if dir.exists() {
                println!("\nDirectory contents:");
                for entry in fs::read_dir(&dir).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    if path.is_file() {
                        println!("\n--- {} ---\n{}", path.display(), fs::read_to_string(&path).unwrap_or_default());
                    } else {
                        println!("Directory: {}", path.display());
                        if path.ends_with("src") {
                            for src_entry in fs::read_dir(&path).unwrap() {
                                let src_entry = src_entry.unwrap();
                                let src_path = src_entry.path();
                                if src_path.is_file() {
                                    println!("\n--- {} ---\n{}", src_path.display(), fs::read_to_string(&src_path).unwrap_or_default());
                                }
                            }
                        }
                    }
                }
            } else {
                println!("Directory does not exist");
            }
            dir
        }
        Err(e) => {
            println!("Failed to generate Rust client: {}", e);
            panic!("Rust client generation failed");
        }
    };

    // Verify Rust client
    assert!(rust_client_dir.exists());
    assert!(rust_client_dir.join("Cargo.toml").exists());
    assert!(rust_client_dir.join("src/lib.rs").exists());
    
    // Check if the Rust client compiles
    #[cfg(windows)]
    let status = tokio::process::Command::new("powershell")
        .arg("-Command")
        .arg(format!(
            "cargo check --manifest-path {}",
            rust_client_dir.join("Cargo.toml").to_string_lossy()
        ))
        .status()
        .await
        .unwrap();

    #[cfg(not(windows))]
    let status = tokio::process::Command::new("cargo")
        .args(["check", "--manifest-path"])
        .arg(rust_client_dir.join("Cargo.toml"))
        .status()
        .await
        .unwrap();

    assert!(status.success(), "Rust client failed to compile");

    println!("\nRust client generated successfully at: {}", rust_client_dir.display());
    println!("You can use this client by adding it as a dependency in your Cargo.toml:");
    println!("test_client = {{ path = \"{}\" }}", rust_client_dir.display());

    // Generate TypeScript client
    let ts_client_dir = match generator
        .generate_typescript_client("test-api", "1.0.0", openapi.clone(), "@test/client")
        .await
    {
        Ok(dir) => {
            println!("\n=== TypeScript Client Generated at {} ===", dir.display());
            if dir.exists() {
                println!("\nDirectory contents:");
                for entry in fs::read_dir(&dir).unwrap() {
                    let entry = entry.unwrap();
                    println!("  {}", entry.path().display());
                }
            }
            dir
        }
        Err(e) => {
            println!("Failed to generate TypeScript client: {}", e);
            panic!("TypeScript client generation failed");
        }
    };

    // Create a test server with all the endpoints from test_api_definition.yaml
    let app = Router::new()
        .route("/healthcheck", routing::get(|| async {
            Json(json!({}))
        }))
        .route("/version", routing::get(|| async {
            Json(json!({
                "version": "1.0.0"
            }))
        }))
        .route("/v1/api/definitions/:api_id/version/:version/export", routing::get(|axum::extract::Path((api_id, version)): axum::extract::Path<(String, String)>| async move {
            Json(json!({
                "openapi": "3.1.0",
                "info": {
                    "title": format!("{} API", api_id),
                    "version": version
                }
            }))
        }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    println!("\nTest server listening on http://{}", addr);

    // Spawn the server in the background
    let server_handle = {
        let app = app.clone();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        })
    };

    // Create a test script that uses the TypeScript client
    let test_script = format!(r#"
        import {{ Configuration, DefaultApi, ExportApiDefinitionRequest }} from './src';
        import fetch from 'node-fetch';
        
        // Fix fetch type
        globalThis.fetch = fetch as unknown as typeof globalThis.fetch;

        async function testClient() {{
            const config = new Configuration({{
                basePath: 'http://{}',
            }});
            const api = new DefaultApi(config);

            try {{
                // Test GET /healthcheck
                console.log('Testing GET /healthcheck...');
                const health = await api.getHealthCheck();
                console.assert(Object.keys(health).length === 0, 'GET /healthcheck failed: expected empty object');

                // Test GET /version
                console.log('Testing GET /version...');
                const version = await api.getVersion();
                console.assert(version.version === '1.0.0', 'GET /version failed: version mismatch');

                // Test GET /v1/api/definitions/test-api/version/1.0.0/export
                console.log('Testing GET /v1/api/definitions/test-api/version/1.0.0/export...');
                const request: ExportApiDefinitionRequest = {{
                    apiId: 'test-api',
                    version: '1.0.0'
                }};
                const apiDef = await api.exportApiDefinition(request);
                console.assert(apiDef.openapi === '3.1.0', 'GET /v1/api/definitions/test-api/version/1.0.0/export failed: openapi version mismatch');
                console.assert(apiDef.info.title === 'test-api API', 'GET /v1/api/definitions/test-api/version/1.0.0/export failed: title mismatch');
                console.assert(apiDef.info.version === '1.0.0', 'GET /v1/api/definitions/test-api/version/1.0.0/export failed: version mismatch');

                console.log('All TypeScript client tests passed!');
                process.exit(0);
            }} catch (error) {{
                console.error('Test failed:', error);
                process.exit(1);
            }}
        }}

        testClient().catch(error => {{
            console.error('Unhandled error:', error);
            process.exit(1);
        }});
    "#, addr);

    fs::write(ts_client_dir.join("test.ts"), test_script).unwrap();

    // Install dependencies and run the test
    println!("\nChecking for npm...");
    #[cfg(windows)]
    let npm_check = tokio::process::Command::new("npm.cmd")
        .arg("--version")
        .output()
        .await;

    #[cfg(not(windows))]
    let npm_check = tokio::process::Command::new("npm")
        .arg("--version")
        .output()
        .await;

    match &npm_check {
        Ok(output) => {
            println!("npm version: {}", String::from_utf8_lossy(&output.stdout));
            if !output.status.success() {
                panic!("npm check failed with stderr: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            panic!("Failed to check npm: {}", e);
        }
    }

    // Initialize npm project
    println!("Initializing npm project...");
    #[cfg(windows)]
    let init_status = tokio::process::Command::new("npm.cmd")
        .args(["init", "-y"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    #[cfg(not(windows))]
    let init_status = tokio::process::Command::new("npm")
        .args(["init", "-y"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    match init_status {
        Ok(status) => {
            if !status.success() {
                panic!("Failed to initialize npm project");
            }
        }
        Err(e) => {
            panic!("Failed to run npm init: {}", e);
        }
    }

    // Install TypeScript and ts-node
    println!("Installing TypeScript dependencies...");
    #[cfg(windows)]
    let ts_install_status = tokio::process::Command::new("npm.cmd")
        .args(["install", "typescript", "ts-node", "--save-dev"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    #[cfg(not(windows))]
    let ts_install_status = tokio::process::Command::new("npm")
        .args(["install", "typescript", "ts-node", "--save-dev"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    match ts_install_status {
        Ok(status) => {
            if !status.success() {
                panic!("Failed to install TypeScript dependencies");
            }
        }
        Err(e) => {
            panic!("Failed to install TypeScript: {}", e);
        }
    }

    println!("Installing node-fetch dependencies...");
    #[cfg(windows)]
    let fetch_install_status = tokio::process::Command::new("npm.cmd")
        .args(["install", "node-fetch", "@types/node-fetch", "--save-dev"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    #[cfg(not(windows))]
    let fetch_install_status = tokio::process::Command::new("npm")
        .args(["install", "node-fetch", "@types/node-fetch", "--save-dev"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    match fetch_install_status {
        Ok(status) => {
            if !status.success() {
                panic!("Failed to install node-fetch dependencies");
            }
        }
        Err(e) => {
            panic!("Failed to install node-fetch: {}", e);
        }
    }

    println!("Running TypeScript client tests...");
    #[cfg(windows)]
    let test_status = tokio::process::Command::new("npx.cmd")
        .args(["ts-node", "test.ts"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    #[cfg(not(windows))]
    let test_status = tokio::process::Command::new("npx")
        .args(["ts-node", "test.ts"])
        .current_dir(&ts_client_dir)
        .status()
        .await;

    // Clean up
    server_handle.abort();

    match test_status {
        Ok(status) => {
            if !status.success() {
                panic!("TypeScript client tests failed");
            }
        }
        Err(e) => {
            panic!("Failed to run TypeScript tests: {}", e);
        }
    }

    println!("\nAll client tests passed successfully!");

    // Print TypeScript client files
    println!("\nGenerated TypeScript client files:");
    for entry in fs::read_dir(ts_client_dir.join("src")).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            println!("\n=== {} ===\n{}", path.display(), fs::read_to_string(&path).unwrap());
        }
    }
}

#[tokio::test]
#[ignore = "Requires Node.js and TypeScript to be installed"]
async fn test_typescript_client_generation() {
    // Check if Node.js is installed
    let node_check = tokio::process::Command::new("node")
        .arg("--version")
        .output()
        .await;

    match &node_check {
        Ok(output) => println!("Node.js version: {}", String::from_utf8_lossy(&output.stdout)),
        Err(e) => {
            println!("Failed to check Node.js: {}", e);
            println!("Skipping TypeScript client test: Node.js is not installed");
            return;
        }
    }

    // Check if TypeScript is installed
    println!("Checking for TypeScript...");
    #[cfg(windows)]
    let tsc_check = tokio::process::Command::new("npx.cmd")
        .args(["tsc", "--version"])
        .output()
        .await;

    #[cfg(not(windows))]
    let tsc_check = tokio::process::Command::new("npx")
        .args(["tsc", "--version"])
        .output()
        .await;

    match &tsc_check {
        Ok(output) => {
            println!("TypeScript version: {}", String::from_utf8_lossy(&output.stdout));
            if !output.status.success() {
                println!("TypeScript check failed with stderr: {}", String::from_utf8_lossy(&output.stderr));
                println!("Skipping TypeScript client test: TypeScript check failed");
                return;
            }
        }
        Err(e) => {
            println!("Failed to check TypeScript: {}", e);
            println!("Skipping TypeScript client test: TypeScript is not installed");
            return;
        }
    }

    println!("TypeScript is installed, proceeding with test...");

    // Create test OpenAPI spec
    let mut openapi = OpenApi::new(Info::new("Test API", "1.0.0"), PathsBuilder::new());

    // Add a test endpoint
    let mut get_path = PathItem::new(HttpMethod::Get, OperationBuilder::new().build());
    let get_operation = {
        // Build the response content
        let mut content = IndexMap::new();
        content.insert(
            "application/json".to_string(),
            Content::new(Some(RefOr::T(Schema::Array(
                Array::new(Schema::Object(
                    ObjectBuilder::new()
                        .schema_type(Type::Object)
                        .property("id", Schema::Object(ObjectBuilder::new().schema_type(Type::String).into()))
                        .property("name", Schema::Object(ObjectBuilder::new().schema_type(Type::String).into()))
                        .required("id")
                        .required("name")
                        .into()
                ))
            )))),
        );

        // Build the responses
        let responses = ResponsesBuilder::new()
            .response("200", {
                let mut response = Response::new("List of items");
                response.content = content;
                response
            })
            .build();

        // Build the operation
        OperationBuilder::new()
            .operation_id(Some("getItems"))
            .description(Some("Get items with optional filtering"))
            .responses(responses)
            .build()
    };
    get_path.get = Some(get_operation);
    openapi.paths.paths.insert("/items".to_string(), get_path);

    // Export OpenAPI schema
    let temp_dir = tempdir().unwrap();
    let openapi_exporter = OpenApiExporter;
    let openapi_json_path = temp_dir.path().join("openapi.json");
    let json_content = openapi_exporter.export_openapi(
        "test-api",
        "1.0.0",
        openapi.clone(),
        &OpenApiFormat { json: true }
    );
    fs::write(&openapi_json_path, json_content).unwrap();

    // Generate TypeScript client
    let generator = ClientGenerator::new(temp_dir.path());
    let ts_client_dir = match generator
        .generate_typescript_client("test-api", "1.0.0", openapi, "@test/client")
        .await
    {
        Ok(dir) => {
            println!("TypeScript client directory: {}", dir.display());
            if dir.exists() {
                println!("Directory exists");
                let entries = fs::read_dir(&dir).unwrap();
                println!("Directory contents:");
                for entry in entries {
                    let entry = entry.unwrap();
                    println!("  {}", entry.path().display());
                }

                // Create a basic tsconfig.json
                let tsconfig = r#"{
                    "compilerOptions": {
                        "target": "es2020",
                        "module": "commonjs",
                        "strict": true,
                        "esModuleInterop": true,
                        "skipLibCheck": true,
                        "forceConsistentCasingInFileNames": true
                    }
                }"#;
                fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
                println!("Created tsconfig.json");
            } else {
                println!("Directory does not exist");
            }
            dir
        }
        Err(e) => {
            println!("Failed to generate TypeScript client: {}", e);
            panic!("TypeScript client generation failed");
        }
    };

    // Verify TypeScript client
    assert!(ts_client_dir.exists());
    assert!(ts_client_dir.join("package.json").exists());
    assert!(ts_client_dir.join("src").exists());
    
    // Print the contents of the src directory
    println!("\nContents of src directory:");
    for entry in fs::read_dir(ts_client_dir.join("src")).unwrap() {
        let entry = entry.unwrap();
        println!("  {}", entry.path().display());
    }

    // Check if the TypeScript client compiles
    #[cfg(windows)]
    let status = tokio::process::Command::new("npx.cmd")
        .args(["tsc", "-p"])
        .arg(ts_client_dir)
        .status()
        .await
        .unwrap();

    #[cfg(not(windows))]
    let status = tokio::process::Command::new("npx")
        .args(["tsc", "-p"])
        .arg(ts_client_dir)
        .status()
        .await
        .unwrap();

    assert!(status.success(), "TypeScript client failed to compile");
} 