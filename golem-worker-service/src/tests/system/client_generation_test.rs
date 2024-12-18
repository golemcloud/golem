use crate::api::definition::types::{ApiDefinition, Route, HttpMethod, BindingType};
use crate::api::openapi::OpenAPIConverter;
use openapi_generator::Generator;
use std::process::Command;
use tempfile::TempDir;

#[tokio::test]
async fn test_typescript_client() {
    let temp_dir = TempDir::new().unwrap();
    let api = create_test_api().await;
    let spec = OpenAPIConverter::convert(&api);
    
    // Generate TypeScript client
    let generator = Generator::new()
        .language("typescript-fetch")
        .input_spec(&spec)
        .output_dir(temp_dir.path());
    
    generator.generate().await.unwrap();

    // Verify generated files
    assert!(temp_dir.path().join("api.ts").exists());
    assert!(temp_dir.path().join("package.json").exists());

    // Test npm installation and execution
    assert!(Command::new("npm")
        .current_dir(temp_dir.path())
        .args(&["install"])
        .status()
        .unwrap()
        .success());
        
    assert!(Command::new("npm")
        .current_dir(temp_dir.path())
        .args(&["test"])
        .status()
        .unwrap()
        .success());
}

async fn create_test_api() -> ApiDefinition {
    ApiDefinition {
        id: "test-api".to_string(),
        name: "Test API".to_string(),
        version: "1.0".to_string(),
        description: "Test API for system tests".to_string(),
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
        ],
    }
}
