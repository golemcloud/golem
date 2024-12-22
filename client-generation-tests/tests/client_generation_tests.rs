#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::fs;
    use std::time::Duration;
    use assert2::assert;
    use golem_worker_service_base::gateway_api_definition::{
        ApiDefinitionId, ApiVersion,
        http::{HttpApiDefinition, HttpApiDefinitionRequest, Route, MethodPattern, AllPathPatterns, RouteRequest},
    };
    use golem_worker_service_base::gateway_binding::GatewayBinding;
    use golem_common::model::ComponentId;
    use serde::{Serialize, Deserialize};
    use tokio::runtime::Runtime;
    use tokio::time::sleep;
    use uuid::Uuid;
    use chrono::Utc;
    use utoipa::openapi::{
        self,
        OpenApi,
        Info,
        Paths,
        PathItem,
        Operation,
        Response,
        Content,
        Schema,
        SchemaType,
        ObjectBuilder,
    };

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
        // Create a test API definition request
        let routes = vec![
            // GET /users/{id}
            RouteRequest {
                method: MethodPattern::Get,
                path: AllPathPatterns::parse("/users/{id}").unwrap(),
                binding: GatewayBinding::Http {
                    url: "http://localhost:3000/users/${path.id}".to_string(),
                    method: "GET".to_string(),
                    headers: None,
                    body: None,
                },
                cors: None,
                security: None,
            },
            // POST /users
            RouteRequest {
                method: MethodPattern::Post,
                path: AllPathPatterns::parse("/users").unwrap(),
                binding: GatewayBinding::Http {
                    url: "http://localhost:3000/users".to_string(),
                    method: "POST".to_string(),
                    headers: None,
                    body: Some("${body}".to_string()),
                },
                cors: None,
                security: None,
            },
            // PUT /users/{id}
            RouteRequest {
                method: MethodPattern::Put,
                path: AllPathPatterns::parse("/users/{id}").unwrap(),
                binding: GatewayBinding::Http {
                    url: "http://localhost:3000/users/${path.id}".to_string(),
                    method: "PUT".to_string(),
                    headers: None,
                    body: Some("${body}".to_string()),
                },
                cors: None,
                security: None,
            },
            // DELETE /users/{id}
            RouteRequest {
                method: MethodPattern::Delete,
                path: AllPathPatterns::parse("/users/{id}").unwrap(),
                binding: GatewayBinding::Http {
                    url: "http://localhost:3000/users/${path.id}".to_string(),
                    method: "DELETE".to_string(),
                    headers: None,
                    body: None,
                },
                cors: None,
                security: None,
            },
        ];

        let request = HttpApiDefinitionRequest {
            id: ApiDefinitionId("test-api".to_string()),
            version: ApiVersion("1.0".to_string()),
            security: None,
            routes,
            draft: true,
        };

        // Create the API definition
        HttpApiDefinition {
            id: request.id,
            version: request.version,
            routes: request.routes.into_iter().map(Route::from).collect(),
            draft: request.draft,
            created_at: Utc::now(),
        }
    }

    fn generate_openapi_spec(api_def: &HttpApiDefinition) -> OpenApi {
        // Create OpenAPI document
        let mut paths = Paths::new();

        // Add user schema
        let user_schema = ObjectBuilder::new()
            .property("id", Schema::Integer(openapi::Integer::new()))
            .property("name", Schema::String(openapi::StringType::new()))
            .property("email", Schema::String(openapi::StringType::new()))
            .into_schema();

        // Add response schema
        let response_schema = ObjectBuilder::new()
            .property("status", Schema::String(openapi::StringType::new()))
            .property("data", Schema::Object(user_schema.clone()))
            .into_schema();

        // Add paths for each route
        for route in &api_def.routes {
            let path = route.path.to_string();
            let method = route.method.to_string().to_lowercase();

            let mut operation = Operation::new();
            operation.responses.insert(
                "200".to_string(),
                Response::new("Success")
                    .content("application/json", Content::new(response_schema.clone())),
            );

            let mut path_item = PathItem::new();
            match method.as_str() {
                "get" => path_item.get = Some(operation),
                "post" => path_item.post = Some(operation),
                "put" => path_item.put = Some(operation),
                "delete" => path_item.delete = Some(operation),
                _ => continue,
            }

            paths.paths.insert(path, path_item);
        }

        OpenApi {
            openapi: "3.0.0".to_string(),
            info: Info::new("Test API", api_def.version.0.as_str()),
            paths,
            ..Default::default()
        }
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

            // Generate OpenAPI spec
            let openapi = generate_openapi_spec(&api_def);
            let openapi_json = serde_json::to_string_pretty(&openapi).unwrap();
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

            // Generate OpenAPI spec
            let openapi = generate_openapi_spec(&api_def);
            let openapi_json = serde_json::to_string_pretty(&openapi).unwrap();
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

            // Generate OpenAPI spec
            let openapi = generate_openapi_spec(&api_def);
            let openapi_json = serde_json::to_string_pretty(&openapi).unwrap();
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

    #[test]
    fn test_openapi_export() {
        // Create a test API definition
        let api_def = HttpApiDefinition {
            id: ApiDefinitionId("test-api".to_string()),
            version: ApiVersion("1.0.0".to_string()),
            routes: vec![
                Route {
                    method: MethodPattern::GET,
                    path: AllPathPatterns::from_str("/users/{id}").unwrap(),
                    binding: None,
                    middlewares: None,
                },
                Route {
                    method: MethodPattern::POST,
                    path: AllPathPatterns::from_str("/users").unwrap(),
                    binding: None,
                    middlewares: None,
                },
                Route {
                    method: MethodPattern::PUT,
                    path: AllPathPatterns::from_str("/users/{id}").unwrap(),
                    binding: None,
                    middlewares: None,
                },
                Route {
                    method: MethodPattern::DELETE,
                    path: AllPathPatterns::from_str("/users/{id}").unwrap(),
                    binding: None,
                    middlewares: None,
                },
            ],
            draft: true,
            created_at: Utc::now(),
        };

        // Generate OpenAPI spec
        let openapi_spec = generate_openapi_spec(&api_def);

        // Verify OpenAPI spec structure
        assert_eq!(openapi_spec.openapi, "3.0.0");
        assert_eq!(openapi_spec.info.title, "Test API");
        assert_eq!(openapi_spec.info.version, api_def.version.0);

        // Verify paths
        let paths = openapi_spec.paths;
        assert!(paths.paths.contains_key("/users/{id}"));
        assert!(paths.paths.contains_key("/users"));

        // Verify methods
        let user_id_path = paths.paths.get("/users/{id}").unwrap();
        assert!(user_id_path.get.is_some());
        assert!(user_id_path.put.is_some());
        assert!(user_id_path.delete.is_some());

        let users_path = paths.paths.get("/users").unwrap();
        assert!(users_path.post.is_some());

        // Verify response schema
        let get_response = user_id_path.get.as_ref().unwrap().responses.get("200").unwrap();
        assert_eq!(get_response.description, "Success");
        assert!(get_response.content.contains_key("application/json"));

        let schema = get_response.content.get("application/json").unwrap().schema.as_ref().unwrap();
        match schema {
            Schema::Object(obj) => {
                assert!(obj.properties.contains_key("status"));
                assert!(obj.properties.contains_key("data"));
            }
            _ => panic!("Expected object schema"),
        }
    }
} 