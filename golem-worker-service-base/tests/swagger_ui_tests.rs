use anyhow::Result;
use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{create_swagger_ui, SwaggerUiConfig};
use poem_openapi::{payload::{Json, PlainText}, Object, ApiResponse};

test_r::enable!();

#[cfg(test)]
mod swagger_ui_tests {
    use super::*;

    #[test]
    fn test_swagger_ui_config_default() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SwaggerUiConfig::default();
            assert!(!config.enabled);
            assert_eq!(config.title, None);
            assert_eq!(config.version, None);
            assert_eq!(config.server_url, None);
            Ok(())
        })
    }

    #[test]
    fn test_swagger_ui_custom_config() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SwaggerUiConfig {
                enabled: true,
                title: Some("Custom API".to_string()),
                version: Some("1.0.0".to_string()),
                server_url: Some("http://localhost:8080".to_string()),
            };
            
            assert!(config.enabled);
            assert_eq!(config.title, Some("Custom API".to_string()));
            assert_eq!(config.version, Some("1.0.0".to_string()));
            assert_eq!(config.server_url, Some("http://localhost:8080".to_string()));
            Ok(())
        })
    }

    #[test]
    fn test_create_swagger_ui() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SwaggerUiConfig {
                enabled: true,
                title: Some("Test API".to_string()),
                version: Some("1.0".to_string()),
                server_url: Some("http://localhost:8080".to_string()),
            };

            // Note: We can't directly test the OpenApiService result since it's opaque
            // But we can verify it doesn't panic
            let _service = create_swagger_ui(MockApi, &config);
            Ok(())
        })
    }

    #[test]
    fn test_openapi_service_configuration() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SwaggerUiConfig {
                enabled: true,
                title: Some("Full Config API".to_string()),
                version: Some("1.0".to_string()),
                server_url: Some("http://localhost:8080".to_string()),
            };

            let service = create_swagger_ui(MockApi, &config)
                .summary("API Summary")
                .description("Detailed API description")
                .terms_of_service("https://example.com/terms");

            // Test available endpoint generation methods
            let _swagger_ui = service.swagger_ui();
            let _swagger_html = service.swagger_ui_html();
            let _spec_endpoint = service.spec_endpoint();
            let _spec_yaml = service.spec_endpoint_yaml();
            let spec_json = service.spec();

            // Verify some basic content in the OpenAPI spec
            assert!(spec_json.contains("Full Config API"));
            assert!(spec_json.contains("API Summary"));
            assert!(spec_json.contains("Detailed API description"));
            assert!(spec_json.contains("https://example.com/terms"));

            Ok(())
        })
    }

    #[test]
    fn test_api_responses() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = SwaggerUiConfig {
                enabled: true,
                title: Some("Test API".to_string()),
                version: Some("1.0".to_string()),
                server_url: None,
            };

            let api = MockApiWithResponses::new();
            let service = create_swagger_ui(api, &config);
            let spec = service.spec();

            // Verify response definitions in OpenAPI spec
            assert!(spec.contains("200")); // OK response
            assert!(spec.contains("201")); // Created response
            assert!(spec.contains("400")); // BadRequest response
            assert!(spec.contains("404")); // NotFound response

            Ok(())
        })
    }
}

// Mock API for testing
struct MockApi;

#[poem_openapi::OpenApi]
impl MockApi {
    #[oai(path = "/test", method = "get")]
    async fn test(&self) -> poem_openapi::payload::PlainText<String> {
        poem_openapi::payload::PlainText("test".to_string())
    }
}

// Mock API with various response types for testing
#[derive(ApiResponse)]
enum TestResponse {
    /// Successful response
    #[oai(status = 200)]
    OK(PlainText<String>),
    /// Resource created
    #[oai(status = 201)]
    Created,
    /// Bad request
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    /// Resource not found
    #[oai(status = 404)]
    NotFound(PlainText<String>),
}

#[derive(Object)]
struct TestObject {
    id: String,
    name: String,
}

struct MockApiWithResponses;

impl MockApiWithResponses {
    fn new() -> Self {
        Self
    }
}

#[poem_openapi::OpenApi]
impl MockApiWithResponses {
    /// Test endpoint with various response types
    #[oai(path = "/test", method = "post")]
    async fn test(&self, _data: Json<TestObject>) -> TestResponse {
        TestResponse::OK(PlainText("Success".to_string()))
    }

    /// Test endpoint for created response
    #[oai(path = "/test/create", method = "post")]
    async fn test_create(&self, _data: Json<TestObject>) -> TestResponse {
        TestResponse::Created
    }

    /// Test endpoint for bad request response
    #[oai(path = "/test/bad", method = "post")]
    async fn test_bad_request(&self, _data: Json<TestObject>) -> TestResponse {
        TestResponse::BadRequest(PlainText("Invalid request".to_string()))
    }

    /// Test endpoint for not found response
    #[oai(path = "/test/notfound", method = "get")]
    async fn test_not_found(&self) -> TestResponse {
        TestResponse::NotFound(PlainText("Resource not found".to_string()))
    }
} 