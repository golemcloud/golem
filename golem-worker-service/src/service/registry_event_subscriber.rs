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
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::custom_api::route_resolver::RouteResolver;
use crate::mcp::McpCapabilityLookup;
use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::auth::AuthService;
use crate::service::component::ComponentService;
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_service_base::clients::registry::{RegistryInvalidationHandler, RegistryService};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub(crate) struct WorkerServiceRegistryInvalidationHandler {
    component_service: Arc<dyn ComponentService>,
    agent_resolution_cache: Arc<AgentResolutionCache>,
    route_resolver: Arc<RouteResolver>,
    auth_service: Arc<dyn AuthService>,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
}

impl WorkerServiceRegistryInvalidationHandler {
    pub async fn run(
        registry_service: Arc<dyn RegistryService>,
        component_service: Arc<dyn ComponentService>,
        agent_resolution_cache: Arc<AgentResolutionCache>,
        route_resolver: Arc<RouteResolver>,
        auth_service: Arc<dyn AuthService>,
        mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
        shutdown_token: Option<CancellationToken>,
    ) {
        registry_service
            .run_registry_invalidation_event_subscriber(
                "worker-service",
                shutdown_token,
                Arc::new(Self {
                    component_service,
                    agent_resolution_cache,
                    route_resolver,
                    auth_service,
                    mcp_capability_lookup,
                }),
            )
            .await;
    }
}

#[async_trait::async_trait]
impl RegistryInvalidationHandler for WorkerServiceRegistryInvalidationHandler {
    async fn on_event(&self, event: RegistryInvalidationEvent) {
        if matches!(
            event,
            RegistryInvalidationEvent::CursorExpired { .. }
                | RegistryInvalidationEvent::DeploymentChanged { .. }
                | RegistryInvalidationEvent::ApplicationDeleted { .. }
                | RegistryInvalidationEvent::EnvironmentDeleted { .. }
        ) {
            self.component_service.invalidate_current().await;
        }
        if matches!(
            event,
            RegistryInvalidationEvent::CursorExpired { .. }
                | RegistryInvalidationEvent::DeploymentChanged { .. }
                | RegistryInvalidationEvent::DomainRegistrationChanged { .. }
                | RegistryInvalidationEvent::SecuritySchemeChanged { .. }
                | RegistryInvalidationEvent::ApplicationDeleted { .. }
                | RegistryInvalidationEvent::EnvironmentDeleted { .. }
        ) {
            self.mcp_capability_lookup.invalidate_all().await;
        }
        match &event {
            RegistryInvalidationEvent::CursorExpired { .. } => {
                warn!("Registry invalidation cursor expired, flushing all caches");
                self.route_resolver.clear_all().await;
                self.agent_resolution_cache.clear().await;
                self.auth_service.clear_all_caches().await;
            }
            RegistryInvalidationEvent::DeploymentChanged {
                environment_id,
                deployment_revision,
                current_deployment_revision,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    deployment_revision = %deployment_revision,
                    current_deployment_revision = %current_deployment_revision,
                    "Received deployment changed event"
                );
                self.agent_resolution_cache.update_current_revision(
                    *environment_id,
                    *deployment_revision,
                    *current_deployment_revision,
                );
                self.route_resolver
                    .invalidate_domains_for_environment(*environment_id)
                    .await;
            }
            RegistryInvalidationEvent::DomainRegistrationChanged {
                environment_id,
                domains,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    domains = ?domains,
                    "Received domain registration changed event"
                );
                self.route_resolver.clear_all().await;
            }
            RegistryInvalidationEvent::AccountTokensInvalidated { account_id, .. } => {
                debug!(
                    account_id = %account_id,
                    "Received account tokens invalidated event"
                );
                self.auth_service
                    .invalidate_tokens_for_account(*account_id)
                    .await;
            }
            RegistryInvalidationEvent::EnvironmentPermissionsChanged {
                environment_id,
                grantee_account_id,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    grantee_account_id = %grantee_account_id,
                    "Received environment permissions changed event"
                );
                self.auth_service
                    .invalidate_environment_auth(*environment_id, *grantee_account_id)
                    .await;
            }
            RegistryInvalidationEvent::SecuritySchemeChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received security scheme changed event, invalidating route cache"
                );
                self.route_resolver
                    .invalidate_domains_for_environment(*environment_id)
                    .await;
            }
            RegistryInvalidationEvent::RetryPolicyChanged { environment_id, .. } => {
                debug!(
                    environment_id = %environment_id,
                    "Received retry policy changed event, ignoring"
                );
            }
            RegistryInvalidationEvent::ResourceDefinitionChanged { .. } => {}
            RegistryInvalidationEvent::AgentSecretChanged { environment_id, .. } => {
                self.route_resolver
                    .invalidate_domains_for_environment(*environment_id)
                    .await;
            }
            RegistryInvalidationEvent::CardRevoked { .. } => {}
            RegistryInvalidationEvent::ApplicationDeleted {
                application_id,
                account_id,
                app_name,
                environment_ids,
                ..
            } => {
                debug!(
                    application_id = %application_id,
                    account_id = %account_id,
                    app_name,
                    environment_count = environment_ids.len(),
                    "Received application deleted event, invalidating targeted cache entries"
                );
                self.agent_resolution_cache
                    .invalidate_by_app_name(app_name, environment_ids)
                    .await;
                for env_id in environment_ids {
                    self.route_resolver
                        .invalidate_domains_for_environment(*env_id)
                        .await;
                }
            }
            RegistryInvalidationEvent::EnvironmentDeleted {
                environment_id,
                app_name,
                env_name,
                ..
            } => {
                debug!(
                    environment_id = %environment_id,
                    app_name,
                    env_name,
                    "Received environment deleted event, invalidating targeted cache entries"
                );
                self.agent_resolution_cache
                    .invalidate_by_env(app_name, env_name, *environment_id)
                    .await;
                self.route_resolver
                    .invalidate_domains_for_environment(*environment_id)
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthServiceConfig, ComponentServiceConfig, RouteResolverConfig};
    use crate::custom_api::api_definition_lookup::RegistryServiceApiDefinitionsLookup;
    use crate::mcp::RegistryServiceMcpCapabilityLookup;
    use crate::service::auth::RemoteAuthService;
    use crate::service::component::RemoteComponentService;
    use crate::service::component::tests::{TestRegistry, component};
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use std::time::Duration;
    use test_r::test;

    #[test]
    async fn deployment_cursor_and_deletion_events_refresh_current_metadata() {
        let a = component();
        let mut b = a.clone();
        b.revision = ComponentRevision::new(6).unwrap();
        let registry = Arc::new(TestRegistry::new(
            vec![a.clone(), b.clone()],
            Some(a.clone()),
        ));
        let components = Arc::new(RemoteComponentService::new(
            registry.clone(),
            &ComponentServiceConfig::default(),
        ));
        let handler = WorkerServiceRegistryInvalidationHandler {
            component_service: components.clone(),
            agent_resolution_cache: Arc::new(AgentResolutionCache::new(
                registry.clone(),
                10,
                Duration::from_secs(60),
                Duration::from_secs(60),
            )),
            route_resolver: Arc::new(RouteResolver::new(
                &RouteResolverConfig::default(),
                Arc::new(RegistryServiceApiDefinitionsLookup::new(registry.clone())),
            )),
            auth_service: Arc::new(RemoteAuthService::new(
                registry.clone(),
                &AuthServiceConfig::default(),
            )),
            mcp_capability_lookup: Arc::new(RegistryServiceMcpCapabilityLookup::new(
                registry.clone(),
            )),
        };
        let events = [
            RegistryInvalidationEvent::DeploymentChanged {
                event_id: 1,
                environment_id: a.environment_id,
                deployment_revision: DeploymentRevision::INITIAL,
                current_deployment_revision: CurrentDeploymentRevision::INITIAL,
            },
            RegistryInvalidationEvent::CursorExpired { event_id: 2 },
            RegistryInvalidationEvent::ApplicationDeleted {
                event_id: 3,
                application_id: a.application_id,
                account_id: a.account_id,
                app_name: a.application_name.0.clone(),
                environment_ids: vec![a.environment_id],
            },
            RegistryInvalidationEvent::EnvironmentDeleted {
                event_id: 4,
                environment_id: a.environment_id,
                app_name: a.application_name.0.clone(),
                env_name: a.environment_name.to_string(),
            },
        ];
        for event in events {
            *registry.deployed.lock().unwrap() = Some(a.clone());
            components.invalidate_current().await;
            assert_eq!(
                components.get_current_by_id(a.id).await.unwrap().revision,
                a.revision
            );
            *registry.deployed.lock().unwrap() = Some(b.clone());
            handler.on_event(event).await;
            assert_eq!(
                components.get_current_by_id(a.id).await.unwrap().revision,
                b.revision
            );
        }
    }
}
