// Test Swagger UI integration
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

use crate::api::openapi::SwaggerUiConfig;
use crate::api::openapi::generate_swagger_ui;

#[test]
fn test_swagger_ui_rendering() {
    // Configure Swagger UI
    let config = SwaggerUiConfig {
        enabled: true,
        path: "/docs".to_string(),
        title: Some("My Test API".to_string()),
        theme: Some("dark".to_string()),
        api_id: "test_api".to_string(),
        version: "1.0".to_string(),
    };

    // Generate Swagger UI HTML
    let swagger_ui_html = generate_swagger_ui(&config);

    // Assert the HTML contains expected elements
    assert!(swagger_ui_html.contains("My Test API"));
    assert!(swagger_ui_html.contains("swagger-ui"));
    assert!(swagger_ui_html.contains("dark"));
}
