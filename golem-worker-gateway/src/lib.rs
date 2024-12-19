// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod openapi;

use axum::{
    Router,
    routing::{get, post},
};
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;

use golem_api_grpc::proto::golem::apidefinition::{
    ApiDefinition, GatewayBindingType, StaticBinding,
};
use openapi::{ApiDefinitionConverter, OpenApiError, SwaggerUiHandler};

/// Configure Swagger UI for an API Definition
pub async fn configure_swagger_ui(
    app: &mut Router,
    api: &ApiDefinition,
    path: &str,
    cors_allowed_origins: Option<String>,
) -> Result<(), OpenApiError> {
    let converter = ApiDefinitionConverter::new();
    let openapi = converter.convert(api)?;
    let spec = serde_json::to_string(&openapi)?;
    
    let handler = Arc::new(SwaggerUiHandler::new(
        spec,
        None, // custom CSS URL
        None, // custom JS URL
        cors_allowed_origins,
    ));

    let mut swagger_router = Router::new()
        .route("/", get(SwaggerUiHandler::serve_ui))
        .route("/spec", get(SwaggerUiHandler::serve_spec))
        .with_state(handler);

    if let Some(origins) = cors_allowed_origins {
        let cors = CorsLayer::new()
            .allow_origin(origins.parse().unwrap())
            .allow_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
            .allow_headers(vec!["content-type"]);

        swagger_router = swagger_router.layer(ServiceBuilder::new().layer(cors));
    }

    app.nest(path, swagger_router);
    Ok(())
}

/// Configure a binding for an API Definition
pub async fn configure_binding(
    app: &mut Router,
    binding: &StaticBinding,
) -> Result<(), OpenApiError> {
    match binding.binding_type() {
        GatewayBindingType::SwaggerUi => {
            if let Some(swagger_ui) = &binding.swagger_ui {
                configure_swagger_ui(
                    app,
                    &binding.api_definition.clone().unwrap_or_default(),
                    &binding.path,
                    Some(swagger_ui.cors_allowed_origins.clone()),
                ).await?;
            }
        }
        _ => {}
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
    async fn test_swagger_ui_configuration() {
        let api = ApiDefinition {
            id: None,
            version: "1.0.0".to_string(),
            http_api: None,
            ..Default::default()
        };

        let mut app = Router::new();
        configure_swagger_ui(&mut app, &api, "/docs", Some("*".to_string()))
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/docs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
