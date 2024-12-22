#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::fs;
    use std::time::Duration;
    use assert2::assert;
    use golem_worker_service_base::gateway_api_definition::http::{
        HttpApiDefinition, ApiDefinitionId, ApiVersion, Route, MethodPattern, AllPathPatterns,
    };
    use golem_worker_service_base::gateway_binding::{GatewayBinding, WorkerBinding};
    use golem_common::model::{ComponentId, VersionedComponentId};
    use golem_wasm_ast::rib::expr::Expr;
    use golem_worker_service_base::gateway_binding::ResponseMapping;
    use serde::{Serialize, Deserialize};
    use tokio::runtime::Runtime;
    use tokio::time::sleep;
    use uuid::Uuid;

    mod test_server;
    use test_server::{TestServer, TestUser, TestResponse};

    async fn start_test_server() {
        let server = TestServer::new();
        tokio::spawn(async move {
            server.start(3000).await;
        });
        // Give the server time to start
        sleep(Duration::from_secs(1)).await;
    }

    fn verify_typescript_client() -> Result<(), Box<dyn std::error::Error>> {
        // Install dependencies
        Command::new("npm")
            .current_dir("tests/client_generation_tests/typescript/client")
            .arg("install")
            .status()?;

        // Create test file
        let test_code = r#"
import { Configuration, DefaultApi } from './';

async function test() {
    const config = new Configuration({
        basePath: 'http://localhost:3000'
    });
    const api = new DefaultApi(config);

    // Test create user
    const newUser = {
        id: 1,
        name: 'Test User',
        email: 'test@example.com'
    };
    const createResponse = await api.createUser(newUser);
    console.assert(createResponse.data.status === 'success', 'Create user failed');

    // Test get user
    const getResponse = await api.getUser(1);
    console.assert(getResponse.data.data.name === 'Test User', 'Get user failed');

    // Test update user
    const updatedUser = { ...newUser, name: 'Updated User' };
    const updateResponse = await api.updateUser(1, updatedUser);
    console.assert(updateResponse.data.data.name === 'Updated User', 'Update user failed');

    // Test delete user
    const deleteResponse = await api.deleteUser(1);
    console.assert(deleteResponse.data.status === 'success', 'Delete user failed');
}

test().catch(console.error);
"#;
        fs::write(
            "tests/client_generation_tests/typescript/client/test.ts",
            test_code,
        )?;

        // Run the test
        Command::new("npx")
            .current_dir("tests/client_generation_tests/typescript/client")
            .args(&["ts-node", "test.ts"])
            .status()?;

        Ok(())
    }

    fn verify_python_client() -> Result<(), Box<dyn std::error::Error>> {
        // Install dependencies
        Command::new("pip")
            .args(&["install", "-r", "requirements.txt"])
            .current_dir("tests/client_generation_tests/python/client")
            .status()?;

        // Create test file
        let test_code = r#"
import unittest
from __future__ import absolute_import
import os
import sys
sys.path.append(".")

import openapi_client
from openapi_client.rest import ApiException

class TestDefaultApi(unittest.TestCase):
    def setUp(self):
        configuration = openapi_client.Configuration(
            host="http://localhost:3000"
        )
        self.api = openapi_client.DefaultApi(openapi_client.ApiClient(configuration))

    def test_crud_operations(self):
        # Test create user
        new_user = {
            "id": 1,
            "name": "Test User",
            "email": "test@example.com"
        }
        response = self.api.create_user(new_user)
        self.assertEqual(response.status, "success")

        # Test get user
        response = self.api.get_user(1)
        self.assertEqual(response.data.name, "Test User")

        # Test update user
        updated_user = {
            "id": 1,
            "name": "Updated User",
            "email": "test@example.com"
        }
        response = self.api.update_user(1, updated_user)
        self.assertEqual(response.data.name, "Updated User")

        # Test delete user
        response = self.api.delete_user(1)
        self.assertEqual(response.status, "success")

if __name__ == '__main__':
    unittest.main()
"#;
        fs::write(
            "tests/client_generation_tests/python/client/test_api.py",
            test_code,
        )?;

        // Run the test
        Command::new("python")
            .args(&["-m", "unittest", "test_api.py"])
            .current_dir("tests/client_generation_tests/python/client")
            .status()?;

        Ok(())
    }

    fn verify_rust_client() -> Result<(), Box<dyn std::error::Error>> {
        // Create test file
        let test_code = r#"
use test_api;
use test_api::{Configuration, DefaultApi};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Configuration::new("http://localhost:3000".to_string());
    let client = DefaultApi::new(config);

    // Test create user
    let new_user = TestUser {
        id: 1,
        name: "Test User".to_string(),
        email: "test@example.com".to_string(),
    };
    let response = client.create_user(new_user).await?;
    assert_eq!(response.status, "success");

    // Test get user
    let response = client.get_user(1).await?;
    assert_eq!(response.data.unwrap().name, "Test User");

    // Test update user
    let updated_user = TestUser {
        id: 1,
        name: "Updated User".to_string(),
        email: "test@example.com".to_string(),
    };
    let response = client.update_user(1, updated_user).await?;
    assert_eq!(response.data.unwrap().name, "Updated User");

    // Test delete user
    let response = client.delete_user(1).await?;
    assert_eq!(response.status, "success");

    Ok(())
}
"#;
        fs::write(
            "tests/client_generation_tests/rust/client/examples/test.rs",
            test_code,
        )?;

        // Build and run the test
        Command::new("cargo")
            .args(&["run", "--example", "test"])
            .current_dir("tests/client_generation_tests/rust/client")
            .status()?;

        Ok(())
    }

    fn setup_test_api() -> HttpApiDefinition {
        // Create a test API definition with CRUD endpoints
        let mut api_def = HttpApiDefinition::new(
            ApiDefinitionId("test-api".to_string()),
            ApiVersion("1.0".to_string()),
        );

        // Create a test component ID
        let component_id = VersionedComponentId {
            component_id: ComponentId(Uuid::new_v4()),
            version: 1,
        };

        // Add test routes
        let routes = vec![
            // GET /users/{id}
            Route {
                method: MethodPattern::Get,
                path: AllPathPatterns::parse("/users/{id}").unwrap(),
                binding: GatewayBinding::Default(WorkerBinding {
                    component_id: component_id.clone(),
                    worker_name: None,
                    idempotency_key: None,
                    response_mapping: ResponseMapping(Expr::literal("${response}")),
                }),
                middlewares: None,
            },
            // POST /users
            Route {
                method: MethodPattern::Post,
                path: AllPathPatterns::parse("/users").unwrap(),
                binding: GatewayBinding::Default(WorkerBinding {
                    component_id: component_id.clone(),
                    worker_name: None,
                    idempotency_key: None,
                    response_mapping: ResponseMapping(Expr::literal("${response}")),
                }),
                middlewares: None,
            },
            // PUT /users/{id}
            Route {
                method: MethodPattern::Put,
                path: AllPathPatterns::parse("/users/{id}").unwrap(),
                binding: GatewayBinding::Default(WorkerBinding {
                    component_id: component_id.clone(),
                    worker_name: None,
                    idempotency_key: None,
                    response_mapping: ResponseMapping(Expr::literal("${response}")),
                }),
                middlewares: None,
            },
            // DELETE /users/{id}
            Route {
                method: MethodPattern::Delete,
                path: AllPathPatterns::parse("/users/{id}").unwrap(),
                binding: GatewayBinding::Default(WorkerBinding {
                    component_id: component_id.clone(),
                    worker_name: None,
                    idempotency_key: None,
                    response_mapping: ResponseMapping(Expr::literal("${response}")),
                }),
                middlewares: None,
            },
        ];

        api_def.routes = routes;
        api_def
    }

    fn generate_client_library(openapi_spec: &str, lang: &str, output_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        // Ensure openapi-generator-cli is installed
        let status = Command::new("openapi-generator-cli")
            .arg("version")
            .status()?;

        if !status.success() {
            return Err("openapi-generator-cli not found. Please install it first.".into());
        }

        // Generate client library
        let status = Command::new("openapi-generator-cli")
            .args(&[
                "generate",
                "-i", openapi_spec,
                "-g", lang,
                "-o", output_dir.to_str().unwrap(),
            ])
            .status()?;

        if !status.success() {
            return Err("Failed to generate client library".into());
        }

        Ok(())
    }

    #[test]
    fn test_typescript_client_generation() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            // Start test server
            start_test_server().await;

            // Setup test API
            let api_def = setup_test_api();

            // Export OpenAPI spec
            let openapi_json = api_def.to_openapi_string("json").unwrap();
            let spec_path = PathBuf::from("tests/client_generation_tests/typescript/openapi.json");
            fs::write(&spec_path, openapi_json).unwrap();

            // Generate TypeScript client
            let output_dir = PathBuf::from("tests/client_generation_tests/typescript/client");
            generate_client_library(
                spec_path.to_str().unwrap(),
                "typescript-fetch",
                &output_dir,
            ).unwrap();

            // Verify the generated client
            verify_typescript_client().unwrap();
        });
    }

    #[test]
    fn test_python_client_generation() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            // Start test server
            start_test_server().await;

            // Setup test API
            let api_def = setup_test_api();

            // Export OpenAPI spec
            let openapi_json = api_def.to_openapi_string("json").unwrap();
            let spec_path = PathBuf::from("tests/client_generation_tests/python/openapi.json");
            fs::write(&spec_path, openapi_json).unwrap();

            // Generate Python client
            let output_dir = PathBuf::from("tests/client_generation_tests/python/client");
            generate_client_library(
                spec_path.to_str().unwrap(),
                "python",
                &output_dir,
            ).unwrap();

            // Verify the generated client
            verify_python_client().unwrap();
        });
    }

    #[test]
    fn test_rust_client_generation() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            // Start test server
            start_test_server().await;

            // Setup test API
            let api_def = setup_test_api();

            // Export OpenAPI spec
            let openapi_json = api_def.to_openapi_string("json").unwrap();
            let spec_path = PathBuf::from("tests/client_generation_tests/rust/openapi.json");
            fs::write(&spec_path, openapi_json).unwrap();

            // Generate Rust client
            let output_dir = PathBuf::from("tests/client_generation_tests/rust/client");
            generate_client_library(
                spec_path.to_str().unwrap(),
                "rust",
                &output_dir,
            ).unwrap();

            // Verify the generated client
            verify_rust_client().unwrap();
        });
    }
} 