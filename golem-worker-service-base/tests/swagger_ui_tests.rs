use golem_worker_service_base::gateway_api_definition::http::swagger_ui::{SwaggerUiConfig, generate_swagger_ui};
use golem_worker_service_base::gateway_api_definition::http::openapi_export::OpenApiExporter;

#[test]
fn test_swagger_ui_config_default() {
    let config = SwaggerUiConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.path, "/docs");
    assert_eq!(config.title, None);
    assert_eq!(config.theme, None);
    assert_eq!(config.api_id, "default");
    assert_eq!(config.version, "1.0");
}

#[test]
fn test_swagger_ui_generation() {
    let config = SwaggerUiConfig {
        enabled: true,
        path: "/custom/docs".to_string(),
        title: Some("Custom API".to_string()),
        theme: Some("dark".to_string()),
        api_id: "test-api".to_string(),
        version: "1.0.0".to_string(),
    };
    let html = generate_swagger_ui(&config);
    
    // Verify HTML structure
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("<html lang=\"en\">"));
    assert!(html.contains("<meta charset=\"utf-8\" />"));
    assert!(html.contains("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />"));
    
    // Verify title configuration
    assert!(html.contains("<title>Custom API</title>"));
    
    // Verify OpenAPI URL generation and usage
    let expected_url = OpenApiExporter::get_export_path("test-api", "1.0.0");
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
    let config = SwaggerUiConfig {
        enabled: true,
        title: None,
        ..SwaggerUiConfig::default()
    };
    
    let html = generate_swagger_ui(&config);
    assert!(html.contains("<title>API Documentation</title>"));
}

#[test]
fn test_swagger_ui_theme_variants() {
    // Test light theme (None)
    let light_config = SwaggerUiConfig {
        enabled: true,
        theme: None,
        ..SwaggerUiConfig::default()
    };
    let light_html = generate_swagger_ui(&light_config);
    assert!(!light_html.contains("background-color: #1a1a1a"));
    assert!(!light_html.contains("filter: invert(88%) hue-rotate(180deg)"));
    assert!(!light_html.contains(r#"syntaxHighlight: { theme: "monokai" }"#));
    
    // Test dark theme
    let dark_config = SwaggerUiConfig {
        enabled: true,
        theme: Some("dark".to_string()),
        ..SwaggerUiConfig::default()
    };
    let dark_html = generate_swagger_ui(&dark_config);
    assert!(dark_html.contains("background-color: #1a1a1a"));
    assert!(dark_html.contains("filter: invert(88%) hue-rotate(180deg)"));
    assert!(dark_html.contains(r#"syntaxHighlight: { theme: "monokai" }"#));
}

#[test]
fn test_swagger_ui_disabled() {
    let config = SwaggerUiConfig {
        enabled: false,
        ..SwaggerUiConfig::default()
    };
    assert_eq!(generate_swagger_ui(&config), String::new());
} 