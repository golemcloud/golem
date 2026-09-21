// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

mod bounded_json;
mod cache;
mod call_agent;
mod http_openapi_spec;
mod merge;
mod provider_document;
mod response_schema;
mod route_schema;
mod schema_mapping;
mod service;

#[cfg(test)]
mod tests;

pub use http_openapi_spec::*;
pub use service::{OpenApiDocument, OpenApiError, OpenApiService};

#[cfg(test)]
pub(in crate::custom_api) use service::tests as test_support;

use crate::custom_api::RichCompiledRoute;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OpenApiKey {
    snapshot_id: Uuid,
}

impl OpenApiKey {
    pub(crate) fn fresh() -> Self {
        Self {
            snapshot_id: Uuid::new_v4(),
        }
    }
}

pub struct OpenApiInputs {
    pub(crate) key: OpenApiKey,
    pub public_origin: String,
    pub routes: Vec<Arc<RichCompiledRoute>>,
}

impl OpenApiInputs {
    pub fn generated_contribution(&self) -> Result<serde_json::Value, String> {
        HttpApiOpenApiSpec::contribution_from_routes(
            &self.routes.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &self.public_origin,
        )
    }
}
