use std::sync::Arc;
use axum::{
    Router,
    routing::{get, post},
    extract::State,
};
use golem_api_grpc::proto::golem::apidefinition::{
    ApiDefinition, GatewayBindingType, StaticBinding,
};
use crate::{
    openapi::{ApiDefinitionConverter, OpenApiError, SwaggerUiHandler, SwaggerUiConfig},
    error::Result,
};

#[derive(Debug, Clone)]
pub enum BindingType {
    Default(RibScript),
    FileServer(RibScript),
    SwaggerUI(SwaggerUIBinding),
}

#[derive(Debug, Clone)]
pub struct SwaggerUIBinding {
    pub config: SwaggerUiConfig,
    pub cors_allowed_origins: Option<String>,
}

impl SwaggerUIBinding {
    pub fn new(config: SwaggerUiConfig, cors_allowed_origins: Option<String>) -> Self {
        Self {
            config,
            cors_allowed_origins,
        }
    }
}

impl TryFrom<&StaticBinding> for BindingType {
    type Error = OpenApiError;

    fn try_from(binding: &StaticBinding) -> Result<Self, Self::Error> {
        match binding.binding_type() {
            GatewayBindingType::Default => {
                let script = RibScript::parse(&binding.rib_script)?;
                Ok(BindingType::Default(script))
            }
            GatewayBindingType::FileServer => {
                let script = RibScript::parse(&binding.rib_script)?;
                Ok(BindingType::FileServer(script))
            }
            GatewayBindingType::SwaggerUi => {
                let config = if let Some(config_str) = &binding.swagger_ui_config {
                    serde_json::from_str(config_str)?
                } else {
                    SwaggerUiConfig::default()
                };
                
                Ok(BindingType::SwaggerUI(SwaggerUIBinding::new(
                    config,
                    binding.cors_allowed_origins.clone(),
                )))
            }
        }
    }
}

pub async fn configure_binding(
    app: &mut Router,
    binding: &StaticBinding,
    api: &ApiDefinition,
) -> Result<(), OpenApiError> {
    let binding_type = BindingType::try_from(binding)?;
    
    match binding_type {
        BindingType::Default(script) => {
            // Configure default binding
            configure_default_binding(app, script, binding.path())?;
        }
        BindingType::FileServer(script) => {
            // Configure file server binding
            configure_file_server_binding(app, script, binding.path())?;
        }
        BindingType::SwaggerUI(swagger_ui) => {
            // Convert API Definition to OpenAPI spec
            let converter = ApiDefinitionConverter::new();
            let openapi = converter.convert(api)?;
            let spec = serde_json::to_string_pretty(&openapi)?;
            
            // Create Swagger UI handler
            let handler = Arc::new(SwaggerUiHandler::new(
                spec,
                Some(swagger_ui.config),
                None,
                None,
                swagger_ui.cors_allowed_origins,
            ));

            // Configure routes
            let swagger_router = Router::new()
                .route("/", get(SwaggerUiHandler::serve_ui))
                .route("/spec", get(SwaggerUiHandler::serve_spec))
                .with_state(handler);

            app.nest(binding.path(), swagger_router);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_swagger_ui_binding() -> Result<()> {
        let api = ApiDefinition {
            id: Some(ApiDefinitionId {
                value: "test-api".to_string(),
            }),
            version: "1.0.0".to_string(),
            http_api: Some(CompiledHttpApiDefinition {
                routes: vec![
                    CompiledHttpRoute {
                        method: 0,
                        path: "/api/test".to_string(),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };

        let binding = StaticBinding {
            path: "/docs".to_string(),
            binding_type: GatewayBindingType::SwaggerUi as i32,
            swagger_ui_config: Some(serde_json::to_string(&SwaggerUiConfig {
                title: "Test API".to_string(),
                description: Some("Test Description".to_string()),
                version: "1.0.0".to_string(),
                theme: SwaggerUiTheme::Dark,
                ..Default::default()
            })?),
            cors_allowed_origins: Some("*".to_string()),
            ..Default::default()
        };

        let mut app = Router::new();
        configure_binding(&mut app, &binding, &api).await?;

        // Test Swagger UI endpoint
        let response = app
            .oneshot(Request::builder().uri("/docs").body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);

        // Test OpenAPI spec endpoint
        let response = app
            .oneshot(Request::builder().uri("/docs/spec").body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);

        Ok(())
    }
}
