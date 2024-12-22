use serde::{Deserialize, Serialize};
use crate::gateway_api_definition::http::openapi_export::OpenApiExporter;

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

    let openapi_url = OpenApiExporter::get_export_path(&config.api_id, &config.version);

    // Generate basic HTML with Swagger UI
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