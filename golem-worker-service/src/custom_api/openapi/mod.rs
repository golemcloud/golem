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

mod budget;
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

pub(crate) use budget::GENERATION_TIMEOUT;
pub use http_openapi_spec::*;
pub use service::{OpenApiDocument, OpenApiError, OpenApiService};

#[cfg(test)]
pub(in crate::custom_api) use service::tests as test_support;

use crate::custom_api::RichCompiledRoute;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub(crate) struct Freshness {
    counter: Arc<AtomicU64>,
    observed: u64,
}

impl Freshness {
    pub fn capture(counter: Arc<AtomicU64>) -> Self {
        let observed = counter.load(Ordering::SeqCst);
        Self { counter, observed }
    }

    pub fn is_current(&self) -> bool {
        self.counter.load(Ordering::SeqCst) == self.observed
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OpenApiKey {
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub domain: Domain,
}

pub struct OpenApiInputs {
    pub(crate) key: OpenApiKey,
    pub(crate) freshness: Freshness,
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
