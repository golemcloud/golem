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
        if let Some(base_components) = &mut base.components {
            if let Some(other_components) = other.components {
                // Merge schemas
                base_components.schemas.extend(other_components.schemas);
                // Merge responses
                base_components.responses.extend(other_components.responses);
                // Merge security schemes
                base_components.security_schemes.extend(other_components.security_schemes);
            }
        } else {
            base.components = other.components;
        }

        // Merge security requirements
        if let Some(base_security) = &mut base.security {
            if let Some(other_security) = other.security {
                base_security.extend(other_security);
            }
        } else {
            base.security = other.security;
        }

        // Merge tags
        if let Some(base_tags) = &mut base.tags {
            if let Some(other_tags) = other.tags {
                base_tags.extend(other_tags);
            }
        } else {
            base.tags = other.tags;
        }

        // Merge servers
        if let Some(base_servers) = &mut base.servers {
            if let Some(other_servers) = other.servers {
                base_servers.extend(other_servers);
            }
        } else {
            base.servers = other.servers;
        }

        base
    }
} 