// swagger_ui.rs
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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUiConfig {
    pub enabled: bool,
    pub path: String,
    pub title: Option<String>,
    pub theme: Option<String>,
    pub api_id: String,
    pub version: String,
}

impl Default for SwaggerUiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "/docs".to_string(),
            title: None,
            theme: None,
            api_id: "default".to_string(),
            version: "1.0".to_string(),
        }
    }
}

/// Generates Swagger UI HTML content for a given OpenAPI spec URL
pub fn generate_swagger_ui(config: &SwaggerUiConfig) -> String {
    if !config.enabled {
        return String::new();
    }

    let openapi_url = format!("/v1/api/definitions/{}/version/{}/export", config.api_id, config.version);

    format!(
        r#"
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="utf-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1" />
            <title>{}</title>
            <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@latest/swagger-ui.css" />
            <style>
                {}
            </style>
        </head>
        <body>
            <div id="swagger-ui"></div>
            <script src="https://unpkg.com/swagger-ui-dist@latest/swagger-ui-bundle.js"></script>
            <script>
                window.onload = () => {{
                    window.ui = SwaggerUIBundle({{
                        url: '{}',
                        dom_id: '#swagger-ui',
                        deepLinking: true,
                        presets: [
                            SwaggerUIBundle.presets.apis,
                            SwaggerUIBundle.SwaggerUIStandalonePreset
                        ],
                        layout: "BaseLayout",
                        {} // Theme configuration
                    }});
                }};
            </script>
        </body>
        </html>
        "#,
        config.title.as_deref().unwrap_or("API Documentation"),
        if config.theme.as_deref() == Some("dark") {
            r#"
            body {
                background-color: #1a1a1a;
                color: #ffffff;
            }
            .swagger-ui {
                filter: invert(88%) hue-rotate(180deg);
            }
            .swagger-ui .topbar {
                background-color: #1a1a1a;
            }
            "#
        } else {
            ""
        },
        openapi_url,
        if config.theme.as_deref() == Some("dark") {
            r#"syntaxHighlight: { theme: "monokai" }"#
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_swagger_ui_enabled() {
        let config = SwaggerUiConfig {
            enabled: true,
            path: "/docs".to_string(),
            title: Some("Test API Docs".to_string()),
            theme: Some("dark".to_string()),
            api_id: "test_api".to_string(),
            version: "1.0".to_string(),
        };

        let html = generate_swagger_ui(&config);
        assert!(html.contains("Test API Docs"));
        assert!(html.contains("swagger-ui"));
    }

    #[test]
    fn test_generate_swagger_ui_disabled() {
        let config = SwaggerUiConfig {
            enabled: false,
            path: "/docs".to_string(),
            ..Default::default()
        };

        let html = generate_swagger_ui(&config);
        assert!(html.is_empty());
    }
}

