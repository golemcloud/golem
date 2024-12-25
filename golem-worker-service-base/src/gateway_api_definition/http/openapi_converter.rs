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

use utoipa::openapi::OpenApi;
use std::sync::Arc;
use crate::gateway_api_definition::http::openapi_export::OpenApiExporter;

pub struct OpenApiConverter {
    pub exporter: Arc<OpenApiExporter>,
}

impl OpenApiConverter {
    pub fn new() -> Self {
        Self {
            exporter: Arc::new(OpenApiExporter),
        }
    }

    pub fn merge_openapi(mut base: OpenApi, other: OpenApi) -> OpenApi {
        // Merge paths
        base.paths.paths.extend(other.paths.paths);

        // Merge components
        match (&mut base.components, other.components.as_ref()) {
            (Some(base_components), Some(other_components)) => {
                // Merge schemas
                base_components.schemas.extend(other_components.schemas.clone());
                // Merge responses
                base_components.responses.extend(other_components.responses.clone());
                // Merge security schemes
                base_components.security_schemes.extend(other_components.security_schemes.clone());
            }
            _ => base.components = other.components,
        }

        // Merge security requirements
        match (&mut base.security, other.security.as_ref()) {
            (Some(base_security), Some(other_security)) => {
                base_security.extend(other_security.clone());
            }
            _ => base.security = other.security,
        }

        // Merge tags
        match (&mut base.tags, other.tags.as_ref()) {
            (Some(base_tags), Some(other_tags)) => {
                base_tags.extend(other_tags.clone());
            }
            _ => base.tags = other.tags,
        }

        // Merge servers
        match (&mut base.servers, other.servers.as_ref()) {
            (Some(base_servers), Some(other_servers)) => {
                base_servers.extend(other_servers.clone());
            }
            _ => base.servers = other.servers,
        }

        base
    }
}

impl Default for OpenApiConverter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_openapi_converter() {
        let _converter = super::OpenApiConverter::new();
        
        // Create base OpenAPI
        let mut base = utoipa::openapi::OpenApi::new();
        base.paths.paths.insert("/base".to_string(), utoipa::openapi::PathItem::new()
            .get(utoipa::openapi::path::Operation::new().description("Base operation")));
        base.components = Some(utoipa::openapi::Components::new()
            .schema("BaseSchema", utoipa::openapi::Schema::Object(utoipa::openapi::schema::Object::new()))
            .response("BaseResponse", utoipa::openapi::Response::new("Base response"))
            .security_scheme("BaseAuth", utoipa::openapi::security::SecurityScheme::ApiKey(utoipa::openapi::security::ApiKey::Header("X-Base-Auth".to_string()))));
        base.security = Some(vec![utoipa::openapi::SecurityRequirement::new("BaseAuth")]);
        base.tags = Some(vec![utoipa::openapi::Tag::new("base")]);
        base.servers = Some(vec![utoipa::openapi::Server::new("/base")]);
        
        // Create other OpenAPI with duplicate path
        let mut other = utoipa::openapi::OpenApi::new();
        other.paths.paths.insert("/base".to_string(), utoipa::openapi::PathItem::new()
            .post(utoipa::openapi::path::Operation::new().description("Other operation")));
        other.components = Some(utoipa::openapi::Components::new()
            .schema("OtherSchema", utoipa::openapi::Schema::Object(utoipa::openapi::schema::Object::new()))
            .response("OtherResponse", utoipa::openapi::Response::new("Other response"))
            .security_scheme("OtherAuth", utoipa::openapi::security::SecurityScheme::ApiKey(utoipa::openapi::security::ApiKey::Header("X-Other-Auth".to_string()))));
        other.security = Some(vec![utoipa::openapi::SecurityRequirement::new("OtherAuth")]);
        other.tags = Some(vec![utoipa::openapi::Tag::new("other")]);
        other.servers = Some(vec![utoipa::openapi::Server::new("/other")]);
        
        // Test merging with duplicates
        let merged = super::OpenApiConverter::merge_openapi(base.clone(), other.clone());
        
        // Verify paths merged and duplicates handled
        assert!(merged.paths.paths.contains_key("/base"));
        let base_path = merged.paths.paths.get("/base").unwrap();
        assert!(base_path.get.is_some(), "GET operation should be preserved");
        assert!(base_path.post.is_some(), "POST operation should be added");
        
        // Verify components merged
        let components = merged.components.unwrap();
        assert!(components.schemas.contains_key("BaseSchema"));
        assert!(components.schemas.contains_key("OtherSchema"));
        assert!(components.responses.contains_key("BaseResponse"));
        assert!(components.responses.contains_key("OtherResponse"));
        assert!(components.security_schemes.contains_key("BaseAuth"));
        assert!(components.security_schemes.contains_key("OtherAuth"));
        
        // Test empty component merging
        let mut empty_base = utoipa::openapi::OpenApi::new();
        empty_base.components = None;
        let merged = super::OpenApiConverter::merge_openapi(empty_base, other);
        assert!(merged.components.is_some());
        let components = merged.components.unwrap();
        assert!(components.schemas.contains_key("OtherSchema"));
    }

    #[test]
    fn test_openapi_converter_new() {
        let converter = super::OpenApiConverter::new();
        assert!(Arc::strong_count(&converter.exporter) == 1);
    }

    #[test]
    fn test_merge_openapi_with_empty_fields() {
        // Test merging when base has empty optional fields
        let mut base = utoipa::openapi::OpenApi::new();
        base.security = None;
        base.tags = None;
        base.servers = None;
        base.components = None;

        // Create other OpenAPI with all fields populated
        let mut other = utoipa::openapi::OpenApi::new();
        other.security = Some(vec![utoipa::openapi::SecurityRequirement::new("OtherAuth")]);
        other.tags = Some(vec![utoipa::openapi::Tag::new("other")]);
        other.servers = Some(vec![utoipa::openapi::Server::new("/other")]);
        other.components = Some(utoipa::openapi::Components::new()
            .schema("OtherSchema", utoipa::openapi::Schema::Object(utoipa::openapi::schema::Object::new())));

        let merged = super::OpenApiConverter::merge_openapi(base, other.clone());

        // Verify all fields were properly merged
        assert_eq!(merged.security, other.security);
        assert_eq!(merged.tags, other.tags);
        assert_eq!(merged.servers, other.servers);
        assert_eq!(merged.components, other.components);
    }
}
