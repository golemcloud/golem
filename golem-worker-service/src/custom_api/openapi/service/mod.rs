// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use super::http_openapi_spec::{build_security, render_full_path};
use super::merge::{ProviderContribution, merge};
use super::provider_document::{self, Category, DocumentError, PROVIDER_BYTE_LIMIT};
use super::{OpenApiInputs, bounded_json};
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour};
use crate::service::worker::{AgentBinding, WorkerService};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::AgentInvocationResult;
use golem_common::model::agent::ParsedAgentKey;
use golem_common::model::caller::Caller;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_service_base::custom_api::MountTarget;
use golem_service_base::model::auth::AuthCtx;
use serde_json::Value;
use std::sync::Arc;

const PROVIDER_CONCURRENCY: usize = 8;
const CACHE_CAPACITY: usize = 256;

pub struct OpenApiDocument {
    pub json: Bytes,
    pub yaml: Bytes,
}

impl std::fmt::Debug for OpenApiDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenApiDocument").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("OpenAPI generation failed: {category}")]
pub struct OpenApiError {
    category: &'static str,
    diagnostic: Option<DocumentError>,
}

impl OpenApiError {
    pub(crate) fn new(category: &'static str) -> Self {
        Self {
            category,
            diagnostic: None,
        }
    }

    pub fn category(&self) -> &'static str {
        self.category
    }

    pub fn status(&self) -> http::StatusCode {
        http::StatusCode::BAD_GATEWAY
    }
}

impl From<DocumentError> for OpenApiError {
    fn from(error: DocumentError) -> Self {
        let category = match error.category {
            Category::Size => "provider-size",
            Category::MergedSize => "merged-size",
            Category::Depth => "provider-depth",
            Category::Json => "provider-json",
            Category::Structure => "provider-structure",
            Category::Unsupported => "unsupported-field",
            Category::Path => "provider-path",
            Category::Reference => "unsupported-reference",
            Category::OperationConflict => "operation-conflict",
            Category::PathItemConflict => "path-item-conflict",
            Category::ComponentConflict => "component-conflict",
            Category::TagConflict => "tag-conflict",
            Category::ExtensionConflict => "extension-conflict",
            Category::OperationIdConflict => "operation-id-conflict",
            Category::Security => "provider-security",
        };
        Self {
            category,
            diagnostic: Some(error),
        }
    }
}

struct ProviderCall {
    component_id: ComponentId,
    constructor: ParsedAgentKey,
    key: golem_common::model::InvocationRequestKey,
    method: String,
    environment_id: EnvironmentId,
}

#[async_trait]
trait ProviderInvoker: Send + Sync {
    async fn invoke(&self, call: &ProviderCall) -> Result<String, OpenApiError>;
}

struct WorkerProviderInvoker(Arc<WorkerService>);

#[async_trait]
#[golem_async_trait::region_simple]
impl ProviderInvoker for WorkerProviderInvoker {
    async fn invoke(&self, call: &ProviderCall) -> Result<String, OpenApiError> {
        let input = SchemaValue::Record { fields: vec![] }
            .try_into()
            .map_err(|_| OpenApiError::new("provider-input"))?;
        let (target, component, freshness_disposition) = self
            .0
            .bound_invocation_target(
                call.component_id,
                &call.constructor,
                AgentBinding::Constructor,
                &call.key,
                &call.method,
                &AuthCtx::System,
            )
            .await
            .map_err(|_| OpenApiError::new("provider-invocation"))?;
        let output = self
            .0
            .dispatch_agent_invocation(
                &component,
                &target,
                Some(call.method.clone()),
                Some(input),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                call.key.clone(),
                None,
                freshness_disposition,
                vec![],
                call.environment_id,
                component.account_id,
                AuthCtx::System,
                Caller::anonymous().into(),
                None,
                None,
            )
            .await
            .map_err(|_| OpenApiError::new("provider-invocation"))?;
        match output.result {
            AgentInvocationResult::AgentMethod {
                output: SchemaValue::String(value),
            } => Ok(value),
            _ => Err(OpenApiError::new("provider-output")),
        }
    }
}

type OpenApiCache = Cache<super::OpenApiKey, (), Arc<OpenApiDocument>, OpenApiError>;

#[derive(Clone)]
pub struct OpenApiService {
    invoker: Arc<dyn ProviderInvoker>,
    pub(super) cache: OpenApiCache,
    #[cfg(test)]
    pub(super) processing_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl OpenApiService {
    pub fn new(worker: Arc<WorkerService>) -> Self {
        Self {
            invoker: Arc::new(WorkerProviderInvoker(worker)),
            cache: Cache::new(
                Some(CACHE_CAPACITY),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "openapi_documents",
            ),
            #[cfg(test)]
            processing_hook: None,
        }
    }

    pub(super) async fn run(
        &self,
        inputs: Arc<OpenApiInputs>,
    ) -> Result<Arc<OpenApiDocument>, OpenApiError> {
        let providers = inputs.routes.iter().filter(|route| matches!(
            &route.behavior, RichRouteBehaviour::Mount(mount) if mount.openapi_provider.is_some()
        )).cloned().collect::<Vec<_>>();
        let documents = stream::iter(providers)
            .map(|route| {
                let invoker = self.invoker.clone();
                async move {
                    let text = invoke_provider(invoker, &route).await?;
                    Ok::<_, OpenApiError>((route, text))
                }
            })
            .buffer_unordered(PROVIDER_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;
        #[cfg(test)]
        let processing_hook = self.processing_hook.clone();
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(hook) = processing_hook {
                hook();
            }
            let mut generated = inputs
                .generated_contribution()
                .map_err(|_| OpenApiError::new("generated-document"))?;
            let mut schemes = generated
                .get_mut("components")
                .and_then(Value::as_object_mut)
                .and_then(|components| components.remove("securitySchemes"))
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            let mut contributions = Vec::with_capacity(documents.len());
            for (route, text) in documents {
                let RichRouteBehaviour::Mount(mount) = &route.behavior else {
                    unreachable!()
                };
                let MountTarget::AgentMethod(provider) = mount.openapi_provider.as_ref().unwrap();
                let agent = &provider.agent;
                let mount = render_full_path(&route.path);
                let identity = format!("{}:{}:{mount}", agent.component_id, agent.agent_type.0);
                let document = provider_document::parse(&identity, &text)?;
                let security = build_security(&route.security, &mut schemes, &inputs.public_origin)
                    .map_err(|_| OpenApiError::new("mount-security"))?;
                contributions.push(ProviderContribution {
                    router: identity,
                    mount,
                    router_type: agent.agent_type.0.clone(),
                    component_id: agent.component_id,
                    document,
                    mount_security: serde_json::from_value(security)
                        .map_err(|_| OpenApiError::new("mount-security"))?,
                });
            }
            if !schemes.is_empty() {
                generated["components"]["securitySchemes"] = Value::Object(schemes);
            }
            let document = merge(generated, contributions, &inputs.public_origin)?;
            let json = bounded_json::to_vec(&document)?.into();
            let yaml = serde_yaml::to_string(&document)
                .map_err(|_| OpenApiError::new("encoding"))?
                .into();
            Ok(Arc::new(OpenApiDocument { json, yaml }))
        })
        .await
        .map_err(|_| OpenApiError::new("generation-failed"))?
    }
}

fn prepare_call(route: &RichCompiledRoute) -> Result<ProviderCall, OpenApiError> {
    let RichRouteBehaviour::Mount(mount) = &route.behavior else {
        unreachable!()
    };
    let MountTarget::AgentMethod(provider) = mount.openapi_provider.as_ref().unwrap();
    let agent = &provider.agent;
    let key = golem_common::model::InvocationRequestKey::external(None, None);
    let constructor = ParsedAgentKey::try_new(
        agent.agent_type.clone(),
        TypedSchemaValue::new(
            agent.constructor_input.graph.clone(),
            SchemaValue::Record { fields: vec![] },
        ),
    )
    .map_err(|_| OpenApiError::new("provider-input"))?;
    Ok(ProviderCall {
        component_id: agent.component_id,
        constructor,
        key,
        method: provider.method.method_name.clone(),
        environment_id: route.environment_id,
    })
}

async fn invoke_provider(
    invoker: Arc<dyn ProviderInvoker>,
    route: &RichCompiledRoute,
) -> Result<String, OpenApiError> {
    let call = prepare_call(route)?;
    let text = invoker.invoke(&call).await?;
    if text.len() > PROVIDER_BYTE_LIMIT {
        return Err(OpenApiError::new("provider-size"));
    }
    Ok(text)
}

#[cfg(test)]
pub(in crate::custom_api) mod tests;
