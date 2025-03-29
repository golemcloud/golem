// Copyright 2024-2025 Golem Cloud
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

use crate::api::api_definition::HttpApiDefinitionResponseData;
use crate::gateway_api_definition::http::oas_api_definition::OpenApiHttpApiDefinition;
use crate::gateway_api_deployment::ApiSiteString;
use crate::gateway_execution::api_definition_lookup::HttpApiDefinitionsLookup;
use crate::gateway_execution::request::RichRequest;
use crate::service::gateway::api_definition::ApiDefinitionService;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_service_base::auth::EmptyAuthCtx;
use http::StatusCode;
use openapiv3;
use poem::Body;
use tracing::error;

const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

#[async_trait]
pub trait SwaggerBindingHandler<Namespace> {
    async fn handle_swagger_binding_request(
        &self,
        namespace: &Namespace,
        authority: &str,
        request: &RichRequest,
    ) -> SwaggerBindingResult;
}

pub type SwaggerBindingResult = Result<SwaggerBindingSuccess, SwaggerBindingError>;

pub struct SwaggerBindingSuccess {
    pub html_content: String,
}

pub enum SwaggerBindingError {
    NotFound(String),
    InternalError(String),
}

impl From<SwaggerBindingError> for poem::Response {
    fn from(value: SwaggerBindingError) -> Self {
        match value {
            SwaggerBindingError::NotFound(message) => poem::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from_string(message)),
            SwaggerBindingError::InternalError(message) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(message)),
        }
    }
}

pub struct DefaultSwaggerBindingHandler<Namespace> {
    pub api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup<Namespace> + Sync + Send>,
    pub definition_service:
        Option<Arc<dyn ApiDefinitionService<EmptyAuthCtx, Namespace> + Sync + Send>>,
}

use std::sync::Arc;

impl<Namespace> DefaultSwaggerBindingHandler<Namespace> {
    pub fn new(
        api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup<Namespace> + Sync + Send>,
        definition_service: Option<
            Arc<dyn ApiDefinitionService<EmptyAuthCtx, Namespace> + Sync + Send>,
        >,
    ) -> Self {
        Self {
            api_definition_lookup_service,
            definition_service,
        }
    }
}

#[async_trait]
impl<Namespace: Send + Sync + Clone + 'static> SwaggerBindingHandler<Namespace>
    for DefaultSwaggerBindingHandler<Namespace>
{
    async fn handle_swagger_binding_request(
        &self,
        namespace: &Namespace,
        authority: &str,
        request: &RichRequest,
    ) -> SwaggerBindingResult {
        // Get the request path
        let request_path = request.underlying.uri().path().to_string();

        // Get the API definitions
        let api_definitions = match self
            .api_definition_lookup_service
            .get(&ApiSiteString(authority.to_string()))
            .await
        {
            Ok(defs) => defs,
            Err(api_defs_lookup_error) => {
                error!(
                    "API request host: {} - error: {}",
                    authority,
                    api_defs_lookup_error.to_safe_string()
                );
                return Err(SwaggerBindingError::InternalError(
                    api_defs_lookup_error.to_safe_string(),
                ));
            }
        };

        // Find the API definition that matches the Swagger UI path
        let api_def = api_definitions.iter().find(|def| {
            def.routes.iter().any(|route| {
                route.path.to_string() == request_path
                    && matches!(
                        route.binding,
                        crate::gateway_binding::GatewayBindingCompiled::SwaggerUi
                    )
            })
        });

        let api_def = match api_def {
            Some(def) => def,
            None => {
                return Err(SwaggerBindingError::NotFound(
                    "Swagger UI cannot have variable path".to_string(),
                ))
            }
        };

        // Convert to HttpApiDefinitionResponseData using the definition_service
        let definition_service = match &self.definition_service {
            Some(service) => service,
            None => {
                return Err(SwaggerBindingError::InternalError(
                    "Definition service not available".to_string(),
                ))
            }
        };

        let response_data = match definition_service
            .get(
                &api_def.id,
                &api_def.version,
                namespace,
                &EmptyAuthCtx::default(),
            )
            .await
        {
            Ok(Some(compiled_def)) => {
                match HttpApiDefinitionResponseData::from_compiled_http_api_definition(
                    compiled_def,
                    &definition_service.conversion_context(&EmptyAuthCtx::default()),
                )
                .await
                {
                    Ok(response_data) => response_data,
                    Err(e) => {
                        return Err(SwaggerBindingError::InternalError(format!(
                            "Error converting to response data: {}",
                            e
                        )))
                    }
                }
            }
            Ok(None) => {
                return Err(SwaggerBindingError::NotFound(
                    "API definition not found".to_string(),
                ))
            }
            Err(e) => {
                return Err(SwaggerBindingError::InternalError(format!(
                    "Error getting API definition: {}",
                    e
                )))
            }
        };

        // Convert to OpenAPI spec
        let openapi_req = match OpenApiHttpApiDefinition::from_http_api_definition_response_data(
            &response_data,
        ) {
            Ok(req) => req,
            Err(e) => {
                return Err(SwaggerBindingError::InternalError(format!(
                    "Error converting to OpenAPI: {}",
                    e
                )))
            }
        };

        // Add server information
        let mut spec = openapi_req.0;
        if spec.servers.is_empty() {
            spec.servers = Vec::new();
        }
        spec.servers.push(openapiv3::Server {
            url: format!("http://{}", authority),
            description: Some("Local Development Server".to_string()),
            variables: Default::default(),
            extensions: Default::default(),
        });

        // First collect paths to remove
        let mut paths_to_remove = Vec::new();

        // Filter paths - remove non-default binding types
        for (path, path_item) in spec.paths.paths.iter_mut() {
            if let openapiv3::ReferenceOr::Item(path_item) = path_item {
                // Create a vector of mutable references to all operations
                let operations = vec![
                    &mut path_item.get,
                    &mut path_item.post,
                    &mut path_item.put,
                    &mut path_item.delete,
                    &mut path_item.patch,
                    &mut path_item.options,
                ];

                // Process each operation
                for operation in operations {
                    if let Some(op) = operation {
                        if let Some(binding) = op.extensions.get(GOLEM_API_GATEWAY_BINDING) {
                            if let Some(binding_type) = binding.get("binding-type") {
                                if binding_type != "default" {
                                    // Set the operation to None by taking the value
                                    *operation = None;
                                }
                            }
                        }
                    }
                }

                // Check if the path has no operations left
                if path_item.get.is_none()
                    && path_item.post.is_none()
                    && path_item.put.is_none()
                    && path_item.delete.is_none()
                    && path_item.patch.is_none()
                    && path_item.options.is_none()
                {
                    // Mark this path for removal
                    paths_to_remove.push(path.clone());
                }
            }
        }

        // Remove empty paths from the spec
        for path in paths_to_remove {
            spec.paths.paths.shift_remove(&path);
        }

        // Convert to JSON
        let spec_json = match serde_json::to_string_pretty(&spec) {
            Ok(json) => json,
            Err(e) => {
                error!("Failed to serialize OpenAPI spec: {:?}", e);
                return Err(SwaggerBindingError::InternalError(
                    "Failed to serialize OpenAPI spec".to_string(),
                ));
            }
        };

        // Serve the Swagger UI HTML with the JSON embedded
        let html = format!(
            r#"
            <!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <title>Swagger UI</title>
                <link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
                <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
            </head>
            <body>
                <div id="swagger-ui"></div>
                <script>
                    window.onload = () => {{
                        const spec = {};
                        window.ui = SwaggerUIBundle({{
                            spec: spec,
                            dom_id: '#swagger-ui',
                            deepLinking: true,
                            presets: [
                                SwaggerUIBundle.presets.apis,
                                SwaggerUIBundle.SwaggerUIStandalonePreset
                            ],
                        }});
                    }};
                </script>
            </body>
            </html>
        "#,
            spec_json
        );

        Ok(SwaggerBindingSuccess { html_content: html })
    }
}
