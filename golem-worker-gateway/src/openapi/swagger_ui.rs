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

use super::*;
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, Response, StatusCode},
    response::IntoResponse,
};
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const SWAGGER_UI_VERSION: &str = "5.11.2";
const SWAGGER_UI_BUNDLE_URL: &str = "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.11.2/swagger-ui-bundle.js";
const SWAGGER_UI_CSS_URL: &str = "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.11.2/swagger-ui.css";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUiConfig {
    pub title: String,
    pub description: Option<String>,
    pub version: String,
    pub theme: SwaggerUiTheme,
    pub default_models_expand_depth: i32,
    pub default_model_expand_depth: i32,
    pub default_model_rendering: ModelRendering,
    pub display_request_duration: bool,
    pub doc_expansion: DocExpansion,
    pub filter: bool,
    pub persist_authorization: bool,
    pub syntax_highlight: SyntaxHighlight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SwaggerUiTheme {
    Default,
    Dark,
    Feeling,
    Material,
    Monokai,
    Muted,
    Newspaper,
    Outline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelRendering {
    Example,
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocExpansion {
    List,
    Full,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyntaxHighlight {
    Agate,
    Arta,
    Monokai,
    Nord,
    ObsidianCustom,
}

impl Default for SwaggerUiConfig {
    fn default() -> Self {
        Self {
            title: "API Documentation".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            theme: SwaggerUiTheme::Default,
            default_models_expand_depth: 1,
            default_model_expand_depth: 1,
            default_model_rendering: ModelRendering::Example,
            display_request_duration: true,
            doc_expansion: DocExpansion::List,
            filter: true,
            persist_authorization: true,
            syntax_highlight: SyntaxHighlight::Monokai,
        }
    }
}

/// Handles serving the Swagger UI and OpenAPI specification
pub struct SwaggerUiHandler {
    openapi_spec: Arc<String>,
    config: SwaggerUiConfig,
    custom_css_url: Option<String>,
    custom_js_url: Option<String>,
    cors_allowed_origins: Option<String>,
}

impl SwaggerUiHandler {
    pub fn new(
        openapi_spec: String,
        config: Option<SwaggerUiConfig>,
        custom_css_url: Option<String>,
        custom_js_url: Option<String>,
        cors_allowed_origins: Option<String>,
    ) -> Self {
        Self {
            openapi_spec: Arc::new(openapi_spec),
            config: config.unwrap_or_default(),
            custom_css_url,
            custom_js_url,
            cors_allowed_origins,
        }
    }

    /// Serve the Swagger UI HTML page
    pub async fn serve_ui(State(handler): State<Arc<Self>>) -> impl IntoResponse {
        let html = generate_swagger_ui_html(
            "/spec",
            handler.custom_css_url.as_deref(),
            handler.custom_js_url.as_deref(),
            &handler.config,
        );

        let mut response = Response::new(Body::from(html));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );

        if let Some(origins) = &handler.cors_allowed_origins {
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_str(origins).unwrap(),
            );
        }

        response
    }

    /// Serve the OpenAPI specification
    pub async fn serve_spec(State(handler): State<Arc<Self>>) -> impl IntoResponse {
        let mut response = Response::new(Body::from(handler.openapi_spec.to_string()));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );

        if let Some(origins) = &handler.cors_allowed_origins {
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_str(origins).unwrap(),
            );
        }

        response
    }
}

#[cached(
    size = 100,
    key = "String",
    convert = r#"{ format!("{}{}{}{:?}", spec_url, custom_css_url.unwrap_or(""), custom_js_url.unwrap_or(""), config) }"#
)]
fn generate_swagger_ui_html(
    spec_url: &str,
    custom_css_url: Option<&str>,
    custom_js_url: Option<&str>,
    config: &SwaggerUiConfig,
) -> String {
    let css_url = custom_css_url.unwrap_or(SWAGGER_UI_CSS_URL);
    let js_url = custom_js_url.unwrap_or(SWAGGER_UI_BUNDLE_URL);
    let theme_url = match config.theme {
        SwaggerUiTheme::Default => None,
        theme => Some(format!(
            "https://cdn.jsdelivr.net/npm/swagger-ui-themes@3.0.1/themes/3.x/theme-{}.css",
            format!("{:?}", theme).to_lowercase()
        )),
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>{}</title>
    <link rel="stylesheet" type="text/css" href="{}" />
    {}
    <style>
        html {{
            box-sizing: border-box;
            overflow: -moz-scrollbars-vertical;
            overflow-y: scroll;
        }}
        *, *:before, *:after {{
            box-sizing: inherit;
        }}
        body {{
            margin: 0;
            background: #fafafa;
        }}
        .swagger-ui .topbar {{
            display: none;
        }}
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="{}"></script>
    <script>
        window.onload = function() {{
            const ui = SwaggerUIBundle({{
                url: "{}",
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
                layout: "StandaloneLayout",
                defaultModelsExpandDepth: {},
                defaultModelExpandDepth: {},
                defaultModelRendering: "{}",
                displayRequestDuration: {},
                docExpansion: "{}",
                filter: {},
                persistAuthorization: {},
                syntaxHighlight: {{
                    activate: true,
                    theme: "{}"
                }},
                {}
            }});
            window.ui = ui;
        }};
    </script>
</body>
</html>"#,
        config.title,
        css_url,
        theme_url
            .map(|url| format!(r#"<link rel="stylesheet" type="text/css" href="{}" />"#, url))
            .unwrap_or_default(),
        js_url,
        spec_url,
        config.default_models_expand_depth,
        config.default_model_expand_depth,
        format!("{:?}", config.default_model_rendering).to_lowercase(),
        config.display_request_duration,
        format!("{:?}", config.doc_expansion).to_lowercase(),
        config.filter,
        config.persist_authorization,
        format!("{:?}", config.syntax_highlight).to_lowercase(),
        config
            .description
            .as_ref()
            .map(|desc| format!(r#"description: "{}""#, desc))
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_swagger_ui_handler() {
        let handler = Arc::new(SwaggerUiHandler::new(
            r#"{"openapi":"3.0.0"}"#.to_string(),
            Some(SwaggerUiConfig {
                title: "Test API".to_string(),
                description: Some("Test Description".to_string()),
                version: "1.0.0".to_string(),
                theme: SwaggerUiTheme::Dark,
                ..Default::default()
            }),
            None,
            None,
            Some("*".to_string()),
        ));

        // Test UI endpoint
        let response = SwaggerUiHandler::serve_ui(State(handler.clone())).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );

        // Test spec endpoint
        let response = SwaggerUiHandler::serve_spec(State(handler)).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[test]
    fn test_swagger_ui_config() {
        let config = SwaggerUiConfig::default();
        let html = generate_swagger_ui_html("/spec", None, None, &config);
        
        assert!(html.contains("defaultModelsExpandDepth: 1"));
        assert!(html.contains("defaultModelExpandDepth: 1"));
        assert!(html.contains("defaultModelRendering: \"example\""));
        assert!(html.contains("displayRequestDuration: true"));
        assert!(html.contains("docExpansion: \"list\""));
        assert!(html.contains("filter: true"));
        assert!(html.contains("persistAuthorization: true"));
    }
}
