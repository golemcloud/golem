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

#[cfg(test)]
mod tests {
    #[test]
    fn test_swagger_ui_config_default() {
        let config = super::SwaggerUiConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.path, "/docs");
        assert_eq!(config.title, None);
        assert_eq!(config.theme, None);
        assert_eq!(config.api_id, "default");
        assert_eq!(config.version, "1.0");
    }

    #[test]
    fn test_swagger_ui_generation() {
        let config = super::SwaggerUiConfig {
            enabled: true,
            path: "/custom/docs".to_string(),
            title: Some("Custom API".to_string()),
            theme: Some("dark".to_string()),
            api_id: "test-api".to_string(),
            version: "1.0.0".to_string(),
        };
        let html = super::generate_swagger_ui(&config);
        
        // Verify HTML structure
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<meta charset=\"utf-8\" />"));
        assert!(html.contains("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />"));
        
        // Verify title configuration
        assert!(html.contains("<title>Custom API</title>"));
        
        // Verify OpenAPI URL generation and usage
        let expected_url = super::OpenApiExporter::get_export_path("test-api", "1.0.0");
        assert!(html.contains(&format!(r#"url: '{}'"#, expected_url)));
        
        // Verify theme configuration
        assert!(html.contains("background-color: #1a1a1a"));
        assert!(html.contains("filter: invert(88%) hue-rotate(180deg)"));
        assert!(html.contains(r#"syntaxHighlight: { theme: "monokai" }"#));
        
        // Verify SwaggerUI configuration
        assert!(html.contains("deepLinking: true"));
        assert!(html.contains("layout: \"BaseLayout\""));
        assert!(html.contains("SwaggerUIBundle.presets.apis"));
        assert!(html.contains("SwaggerUIBundle.SwaggerUIStandalonePreset"));
    }

    #[test]
    fn test_swagger_ui_default_title() {
        let config = super::SwaggerUiConfig {
            enabled: true,
            title: None,
            ..super::SwaggerUiConfig::default()
        };
        
        let html = super::generate_swagger_ui(&config);
        assert!(html.contains("<title>API Documentation</title>"));
    }

    #[test]
    fn test_swagger_ui_theme_variants() {
        // Test light theme (None)
        let light_config = super::SwaggerUiConfig {
            enabled: true,
            theme: None,
            ..super::SwaggerUiConfig::default()
        };
        let light_html = super::generate_swagger_ui(&light_config);
        assert!(!light_html.contains("background-color: #1a1a1a"));
        assert!(!light_html.contains("filter: invert(88%) hue-rotate(180deg)"));
        assert!(!light_html.contains(r#"syntaxHighlight: { theme: "monokai" }"#));
        
        // Test dark theme
        let dark_config = super::SwaggerUiConfig {
            enabled: true,
            theme: Some("dark".to_string()),
            ..super::SwaggerUiConfig::default()
        };
        let dark_html = super::generate_swagger_ui(&dark_config);
        assert!(dark_html.contains("background-color: #1a1a1a"));
        assert!(dark_html.contains("filter: invert(88%) hue-rotate(180deg)"));
        assert!(dark_html.contains(r#"syntaxHighlight: { theme: "monokai" }"#));
    }

    #[test]
    fn test_swagger_ui_disabled() {
        let config = super::SwaggerUiConfig {
            enabled: false,
            ..super::SwaggerUiConfig::default()
        };
        assert_eq!(super::generate_swagger_ui(&config), String::new());
    }
}
