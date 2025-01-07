use anyhow::Result;
use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{create_swagger_ui, SwaggerUiConfig, SwaggerUiAuthConfig};
use serde::{Serialize, Deserialize};

#[cfg(test)]
mod utoipa_client_tests {
    use super::*;
    use poem::{
        Route,
        test::TestClient,
        web::Path,
    };
    use poem_openapi::{
        OpenApi, 
        Object,
        payload::Json as PoemJson,
        OpenApiService,
        Enum,
    };

    // Complex types for our API
    #[derive(Debug, Object, Clone, Serialize, Deserialize)]
    struct CreateWorkflowRequest {
        name: String,
        tasks: Vec<String>,
        config: WorkflowConfig,
    }

    #[derive(Debug, Object, Clone, Serialize, Deserialize)]
    struct WorkflowConfig {
        retry_count: u32,
        timeout_seconds: u32,
    }

    #[derive(Debug, Object, Clone, Serialize, Deserialize)]
    struct WorkflowResponse {
        id: String,
        name: String,
        status: WorkflowStatus,
    }

    #[derive(Debug, Clone, Enum, Serialize, Deserialize)]
    #[oai(rename = "WorkflowStatus")]
    enum WorkflowStatus {
        Created,
        Running,
        Completed,
        Failed,
    }

    #[derive(Clone)]
    struct TestApi;

    #[OpenApi]
    impl TestApi {
        /// Create a new workflow
        #[oai(path = "/api/v1/workflows", method = "post")]
        async fn create_workflow(
            &self,
            request: PoemJson<CreateWorkflowRequest>,
        ) -> poem::Result<PoemJson<WorkflowResponse>> {
            Ok(PoemJson(WorkflowResponse {
                id: "wf-123".to_string(),
                name: request.0.name,
                status: WorkflowStatus::Created,
            }))
        }

        /// Get workflow by ID
        #[oai(path = "/api/v1/workflows/:id", method = "get")]
        async fn get_workflow(
            &self,
            id: Path<String>,
        ) -> poem::Result<PoemJson<WorkflowResponse>> {
            Ok(PoemJson(WorkflowResponse {
                id: id.0,
                name: "Test Workflow".to_string(),
                status: WorkflowStatus::Running,
            }))
        }
    }

    #[tokio::test]
    async fn test_workflow_api_with_swagger_ui() -> Result<()> {
        let swagger_config = SwaggerUiConfig {
            enabled: true,
            title: Some("Workflow API".to_string()),
            version: Some("1.0.0".to_string()),
            server_url: Some("http://localhost:3000".to_string()),
            auth: SwaggerUiAuthConfig::default(),
            worker_binding: None,
            golem_extensions: std::collections::HashMap::new(),
        };

        let api_service = OpenApiService::new(TestApi, "Workflow API", "1.0.0")
            .server("http://localhost:3000");
        let swagger_ui = create_swagger_ui(TestApi, &swagger_config);

        let app = Route::new()
            .nest("/", api_service.clone())
            .nest("/docs", swagger_ui.swagger_ui())
            .nest("/api-docs", api_service.spec_endpoint());

        let cli = TestClient::new(app);

        // Test Swagger UI endpoint
        let swagger_ui_resp = cli
            .get("/docs")
            .header("x-api-key", "test-key")
            .send()
            .await;
        
        assert_eq!(swagger_ui_resp.0.status().as_u16(), 200);
        let html = String::from_utf8(swagger_ui_resp.0.into_body().into_bytes().await.unwrap().to_vec())?;
        assert!(html.contains("swagger-ui"));
        assert!(html.contains("Workflow API"));

        // Test OpenAPI spec endpoint
        let docs_resp = cli
            .get("/api-docs/openapi.json")
            .header("x-api-key", "test-key")
            .send()
            .await;

        assert_eq!(docs_resp.0.status().as_u16(), 200);

        // Also test the actual API endpoints
        let create_resp = cli
            .post("/api/v1/workflows")
            .header("x-api-key", "test-key")
            .body_json(&CreateWorkflowRequest {
                name: "test workflow".to_string(),
                tasks: vec!["task1".to_string()],
                config: WorkflowConfig {
                    retry_count: 3,
                    timeout_seconds: 60,
                },
            })
            .send()
            .await;
        
        assert_eq!(create_resp.0.status().as_u16(), 200);

        let get_resp = cli
            .get("/api/v1/workflows/wf-123")
            .header("x-api-key", "test-key")
            .send()
            .await;
        
        assert_eq!(get_resp.0.status().as_u16(), 200);

        Ok(())
    }
} 