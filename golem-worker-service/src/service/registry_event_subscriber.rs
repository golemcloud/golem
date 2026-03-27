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
use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::auth::AuthService;
use golem_common::model::agent::RegistryInvalidationEvent;
use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
use golem_common::model::domain_registration::Domain;
use golem_service_base::clients::registry::RegistryService;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

pub async fn run_registry_event_subscriber(
    registry_service: Arc<dyn RegistryService>,
    agent_resolution_cache: Arc<AgentResolutionCache>,
    route_resolver: Arc<RouteResolver>,
    auth_service: Arc<dyn AuthService>,
) {
    use futures::StreamExt;

    let mut last_seen_event_id: Option<u64> = None;
    let mut backoff = Duration::from_millis(100);
    let max_backoff = Duration::from_secs(30);

    loop {
        match registry_service
            .subscribe_registry_invalidations(last_seen_event_id)
            .await
        {
            Ok(mut stream) => {
                info!("Connected to registry invalidation stream");
                backoff = Duration::from_millis(100);

                while let Some(result) = stream.next().await {
                    match result {
                        Ok(event) => {
                            last_seen_event_id = Some(event.event_id());
                            dispatch_event(
                                &event,
                                &agent_resolution_cache,
                                &route_resolver,
                                &*auth_service,
                            )
                            .await;
                        }
                        Err(e) => {
                            warn!("Error receiving registry event: {e}, reconnecting");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect to registry invalidation stream: {e}");
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn dispatch_event(
    event: &RegistryInvalidationEvent,
    agent_resolution_cache: &AgentResolutionCache,
    route_resolver: &RouteResolver,
    auth_service: &dyn AuthService,
) {
    match event {
        RegistryInvalidationEvent::CursorExpired { .. } => {
            warn!("Registry invalidation cursor expired, flushing all caches");
            agent_resolution_cache.clear().await;
            route_resolver.clear_all().await;
            auth_service.clear_all_caches().await;
        }
        RegistryInvalidationEvent::DeploymentChanged {
            environment_id,
            deployment_revision,
            current_deployment_revision,
            ..
        } => {
            debug!(
                environment_id = %environment_id,
                deployment_revision = deployment_revision,
                current_deployment_revision = current_deployment_revision,
                "Received deployment changed event"
            );
            if let (Ok(rev), Ok(current_rev)) = (
                DeploymentRevision::new(*deployment_revision),
                CurrentDeploymentRevision::new(*current_deployment_revision),
            ) {
                agent_resolution_cache.update_latest_revision(*environment_id, rev, current_rev);
            }
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
            for domain_str in domains {
                let domain = Domain(domain_str.clone());
                route_resolver.invalidate_domain(&domain).await;
            }
        }
        RegistryInvalidationEvent::AccountTokensInvalidated { account_id, .. } => {
            debug!(
                account_id = %account_id,
                "Received account tokens invalidated event"
            );
            auth_service
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
            auth_service
                .invalidate_environment_auth(*environment_id, *grantee_account_id)
                .await;
        }
        RegistryInvalidationEvent::SecuritySchemeChanged { environment_id, .. } => {
            debug!(
                environment_id = %environment_id,
                "Received security scheme changed event, invalidating route cache"
            );
            route_resolver
                .invalidate_domains_for_environment(*environment_id)
                .await;
        }
        RegistryInvalidationEvent::ResourceDefinitionChanged { .. } => {}
    }
}
