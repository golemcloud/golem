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
use crate::service::agent_types::AgentTypeResolver;
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
    agent_types: Arc<AgentTypeResolver>,
    route_resolver: Arc<RouteResolver>,
    auth_service: Arc<dyn AuthService>,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
}

impl WorkerServiceRegistryInvalidationHandler {
    pub async fn run(
        registry_service: Arc<dyn RegistryService>,
        component_service: Arc<dyn ComponentService>,
        agent_resolution_cache: Arc<AgentResolutionCache>,
        agent_types: Arc<AgentTypeResolver>,
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
                    agent_types,
                    route_resolver,
                    auth_service,
                    mcp_capability_lookup,
                }),
            )
            .await;
    }
}

#[async_trait::async_trait]
#[golem_async_trait::region_simple]
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
                self.agent_types.clear().await;
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
                self.agent_types
                    .invalidate_environment(*environment_id)
                    .await;
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
                self.agent_resolution_cache
                    .invalidate_by_account(*account_id)
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
                self.agent_resolution_cache
                    .invalidate_by_account(*grantee_account_id)
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
                    self.agent_types.invalidate_environment(*env_id).await;
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
                self.agent_types
                    .invalidate_environment(*environment_id)
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
    use crate::service::agent_types::tests::registered;
    use crate::service::auth::RemoteAuthService;
    use crate::service::component::RemoteComponentService;
    use crate::service::component::tests::{TestRegistry, component};
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use std::time::Duration;
    use test_r::test;

    fn handler(registry: Arc<TestRegistry>) -> WorkerServiceRegistryInvalidationHandler {
        WorkerServiceRegistryInvalidationHandler {
            component_service: Arc::new(RemoteComponentService::new(
                registry.clone(),
                &ComponentServiceConfig::default(),
            )),
            agent_resolution_cache: Arc::new(AgentResolutionCache::new(
                registry.clone(),
                10,
                Duration::from_secs(60),
                Duration::from_secs(60),
            )),
            agent_types: Arc::new(AgentTypeResolver::new(
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
            mcp_capability_lookup: Arc::new(RegistryServiceMcpCapabilityLookup::new(registry)),
        }
    }

    fn user_auth(
        account_id: golem_common::model::account::AccountId,
        allow_view: bool,
    ) -> golem_service_base::model::auth::AuthCtx {
        use golem_common::model::account::AccountEmail;
        use golem_common::model::card::owner::AgentOwnerPattern;
        use golem_common::model::card::{
            AgentResourcePattern, AgentVerb, ClassPermissionTarget, EffectiveSurface, GrantSurface,
            PermissionTarget,
        };
        use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
        let surface = EffectiveSurface {
            lower: if allow_view {
                vec![GrantSurface {
                    positive: vec![PermissionTarget::Agent(ClassPermissionTarget {
                        owner: AgentOwnerPattern::AnyAgents,
                        verb: Some(AgentVerb::View),
                        resource: AgentResourcePattern::Any,
                    })],
                    negative: vec![],
                }]
            } else {
                vec![]
            },
            ..Default::default()
        };
        AuthCtx::User(UserAuthCtx {
            account_id,
            account_email: AccountEmail::new("caller@test.com"),
            account_plan_id: golem_common::model::plan::PlanId::new(),
            account_roles: Default::default(),
            effective_surface: surface,
            delegation_surface: None,
        })
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn authentication_grant_does_not_join_pre_event_denial() {
        use crate::service::component::tests::LookupGate;
        use golem_common::model::account::{AccountEmail, AccountId};
        use golem_common::model::auth::TokenSecret;
        use golem_common::model::plan::PlanId;
        use golem_service_base::model::auth::AuthCtx;

        let registry = Arc::new(TestRegistry::new(vec![], None));
        let handler = handler(registry.clone());
        let actor = AccountId::new();
        let token = TokenSecret::new();
        // An existing impersonation token can fail authentication while its actor lacks
        // ImpersonateUser permission, then succeed after that permission is restored.
        assert!(
            handler
                .auth_service
                .authenticate_token(token.clone())
                .await
                .is_err()
        );
        let gate = LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(gate.clone());
        let old = handler.auth_service.authenticate_token(token.clone());
        tokio::pin!(old);
        assert!(futures::poll!(&mut old).is_pending());
        gate.started.acquire().await.unwrap().forget();

        let granted = AuthCtx::admin_impersonation(
            actor,
            AccountId::new(),
            AccountEmail::new("target@test.com"),
            Default::default(),
            PlanId::new(),
            Default::default(),
        );
        registry
            .auth_contexts
            .lock()
            .unwrap()
            .insert(token.clone(), granted.clone());
        handler
            .on_event(RegistryInvalidationEvent::AccountTokensInvalidated {
                event_id: 1,
                account_id: actor,
            })
            .await;
        let fresh = handler.auth_service.authenticate_token(token.clone());
        tokio::pin!(fresh);
        let immediate = futures::poll!(&mut fresh);
        gate.release.add_permits(1);
        let _ = old.await;
        let result = match immediate {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => fresh.await,
        };
        // Control: the exact same token authenticates once the obsolete pending entry is gone.
        assert_eq!(
            handler
                .auth_service
                .authenticate_token(token)
                .await
                .unwrap(),
            granted
        );
        assert!(
            result.is_ok(),
            "post-event authentication inherited the pre-event denial"
        );
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn authentication_misses_coalesce_across_unrelated_invalidation() {
        use crate::service::component::tests::LookupGate;
        use golem_common::model::account::AccountId;
        use golem_common::model::auth::TokenSecret;
        use std::sync::atomic::Ordering;

        let registry = Arc::new(TestRegistry::new(vec![], None));
        let handler = handler(registry.clone());
        let token = TokenSecret::new();
        let auth = user_auth(AccountId::new(), true);
        registry
            .auth_contexts
            .lock()
            .unwrap()
            .insert(token.clone(), auth.clone());
        let gate = LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(gate.clone());
        let first = handler.auth_service.authenticate_token(token.clone());
        tokio::pin!(first);
        assert!(futures::poll!(&mut first).is_pending());
        gate.started.acquire().await.unwrap().forget();
        let second = handler.auth_service.authenticate_token(token.clone());
        tokio::pin!(second);
        assert!(futures::poll!(&mut second).is_pending());
        assert_eq!(registry.auth_reads.load(Ordering::SeqCst), 1);

        handler
            .on_event(RegistryInvalidationEvent::AccountTokensInvalidated {
                event_id: 1,
                account_id: AccountId::new(),
            })
            .await;
        gate.release.add_permits(1);
        assert_eq!(first.await.unwrap(), auth);
        assert_eq!(second.await.unwrap(), auth);
        registry.auth_contexts.lock().unwrap().remove(&token);
        assert_eq!(
            handler
                .auth_service
                .authenticate_token(token)
                .await
                .unwrap(),
            auth
        );
        assert_eq!(registry.auth_reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn granted_caller_joining_old_denial() {
        use crate::service::component::tests::LookupGate;
        use golem_common::model::account::AccountId;

        for pinned in [false, true] {
            let deployed = component();
            let registry = Arc::new(TestRegistry::new(vec![], Some(deployed.clone())));
            let handler = handler(registry.clone());
            let name = AgentTypeName("Cart".into());
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 0));
            let account = AccountId::new();
            let denied = user_auth(account, false);
            let allowed = user_auth(account, true);
            let resolve = |auth| {
                let cache = handler.agent_resolution_cache.clone();
                let deployed = deployed.clone();
                let name = name.clone();
                async move {
                    if pinned {
                        cache
                            .resolve_pinned(
                                &deployed.application_name,
                                &deployed.environment_name,
                                &name,
                                DeploymentRevision::INITIAL,
                                None,
                                &auth,
                            )
                            .await
                    } else {
                        cache
                            .resolve(
                                &deployed.application_name,
                                &deployed.environment_name,
                                &name,
                                None,
                                &auth,
                            )
                            .await
                    }
                }
            };
            // Controls: the registry denies the old surface and accepts the granted surface.
            assert!(resolve(denied.clone()).await.is_err());
            assert!(resolve(allowed.clone()).await.is_ok());
            handler.agent_resolution_cache.clear().await;
            let gate = LookupGate::new();
            *registry.next_lookup.lock().unwrap() = Some(gate.clone());
            let old = resolve(denied);
            tokio::pin!(old);
            assert!(futures::poll!(&mut old).is_pending());
            gate.started.acquire().await.unwrap().forget();
            handler
                .on_event(RegistryInvalidationEvent::AccountTokensInvalidated {
                    event_id: 1,
                    account_id: account,
                })
                .await;
            let fresh = resolve(allowed.clone());
            tokio::pin!(fresh);
            // Poll once to ensure the post-event request joins the old pending entry.
            let immediate = futures::poll!(&mut fresh);
            gate.release.add_permits(1);
            assert!(old.await.is_err());
            let result = match immediate {
                std::task::Poll::Ready(result) => result,
                std::task::Poll::Pending => fresh.await,
            };
            // A subsequent call with exactly the same auth succeeds; the joined call must too.
            assert!(resolve(allowed).await.is_ok());
            assert!(
                result.is_ok(),
                "post-event granted caller inherited pre-event denial: {:?}",
                result.err()
            );
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn resolution_misses_coalesce_across_unrelated_invalidation() {
        use crate::service::component::tests::LookupGate;
        use golem_common::model::account::AccountId;
        use std::sync::atomic::Ordering;

        for pinned in [false, true] {
            let deployed = component();
            let registry = Arc::new(TestRegistry::new(vec![], Some(deployed.clone())));
            let handler = handler(registry.clone());
            let name = AgentTypeName("Cart".into());
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 0));
            let auth = user_auth(AccountId::new(), true);
            let gate = LookupGate::new();
            *registry.next_lookup.lock().unwrap() = Some(gate.clone());
            let resolve = || async {
                if pinned {
                    handler
                        .agent_resolution_cache
                        .resolve_pinned(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            DeploymentRevision::INITIAL,
                            None,
                            &auth,
                        )
                        .await
                } else {
                    handler
                        .agent_resolution_cache
                        .resolve(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            None,
                            &auth,
                        )
                        .await
                }
            };
            let first = resolve();
            tokio::pin!(first);
            assert!(futures::poll!(&mut first).is_pending());
            gate.started.acquire().await.unwrap().forget();
            let second = resolve();
            tokio::pin!(second);
            assert!(futures::poll!(&mut second).is_pending());
            assert_eq!(registry.resolution_reads.load(Ordering::SeqCst), 1);
            handler
                .on_event(RegistryInvalidationEvent::AccountTokensInvalidated {
                    event_id: 1,
                    account_id: AccountId::new(),
                })
                .await;
            gate.release.add_permits(1);
            assert_eq!(first.await.unwrap().environment_id, deployed.environment_id);
            assert_eq!(
                second.await.unwrap().environment_id,
                deployed.environment_id
            );
            assert_eq!(
                resolve().await.unwrap().environment_id,
                deployed.environment_id
            );
            assert_eq!(registry.resolution_reads.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    async fn permission_events_refresh_auth_and_both_resolution_caches() {
        use golem_common::model::account::AccountId;
        use golem_common::model::auth::TokenSecret;
        use golem_service_base::model::auth::AuthCtx;

        let account = AccountId::new();
        let deployed = component();
        for event in [
            RegistryInvalidationEvent::AccountTokensInvalidated {
                event_id: 1,
                account_id: account,
            },
            RegistryInvalidationEvent::EnvironmentPermissionsChanged {
                event_id: 2,
                environment_id: deployed.environment_id,
                grantee_account_id: account,
            },
            RegistryInvalidationEvent::CursorExpired { event_id: 3 },
        ] {
            let registry = Arc::new(TestRegistry::new(vec![], Some(deployed.clone())));
            let handler = handler(registry.clone());
            let name = AgentTypeName("Cart".into());
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 0));
            let user = user_auth(account, true);
            let AuthCtx::User(user_ctx) = &user else {
                unreachable!()
            };
            let impersonated = AuthCtx::admin_impersonation(
                AccountId::new(),
                account,
                user_ctx.account_email.clone(),
                Default::default(),
                user_ctx.account_plan_id,
                user_ctx.effective_surface.clone(),
            );
            let affected = [
                (TokenSecret::new(), user.clone()),
                (TokenSecret::new(), user),
                (TokenSecret::new(), impersonated),
            ];
            let other_token = TokenSecret::new();
            let other = user_auth(AccountId::new(), true);
            registry
                .auth_contexts
                .lock()
                .unwrap()
                .extend(affected.clone());
            registry
                .auth_contexts
                .lock()
                .unwrap()
                .insert(other_token.clone(), other.clone());
            for (token, _) in affected
                .iter()
                .chain(std::iter::once(&(other_token.clone(), other.clone())))
            {
                let auth = handler
                    .auth_service
                    .authenticate_token(token.clone())
                    .await
                    .unwrap();
                handler
                    .agent_resolution_cache
                    .resolve(
                        &deployed.application_name,
                        &deployed.environment_name,
                        &name,
                        None,
                        &auth,
                    )
                    .await
                    .unwrap();
                handler
                    .agent_resolution_cache
                    .resolve_pinned(
                        &deployed.application_name,
                        &deployed.environment_name,
                        &name,
                        DeploymentRevision::INITIAL,
                        None,
                        &auth,
                    )
                    .await
                    .unwrap();
            }

            for (token, auth) in &affected {
                let mut restricted = auth.clone();
                match &mut restricted {
                    AuthCtx::User(ctx) => ctx.effective_surface = Default::default(),
                    AuthCtx::AdminImpersonation(ctx) => ctx.effective_surface = Default::default(),
                    _ => unreachable!(),
                }
                registry
                    .auth_contexts
                    .lock()
                    .unwrap()
                    .insert(token.clone(), restricted);
            }
            registry.auth_contexts.lock().unwrap().remove(&other_token);
            handler.on_event(event.clone()).await;

            for (token, _) in &affected {
                let expected = registry
                    .auth_contexts
                    .lock()
                    .unwrap()
                    .get(token)
                    .cloned()
                    .unwrap();
                let auth = handler
                    .auth_service
                    .authenticate_token(token.clone())
                    .await
                    .unwrap();
                assert_eq!(auth, expected);
                assert!(
                    handler
                        .agent_resolution_cache
                        .resolve(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            None,
                            &auth,
                        )
                        .await
                        .is_err()
                );
                assert!(
                    handler
                        .agent_resolution_cache
                        .resolve_pinned(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            DeploymentRevision::INITIAL,
                            None,
                            &auth,
                        )
                        .await
                        .is_err()
                );
            }
            if matches!(event, RegistryInvalidationEvent::CursorExpired { .. }) {
                assert!(
                    handler
                        .auth_service
                        .authenticate_token(other_token)
                        .await
                        .is_err()
                );
            } else {
                assert_eq!(
                    handler
                        .auth_service
                        .authenticate_token(other_token)
                        .await
                        .unwrap(),
                    other
                );
                assert!(
                    handler
                        .agent_resolution_cache
                        .resolve(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            None,
                            &other,
                        )
                        .await
                        .is_ok()
                );
                assert!(
                    handler
                        .agent_resolution_cache
                        .resolve_pinned(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            DeploymentRevision::INITIAL,
                            None,
                            &other,
                        )
                        .await
                        .is_ok()
                );
            }
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn permission_invalidation_drops_in_flight_authentication_results() {
        use crate::service::component::tests::LookupGate;
        use golem_common::model::account::AccountId;
        use golem_common::model::auth::TokenSecret;

        let registry = Arc::new(TestRegistry::new(vec![], None));
        let handler = handler(registry.clone());
        let account = AccountId::new();
        let token = TokenSecret::new();
        registry
            .auth_contexts
            .lock()
            .unwrap()
            .insert(token.clone(), user_auth(account, true));
        let gate = LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(gate.clone());
        let lookup = tokio::spawn({
            let auth = handler.auth_service.clone();
            let token = token.clone();
            async move { auth.authenticate_token(token).await }
        });
        gate.started.acquire().await.unwrap().forget();
        let updated = user_auth(account, false);
        registry
            .auth_contexts
            .lock()
            .unwrap()
            .insert(token.clone(), updated.clone());
        let invalidation = handler.on_event(RegistryInvalidationEvent::AccountTokensInvalidated {
            event_id: 1,
            account_id: account,
        });
        invalidation.await;
        gate.release.add_permits(1);
        lookup.await.unwrap().unwrap();
        assert_eq!(
            handler
                .auth_service
                .authenticate_token(token)
                .await
                .unwrap(),
            updated
        );
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn permission_invalidation_drops_in_flight_resolution_results() {
        use crate::service::component::tests::LookupGate;
        use golem_common::model::account::AccountId;

        for pinned in [false, true] {
            let deployed = component();
            let registry = Arc::new(TestRegistry::new(vec![], Some(deployed.clone())));
            let handler = handler(registry.clone());
            let name = AgentTypeName("Cart".into());
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 0));
            let account = AccountId::new();
            let auth = user_auth(account, true);
            let gate = LookupGate::new();
            *registry.next_lookup.lock().unwrap() = Some(gate.clone());
            let lookup = tokio::spawn({
                let cache = handler.agent_resolution_cache.clone();
                let deployed = deployed.clone();
                let name = name.clone();
                async move {
                    if pinned {
                        cache
                            .resolve_pinned(
                                &deployed.application_name,
                                &deployed.environment_name,
                                &name,
                                DeploymentRevision::INITIAL,
                                None,
                                &auth,
                            )
                            .await
                    } else {
                        cache
                            .resolve(
                                &deployed.application_name,
                                &deployed.environment_name,
                                &name,
                                None,
                                &auth,
                            )
                            .await
                    }
                }
            });
            gate.started.acquire().await.unwrap().forget();
            let invalidation =
                handler.on_event(RegistryInvalidationEvent::AccountTokensInvalidated {
                    event_id: 1,
                    account_id: account,
                });
            invalidation.await;
            let restricted = user_auth(account, false);
            let result = async {
                if pinned {
                    handler
                        .agent_resolution_cache
                        .resolve_pinned(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            DeploymentRevision::INITIAL,
                            None,
                            &restricted,
                        )
                        .await
                } else {
                    handler
                        .agent_resolution_cache
                        .resolve(
                            &deployed.application_name,
                            &deployed.environment_name,
                            &name,
                            None,
                            &restricted,
                        )
                        .await
                }
            };
            tokio::pin!(result);
            assert!(futures::poll!(&mut result).is_pending());
            gate.release.add_permits(1);
            lookup.await.unwrap().unwrap();
            assert!(result.await.is_err());
        }
    }

    #[test]
    async fn deployment_cursor_and_deletion_events_refresh_current_metadata_and_agent_types() {
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
        let agent_types = Arc::new(AgentTypeResolver::new(
            registry.clone(),
            10,
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let handler = WorkerServiceRegistryInvalidationHandler {
            component_service: components.clone(),
            agent_resolution_cache: Arc::new(AgentResolutionCache::new(
                registry.clone(),
                10,
                Duration::from_secs(60),
                Duration::from_secs(60),
            )),
            agent_types: agent_types.clone(),
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
        let name = AgentTypeName("Cart".to_string());
        let deployed_revision = async || {
            agent_types
                .deployed(a.environment_id, &name)
                .await
                .unwrap()
                .map(|registered| registered.implemented_by.component_revision)
        };
        for event in events {
            let event_name = format!("{event:?}");
            *registry.deployed.lock().unwrap() = Some(a.clone());
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 1));
            components.invalidate_current().await;
            agent_types.invalidate_environment(a.environment_id).await;
            assert_eq!(
                components.get_current_by_id(a.id).await.unwrap().revision,
                a.revision
            );
            assert_eq!(deployed_revision().await, ComponentRevision::new(1).ok());

            *registry.deployed.lock().unwrap() = Some(b.clone());
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 2));
            handler.on_event(event).await;
            assert_eq!(
                components.get_current_by_id(a.id).await.unwrap().revision,
                b.revision,
                "{event_name}"
            );
            assert_eq!(
                deployed_revision().await,
                ComponentRevision::new(2).ok(),
                "{event_name}"
            );
        }
    }
}
