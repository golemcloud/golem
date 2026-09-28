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

use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OpenApiKey {
    fingerprint: [u8; 32],
}

impl OpenApiKey {
    pub(crate) fn from_inputs(public_origin: &str, routes: &[Arc<RichCompiledRoute>]) -> Self {
        let mut fingerprint = blake3::Hasher::new();
        fingerprint.update(public_origin.as_bytes());
        for route in routes {
            fingerprint.update(
                format!(
                    "\0{:?}\0{}\0{}\0{:?}\0{:?}",
                    route.environment_id,
                    route.deployment_revision,
                    route.route_id,
                    route.route_match,
                    route.path,
                )
                .as_bytes(),
            );
            match &route.behavior {
                RichRouteBehaviour::CallAgent(call) => {
                    fingerprint.update(format!("\0call:{call:?}").as_bytes());
                }
                RichRouteBehaviour::HttpRouter(provider)
                    if provider.openapi_provider_method.is_some() =>
                {
                    fingerprint.update(format!("\0provider:{provider:?}").as_bytes());
                }
                _ => {}
            }
            match &route.security {
                RichRouteSecurity::None => fingerprint.update(b"\0security:none"),
                RichRouteSecurity::Unavailable => fingerprint.update(b"\0security:unavailable"),
                RichRouteSecurity::SessionFromHeader(inner) => fingerprint.update(
                    format!(
                        "\0security:header:{}",
                        inner.header_name.to_ascii_lowercase()
                    )
                    .as_bytes(),
                ),
                RichRouteSecurity::SecurityScheme(inner) => {
                    let details = &inner.security_scheme;
                    fingerprint.update(
                        format!(
                            "\0security:oidc:{}:{:?}:{:?}",
                            details.name, details.provider_type, details.scopes
                        )
                        .as_bytes(),
                    )
                }
            };
        }
        Self {
            fingerprint: fingerprint.finalize().into(),
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
