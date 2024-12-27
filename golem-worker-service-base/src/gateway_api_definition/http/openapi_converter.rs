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
        for (path, other_item) in other.paths.paths {
            if let Some(base_item) = base.paths.paths.get_mut(&path) {
                // Merge operations for existing paths
                if other_item.get.is_some() {
                    base_item.get = other_item.get;
                }
                if other_item.post.is_some() {
                    base_item.post = other_item.post;
                }
                if other_item.put.is_some() {
                    base_item.put = other_item.put;
                }
                if other_item.delete.is_some() {
                    base_item.delete = other_item.delete;
                }
                if other_item.options.is_some() {
                    base_item.options = other_item.options;
                }
                if other_item.head.is_some() {
                    base_item.head = other_item.head;
                }
                if other_item.patch.is_some() {
                    base_item.patch = other_item.patch;
                }
                if other_item.trace.is_some() {
                    base_item.trace = other_item.trace;
                }
            } else {
                // Add new paths
                base.paths.paths.insert(path, other_item);
            }
        }

        // Merge components if both exist
        if let Some(other_components) = other.components {
            match &mut base.components {
                Some(base_components) => {
                    // Move schemas
                    base_components.schemas.extend(other_components.schemas);
                    // Move responses
                    base_components.responses.extend(other_components.responses);
                    // Move security schemes
                    base_components.security_schemes.extend(other_components.security_schemes);
                }
                None => base.components = Some(other_components),
            }
        }

        // Merge security requirements if both exist
        if let Some(other_security) = other.security {
            match &mut base.security {
                Some(base_security) => base_security.extend(other_security),
                None => base.security = Some(other_security),
            }
        }

        // Merge tags if both exist
        if let Some(other_tags) = other.tags {
            match &mut base.tags {
                Some(base_tags) => base_tags.extend(other_tags),
                None => base.tags = Some(other_tags),
            }
        }

        // Merge servers if both exist
        if let Some(other_servers) = other.servers {
            match &mut base.servers {
                Some(base_servers) => base_servers.extend(other_servers),
                None => base.servers = Some(other_servers),
            }
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
        use utoipa::openapi::{
            Components,
            HttpMethod,
            Object,
            OpenApi,
            PathItem,
            Schema,
            SecurityRequirement,
            Server,
            Tag,
            path::OperationBuilder,
        };
        use super::OpenApiConverter;

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
        use super::OpenApiConverter;
        use std::sync::Arc;
        
        let converter = OpenApiConverter::new();
        assert_eq!(Arc::strong_count(&converter.exporter), 1);
    }

    #[test]
    fn test_merge_openapi_with_empty_fields() {
        use utoipa::openapi::{
            Components,
            Object,
            OpenApi,
            Schema,
            SecurityRequirement,
            Server,
            Tag,
        };
        use super::OpenApiConverter;

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
