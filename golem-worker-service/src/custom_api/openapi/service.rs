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

use super::OpenApiInputs;
use super::budget::Budget;
use super::cache::CacheState;
use super::http_openapi_spec::{build_security, render_full_path};
use super::merge::{ProviderContribution, merge_bounded};
use super::provider_document::{self, Category, DocumentError, PROVIDER_BYTE_LIMIT};
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour};
use crate::service::worker::WorkerService;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::model::agent::{InvocationFreshnessDisposition, ParsedAgentId, Principal};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, AgentInvocationResult, IdempotencyKey};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_service_base::model::auth::AuthCtx;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, timeout, timeout_at};

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(5);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const PROVIDER_CONCURRENCY: usize = 8;
const GENERATION_CONCURRENCY: usize = 8;

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

    pub fn is_stale(&self) -> bool {
        self.category == "stale"
    }

    pub fn status(&self) -> http::StatusCode {
        match self.category {
            "provider-timeout" | "generation-timeout" => http::StatusCode::GATEWAY_TIMEOUT,
            "admission" => http::StatusCode::SERVICE_UNAVAILABLE,
            _ => http::StatusCode::BAD_GATEWAY,
        }
    }
}

impl From<DocumentError> for OpenApiError {
    fn from(error: DocumentError) -> Self {
        let category = match error.category {
            Category::Size => "provider-size",
            Category::MergedSize => "merged-size",
            Category::Timeout => "generation-timeout",
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
    agent_id: AgentId,
    key: IdempotencyKey,
    method: String,
    environment_id: EnvironmentId,
}

#[async_trait]
trait ProviderInvoker: Send + Sync {
    async fn invoke(&self, call: &ProviderCall) -> Result<String, OpenApiError>;
    async fn cleanup(&self, call: &ProviderCall);
}

struct WorkerProviderInvoker(Arc<WorkerService>);

#[async_trait]
impl ProviderInvoker for WorkerProviderInvoker {
    async fn invoke(&self, call: &ProviderCall) -> Result<String, OpenApiError> {
        let input = SchemaValue::Record { fields: vec![] }
            .try_into()
            .map_err(|_| OpenApiError::new("provider-input"))?;
        let output = self
            .0
            .invoke_agent(
                &call.agent_id,
                Some(call.method.clone()),
                Some(input),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                Some(call.key.clone()),
                None,
                true,
                InvocationFreshnessDisposition::KnownFresh,
                vec![],
                AuthCtx::System,
                Principal::anonymous().into(),
                Some(call.environment_id),
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

    async fn cleanup(&self, call: &ProviderCall) {
        // These are bounded best-effort control requests, not confirmation that
        // execution stopped or that any side effects were rolled back.
        let _ = tokio::join!(
            self.0
                .cancel_invocation(&call.agent_id, &call.key, AuthCtx::System),
            self.0.interrupt(&call.agent_id, false, AuthCtx::System),
        );
    }
}

#[derive(Clone)]
pub struct OpenApiService {
    invoker: Arc<dyn ProviderInvoker>,
    pub(super) admission: Arc<Semaphore>,
    pub(super) cache: Arc<Mutex<CacheState>>,
    #[cfg(test)]
    pub(super) processing_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl OpenApiService {
    pub fn new(worker: Arc<WorkerService>) -> Self {
        Self {
            invoker: Arc::new(WorkerProviderInvoker(worker)),
            admission: Arc::new(Semaphore::new(GENERATION_CONCURRENCY)),
            cache: Arc::new(Mutex::new(CacheState::default())),
            #[cfg(test)]
            processing_hook: None,
        }
    }

    pub(super) async fn run(
        &self,
        inputs: Arc<OpenApiInputs>,
        budget: Budget,
        lease: Arc<OwnedSemaphorePermit>,
    ) -> Result<Arc<OpenApiDocument>, OpenApiError> {
        let providers = inputs.routes.iter().filter(|route| matches!(
            &route.behavior, RichRouteBehaviour::HttpRouter(router) if router.openapi_provider_method.is_some()
        )).cloned().collect::<Vec<_>>();
        let documents = stream::iter(providers)
            .map(|route| {
                let invoker = self.invoker.clone();
                let lease = lease.clone();
                async move {
                    let text = invoke_provider(invoker, &route, lease).await?;
                    Ok::<_, OpenApiError>((route, text))
                }
            })
            .buffer_unordered(PROVIDER_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;
        #[cfg(test)]
        let processing_hook = self.processing_hook.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            #[cfg(test)]
            if let Some(hook) = processing_hook {
                hook();
            }
            budget.check()?;
            let mut generated = inputs
                .generated_contribution()
                .map_err(|_| OpenApiError::new("generated-document"))?;
            budget.check()?;
            let mut schemes = generated
                .get_mut("components")
                .and_then(Value::as_object_mut)
                .and_then(|components| components.remove("securitySchemes"))
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            let mut contributions = Vec::with_capacity(documents.len());
            for (route, text) in documents {
                budget.check()?;
                let RichRouteBehaviour::HttpRouter(router) = &route.behavior else {
                    unreachable!()
                };
                let mount = render_full_path(&route.path);
                let identity = format!("{}:{}:{mount}", router.component_id, router.agent_type.0);
                let document = provider_document::parse(&identity, &text)?;
                budget.check()?;
                let security = build_security(&route.security, &mut schemes)
                    .map_err(|_| OpenApiError::new("mount-security"))?;
                contributions.push(ProviderContribution {
                    router: identity,
                    mount,
                    router_type: router.agent_type.0.clone(),
                    component_id: router.component_id,
                    document,
                    mount_security: serde_json::from_value(security)
                        .map_err(|_| OpenApiError::new("mount-security"))?,
                });
            }
            if !schemes.is_empty() {
                generated["components"]["securitySchemes"] = Value::Object(schemes);
            }
            let document = merge_bounded(generated, contributions, &inputs.public_origin, &budget)?;
            let json = budget.json(&document)?.into();
            let yaml = serde_yaml::to_string(&document)
                .map_err(|_| OpenApiError::new("encoding"))?
                .into();
            budget.check()?;
            Ok(Arc::new(OpenApiDocument { json, yaml }))
        })
        .await
        .map_err(|_| OpenApiError::new("generation-failed"))?
    }
}

fn prepare_call(route: &RichCompiledRoute) -> Result<ProviderCall, OpenApiError> {
    let RichRouteBehaviour::HttpRouter(router) = &route.behavior else {
        unreachable!()
    };
    let key = IdempotencyKey::fresh();
    let logical = ParsedAgentId::try_new(
        router.agent_type.clone(),
        TypedSchemaValue::new(
            router.constructor_input.graph.clone(),
            SchemaValue::Record { fields: vec![] },
        ),
        None,
    )
    .map_err(|_| OpenApiError::new("provider-input"))?;
    let identity = logical
        .with_ephemeral_invocation_phantom(&key)
        .map_err(|_| OpenApiError::new("provider-input"))?;
    Ok(ProviderCall {
        agent_id: AgentId {
            component_id: router.component_id,
            agent_id: identity.to_string(),
        },
        key,
        method: router
            .openapi_provider_method
            .as_ref()
            .unwrap()
            .method_name
            .clone(),
        environment_id: route.environment_id,
    })
}

async fn invoke_provider(
    invoker: Arc<dyn ProviderInvoker>,
    route: &RichCompiledRoute,
    lease: Arc<OwnedSemaphorePermit>,
) -> Result<String, OpenApiError> {
    let deadline = Instant::now() + PROVIDER_TIMEOUT;
    let result = timeout_at(deadline, async {
        let call = Arc::new(prepare_call(route)?);
        let mut cleanup = Cleanup {
            invoker: invoker.clone(),
            call: Some(call.clone()),
            lease,
        };
        let text = invoker.invoke(&call).await?;
        cleanup.call = None;
        if Instant::now() > deadline {
            return Err(OpenApiError::new("provider-timeout"));
        }
        if text.len() > PROVIDER_BYTE_LIMIT {
            return Err(OpenApiError::new("provider-size"));
        }
        Ok(text)
    })
    .await;
    result.unwrap_or_else(|_| Err(OpenApiError::new("provider-timeout")))
}

struct Cleanup {
    invoker: Arc<dyn ProviderInvoker>,
    call: Option<Arc<ProviderCall>>,
    lease: Arc<OwnedSemaphorePermit>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(call) = self.call.take() {
            let invoker = self.invoker.clone();
            let lease = self.lease.clone();
            tokio::spawn(async move {
                let _lease = lease;
                let _ = timeout(CLEANUP_TIMEOUT, invoker.cleanup(&call)).await;
            });
        }
    }
}

#[cfg(test)]
pub(in crate::custom_api) mod tests;
