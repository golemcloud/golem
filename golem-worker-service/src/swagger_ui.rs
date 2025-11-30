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

use crate::model::SwaggerHtml;
use golem_service_base::custom_api::openapi::HttpApiDefinitionOpenApiSpec;

pub fn generate_swagger_html(
    authority: &str,
    open_api_spec: HttpApiDefinitionOpenApiSpec,
) -> Result<SwaggerHtml, String> {
    let mut spec = open_api_spec.0;

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

    // Convert back to JSON
    let modified_spec_json = serde_json::to_string(&spec)
        .map_err(|e| format!("Failed to serialize OpenAPI spec: {e}"))?;

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
