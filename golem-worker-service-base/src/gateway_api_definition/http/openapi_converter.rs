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

use std::sync::Arc;
use utoipa::openapi::OpenApi;
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
    use super::OpenApiConverter;
    use utoipa::openapi::{
        OpenApi,
        Server,
        Tag,
    };

    #[test]
    fn test_openapi_converter() {
        let _converter = OpenApiConverter::new();
        
        // Create base OpenAPI
        let mut base = OpenApi::new(Default::default(), ());
        let get_op = OperationBuilder::new().summary(Some("Base operation".to_string())).build();
        let mut path_item = PathItem::new(HttpMethod::Get, ());
        path_item.get = Some(get_op);
        base.paths.paths.insert("/base".to_string(), path_item);
        let mut components = Components::new();
        components.schemas.insert("BaseSchema".to_string(), Schema::Object(Object::new()).into());
        base.components = Some(components);
        base.security = Some(vec![SecurityRequirement::new("BaseAuth", ())]);
        base.tags = Some(vec![Tag::new("base")]);
        base.servers = Some(vec![Server::new("/base")]);
        
        // Create other OpenAPI with duplicate path
        let mut other = OpenApi::new(Default::default(), ());
        let post_op = OperationBuilder::new().summary(Some("Other operation".to_string())).build();
        let mut path_item = PathItem::new(HttpMethod::Get, ());
        path_item.post = Some(post_op);
        other.paths.paths.insert("/base".to_string(), path_item);
        let mut components = Components::new();
        components.schemas.insert("OtherSchema".to_string(), Schema::Object(Object::new()).into());
        other.components = Some(components);
        other.security = Some(vec![SecurityRequirement::new("OtherAuth", ())]);
        other.tags = Some(vec![Tag::new("other")]);
        other.servers = Some(vec![Server::new("/other")]);
        
        // Test merging with duplicates
        let merged = OpenApiConverter::merge_openapi(base.clone(), other.clone());
        
        // Verify paths merged and duplicates handled
        assert!(merged.paths.paths.contains_key("/base"));
        let base_path = merged.paths.paths.get("/base").unwrap();
        assert!(base_path.get.is_some(), "GET operation should be preserved");
        assert!(base_path.post.is_some(), "POST operation should be added");
        
        // Verify components merged
        let components = merged.components.unwrap();
        assert!(components.schemas.contains_key("BaseSchema"));
        assert!(components.schemas.contains_key("OtherSchema"));
        
        // Test empty component merging
        let mut empty_base = OpenApi::new(Default::default(), ());
        empty_base.components = None;
        let merged = OpenApiConverter::merge_openapi(empty_base, other);
        assert!(merged.components.is_some());
        let components = merged.components.unwrap();
        assert!(components.schemas.contains_key("OtherSchema"));
    }

    #[test]
    fn test_openapi_converter_new() {
        let converter = OpenApiConverter::new();
        assert_eq!(Arc::strong_count(&converter.exporter), 1);
    }

    #[test]
    fn test_merge_openapi_with_empty_fields() {
        // Test merging when base has empty optional fields
        let mut base = OpenApi::new(Default::default(), ());
        base.security = None;
        base.tags = None;
        base.servers = None;
        base.components = None;

        // Create other OpenAPI with all fields populated
        let mut other = OpenApi::new(Default::default(), ());
        other.security = Some(vec![SecurityRequirement::new("OtherAuth", ())]);
        other.tags = Some(vec![Tag::new("other")]);
        other.servers = Some(vec![Server::new("/other")]);
        let mut components = Components::new();
        components.schemas.insert("OtherSchema".to_string(), Schema::Object(Object::new()).into());
        other.components = Some(components);

        let merged = OpenApiConverter::merge_openapi(base, other.clone());

        // Verify all fields were properly merged
        assert_eq!(merged.security, other.security);
        assert_eq!(merged.tags, other.tags);
        assert_eq!(merged.servers, other.servers);
        assert_eq!(merged.components, other.components);
    }
}
