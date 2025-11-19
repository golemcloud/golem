// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_service_base::custom_api::compiled_gateway_binding::SwaggerUiBinding;
use http::StatusCode;
use indexmap::IndexMap;
use openapiv3;
use poem::Body;

const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

pub struct SwaggerHtml(pub String);

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

/// Handles a Swagger binding request by:
/// 1. Getting the OpenAPI spec from the SwaggerUI binding
/// 2. Filtering and modifying the OpenAPI spec
/// 3. Generating and returning Swagger UI HTML
pub async fn handle_swagger_binding_request(
    authority: &str,
    swagger_binding: &SwaggerUiBinding,
) -> Result<SwaggerHtml, SwaggerBindingError> {
    // Get the OpenAPI spec from the SwaggerUI binding
    let spec_json = swagger_binding.openapi_spec_json.as_ref().ok_or_else(|| {
        SwaggerBindingError::InternalError(
            "OpenAPI spec not found in SwaggerUI binding".to_string(),
        )
    })?;

    // Parse and modify the OpenAPI spec
    let mut spec: openapiv3::OpenAPI = serde_json::from_str(spec_json).map_err(|e| {
        SwaggerBindingError::InternalError(format!("Failed to parse OpenAPI spec: {e}"))
    })?;

    // Add server information to the OpenAPI spec
    if spec.servers.is_empty() {
        spec.servers = Vec::new();
    }
    spec.servers.push(openapiv3::Server {
        url: format!("http://{authority}"),
        description: Some("Local Development Server".to_string()),
        variables: Default::default(),
        extensions: Default::default(),
    });

    // Filter paths based on binding types
    let paths_to_remove = filter_paths(&mut spec.paths.paths);
    for path in paths_to_remove {
        spec.paths.paths.shift_remove(&path);
    }

    // Convert back to JSON
    let modified_spec_json = serde_json::to_string(&spec).map_err(|e| {
        SwaggerBindingError::InternalError(format!("Failed to serialize OpenAPI spec: {e}"))
    })?;

    // Generate Swagger UI HTML
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>Swagger UI</title>
<link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
<script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
<script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
</head>
<body>
<div id="swagger-ui"></div>
<script>
    const spec = {modified_spec_json};
    window.onload = () => {{
        window.ui = SwaggerUIBundle({{
            spec: spec,
            dom_id: '#swagger-ui',
            deepLinking: true,
            presets: [
                SwaggerUIBundle.presets.apis,
                SwaggerUIStandalonePreset
            ],
            plugins: [
                SwaggerUIBundle.plugins.DownloadUrl
            ]
        }});
    }};
</script>
</body>
</html>"#
    );

    Ok(SwaggerHtml(html))
}

// Filters paths in the OpenAPI specification based on gateway binding types.
fn filter_paths(
    paths: &mut IndexMap<String, openapiv3::ReferenceOr<openapiv3::PathItem>>,
) -> Vec<String> {
    let mut paths_to_remove = Vec::new();

    for (path, path_item) in paths.iter_mut() {
        if let openapiv3::ReferenceOr::Item(path_item) = path_item {
            // Check all possible HTTP operations
            let operations = vec![
                &mut path_item.get,
                &mut path_item.post,
                &mut path_item.put,
                &mut path_item.delete,
                &mut path_item.patch,
                &mut path_item.options,
            ];

            // Filter operations based on binding type
            for operation in operations {
                if let Some(op) = operation {
                    if let Some(binding) = op.extensions.get(GOLEM_API_GATEWAY_BINDING) {
                        if let Some(binding_type) = binding.get("binding-type") {
                            if binding_type != "default" {
                                *operation = None;
                            }
                        }
                    }
                }
            }

            // If all operations are removed, mark the path for removal
            if path_item.get.is_none()
                && path_item.post.is_none()
                && path_item.put.is_none()
                && path_item.delete.is_none()
                && path_item.patch.is_none()
                && path_item.options.is_none()
            {
                paths_to_remove.push(path.clone());
            }
        }
    }

    paths_to_remove
}
