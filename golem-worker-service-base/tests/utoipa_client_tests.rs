#[cfg(test)]
mod utoipa_client_tests {
    use axum::{
        routing::{get, post},
        Router, Json,
        extract::Path,
        response::IntoResponse,
    };
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tower::ServiceBuilder;
    use tower_http::trace::TraceLayer;
    use utoipa::{OpenApi, ToSchema, Modify, openapi::{self, security::{SecurityScheme, ApiKey, ApiKeyValue}}};
    use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{SwaggerUiConfig, generate_swagger_ui};
    use http::header;
    use reqwest::header::{HeaderMap as ReqHeaderMap, HeaderValue as ReqHeaderValue};

    // Complex types for our API
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    struct CreateWorkflowRequest {
        #[schema(example = "My Workflow")]
        name: String,
        #[schema(example = json!(["task1", "task2"]))]
        tasks: Vec<String>,
        #[schema(example = json!({
            "retry_count": 3,
            "timeout_seconds": 300
        }))]
        config: WorkflowConfig,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    struct WorkflowConfig {
        #[schema(example = 3)]
        retry_count: u32,
        #[schema(example = 300)]
        timeout_seconds: u32,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    struct WorkflowResponse {
        #[schema(example = "wf-123")]
        id: String,
        #[schema(example = "My Workflow")]
        name: String,
        #[schema(example = "RUNNING")]
        status: WorkflowStatus,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    #[serde(rename_all = "UPPERCASE")]
    enum WorkflowStatus {
        Created,
        Running,
        Completed,
        Failed,
    }

    // API handlers
    /// Create a new workflow
    #[utoipa::path(
        post,
        path = "/api/v1/workflows",
        request_body = CreateWorkflowRequest,
        responses(
            (status = 201, description = "Workflow created successfully", body = WorkflowResponse),
            (status = 400, description = "Invalid workflow configuration")
        ),
        security(
            ("api_key" = [])
        )
    )]
    async fn create_workflow(
        Json(request): Json<CreateWorkflowRequest>,
    ) -> Json<WorkflowResponse> {
        Json(WorkflowResponse {
            id: "wf-123".to_string(),
            name: request.name,
            status: WorkflowStatus::Created,
        })
    }

    /// Get workflow by ID
    #[utoipa::path(
        get,
        path = "/api/v1/workflows/{id}",
        responses(
            (status = 200, description = "Workflow found", body = WorkflowResponse),
            (status = 404, description = "Workflow not found")
        ),
        params(
            ("id" = String, Path, description = "Workflow ID")
        ),
        security(
            ("api_key" = [])
        )
    )]
    async fn get_workflow(
        Path(id): Path<String>,
    ) -> Json<WorkflowResponse> {
        Json(WorkflowResponse {
            id,
            name: "Test Workflow".to_string(),
            status: WorkflowStatus::Running,
        })
    }

    struct SecurityAddon;

    impl Modify for SecurityAddon {
        fn modify(&self, openapi: &mut openapi::OpenApi) {
            let components = openapi.components.get_or_insert_with(Default::default);
            let api_key_value = ApiKeyValue::new("x-api-key");
            components.add_security_scheme(
                "api_key",
                SecurityScheme::ApiKey(ApiKey::Header(api_key_value))
            );
        }
    }

    // OpenAPI documentation
    #[derive(OpenApi)]
    #[openapi(
        paths(
            create_workflow,
            get_workflow
        ),
        components(
            schemas(
                CreateWorkflowRequest,
                WorkflowConfig,
                WorkflowResponse,
                WorkflowStatus
            )
        ),
        modifiers(&SecurityAddon),
        tags(
            (name = "workflows", description = "Workflow management endpoints")
        ),
        info(
            title = "Workflow API",
            version = "1.0.0",
            description = "API for managing workflow executions"
        )
    )]
    struct ApiDoc;

    // Serve Swagger UI
    async fn serve_swagger_ui() -> impl IntoResponse {
        let config = SwaggerUiConfig {
            enabled: true,
            path: "/docs".to_string(),
            title: Some("Workflow API".to_string()),
            theme: Some("dark".to_string()),
            api_id: "workflow-api".to_string(),
            version: "1.0.0".to_string(),
        };

        let html = generate_swagger_ui(&config);
        
        (
            [(header::CONTENT_TYPE, "text/html")],
            html
        )
    }

    // Serve OpenAPI spec
    async fn serve_openapi() -> impl IntoResponse {
        let doc = ApiDoc::openapi();
        Json(doc)
    }

    async fn setup_test_server() -> SocketAddr {
        let app = Router::new()
            .route("/api/v1/workflows", post(create_workflow))
            .route("/api/v1/workflows/:id", get(get_workflow))
            .route("/docs", get(serve_swagger_ui))
            .route("/api-docs/openapi.json", get(serve_openapi))
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        addr
    }

    #[tokio::test]
    async fn test_workflow_api_with_swagger_ui() {
        // Start the test server
        let addr = setup_test_server().await;
        let base_url = format!("http://{}", addr);

        // Create headers with API key
        let mut headers = ReqHeaderMap::new();
        headers.insert("x-api-key", ReqHeaderValue::from_static("test-key"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();

        // Test Swagger UI endpoint
        let swagger_ui_response = client
            .get(format!("{}/docs", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(swagger_ui_response.status(), 200);
        let html = swagger_ui_response.text().await.unwrap();
        assert!(html.contains("swagger-ui"));
        assert!(html.contains("Workflow API"));

        // Test OpenAPI spec endpoint
        let docs_response = client
            .get(format!("{}/api-docs/openapi.json", base_url))
            .send()
            .await
            .unwrap();

        assert_eq!(docs_response.status(), 200);
        let api_docs: serde_json::Value = docs_response.json().await.unwrap();
        
        // Verify key components of the OpenAPI spec
        assert_eq!(api_docs["info"]["title"], "Workflow API");
        assert_eq!(api_docs["info"]["version"], "1.0.0");
        
        // Test API endpoints
        let create_request = CreateWorkflowRequest {
            name: "Test Workflow".to_string(),
            tasks: vec!["task1".to_string(), "task2".to_string()],
            config: WorkflowConfig {
                retry_count: 3,
                timeout_seconds: 300,
            },
        };

        let response = client
            .post(format!("{}/api/v1/workflows", base_url))
            .json(&create_request)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let workflow: WorkflowResponse = response.json().await.unwrap();
        assert_eq!(workflow.name, "Test Workflow");
        assert!(matches!(workflow.status, WorkflowStatus::Created));

        // Test getting the workflow
        let response = client
            .get(format!("{}/api/v1/workflows/{}", base_url, workflow.id))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let workflow: WorkflowResponse = response.json().await.unwrap();
        assert!(matches!(workflow.status, WorkflowStatus::Running));
    }
} 