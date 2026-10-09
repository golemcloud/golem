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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::local_observability::LocalObservabilityClient;
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use golem_client::api::{
    AccountClient, AgentSecretsClient, ApiDomainClient, ApiSecurityClient, CardClient,
    EnvironmentToolGrantsClient, EnvironmentToolMiddlewareGrantsClient, PluginClient,
    ResourcesClient, RetryPoliciesClient, ToolMiddlewareReleasesClient, ToolReleasesClient,
};
use golem_client::model::StoredCard;
use golem_common::model::account_usage::{MemoryLimit, StorageLimit};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::tui) enum ResourceScope {
    Environment,
    Account,
    Server,
}

impl ResourceScope {
    pub(in crate::tui) const ALL: [Self; 3] = [Self::Environment, Self::Account, Self::Server];

    pub(in crate::tui) const fn label(self) -> &'static str {
        match self {
            Self::Environment => "Environment",
            Self::Account => "Account",
            Self::Server => "Server",
        }
    }

    pub(in crate::tui) fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub(in crate::tui) fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|scope| *scope == self)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::tui) enum ResourceKind {
    Secrets,
    RetryPolicies,
    ResourceDefinitions,
    Domains,
    SecuritySchemes,
    PluginGrants,
    ToolGrants,
    MiddlewareGrants,
    Identity,
    CurrentUsage,
    UsageHistory,
    Limits,
    OwnedShares,
    ReceivedShares,
    ApiTokens,
    Plugins,
    ToolReleases,
    MiddlewareReleases,
    Cards,
    Manifest,
    SessionOverrides,
    EndpointPorts,
    StoragePaths,
    Memory,
    Observability,
}

impl ResourceKind {
    const ENVIRONMENT: [Self; 8] = [
        Self::Secrets,
        Self::RetryPolicies,
        Self::ResourceDefinitions,
        Self::Domains,
        Self::SecuritySchemes,
        Self::PluginGrants,
        Self::ToolGrants,
        Self::MiddlewareGrants,
    ];
    const ACCOUNT: [Self; 11] = [
        Self::Identity,
        Self::CurrentUsage,
        Self::UsageHistory,
        Self::Limits,
        Self::OwnedShares,
        Self::ReceivedShares,
        Self::ApiTokens,
        Self::Plugins,
        Self::ToolReleases,
        Self::MiddlewareReleases,
        Self::Cards,
    ];
    const SERVER: [Self; 6] = [
        Self::Manifest,
        Self::SessionOverrides,
        Self::EndpointPorts,
        Self::StoragePaths,
        Self::Memory,
        Self::Observability,
    ];

    pub(in crate::tui) const fn for_scope(scope: ResourceScope) -> &'static [Self] {
        match scope {
            ResourceScope::Environment => &Self::ENVIRONMENT,
            ResourceScope::Account => &Self::ACCOUNT,
            ResourceScope::Server => &Self::SERVER,
        }
    }

    pub(in crate::tui) const fn scope(self) -> ResourceScope {
        match self {
            Self::Secrets
            | Self::RetryPolicies
            | Self::ResourceDefinitions
            | Self::Domains
            | Self::SecuritySchemes
            | Self::PluginGrants
            | Self::ToolGrants
            | Self::MiddlewareGrants => ResourceScope::Environment,
            Self::Identity
            | Self::CurrentUsage
            | Self::UsageHistory
            | Self::Limits
            | Self::OwnedShares
            | Self::ReceivedShares
            | Self::ApiTokens
            | Self::Plugins
            | Self::ToolReleases
            | Self::MiddlewareReleases
            | Self::Cards => ResourceScope::Account,
            Self::Manifest
            | Self::SessionOverrides
            | Self::EndpointPorts
            | Self::StoragePaths
            | Self::Memory
            | Self::Observability => ResourceScope::Server,
        }
    }

    pub(in crate::tui) const fn label(self) -> &'static str {
        match self {
            Self::Secrets => "Secrets",
            Self::RetryPolicies => "Retry Policies",
            Self::ResourceDefinitions => "Resource Definitions",
            Self::Domains => "Domains",
            Self::SecuritySchemes => "Security Schemes",
            Self::PluginGrants => "Plugin Grants",
            Self::ToolGrants => "Tool Grants",
            Self::MiddlewareGrants => "Middleware Grants",
            Self::Identity => "Identity & Roles",
            Self::CurrentUsage => "Current Usage",
            Self::UsageHistory => "Usage History",
            Self::Limits => "Effective Limits",
            Self::OwnedShares => "Owned Shares",
            Self::ReceivedShares => "Received Shares",
            Self::ApiTokens => "API Tokens",
            Self::Plugins => "Owned Plugins",
            Self::ToolReleases => "Tool Releases",
            Self::MiddlewareReleases => "Middleware Releases",
            Self::Cards => "Cards",
            Self::Manifest => "Manifest",
            Self::SessionOverrides => "Session Overrides",
            Self::EndpointPorts => "Endpoint & Ports",
            Self::StoragePaths => "Storage Paths",
            Self::Memory => "Memory",
            Self::Observability => "Observability",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::tui) enum ResourceFocus {
    #[default]
    Kinds,
    Items,
    Details,
}

impl ResourceFocus {
    const fn next(self) -> Self {
        match self {
            Self::Kinds => Self::Items,
            Self::Items => Self::Details,
            Self::Details => Self::Kinds,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Kinds => Self::Details,
            Self::Items => Self::Kinds,
            Self::Details => Self::Items,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct ResourceField {
    pub(in crate::tui) label: String,
    pub(in crate::tui) value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct ResourceRow {
    pub(in crate::tui) name: String,
    pub(in crate::tui) summary: String,
    pub(in crate::tui) fields: Vec<ResourceField>,
}

impl ResourceRow {
    fn new(
        name: impl Into<String>,
        summary: impl Into<String>,
        fields: Vec<ResourceField>,
    ) -> Self {
        Self {
            name: name.into(),
            summary: summary.into(),
            fields,
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::new("Unavailable", reason.clone(), vec![field("Reason", reason)])
    }
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ResourcesRefreshPayload {
    pub(in crate::tui) scope: ResourceScope,
    pub(in crate::tui) kind: ResourceKind,
    pub(in crate::tui) rows: Vec<ResourceRow>,
}

#[derive(Clone, Debug, Default)]
struct ResourceKindCache {
    rows: Vec<ResourceRow>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct ResourceScopeState {
    selected_kind: usize,
    selected_row: usize,
    focus: ResourceFocus,
    details_scroll: u16,
    cache: BTreeMap<ResourceKind, ResourceKindCache>,
}

pub(in crate::tui) struct ResourcesState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<super::super::context_executor::TuiContextId>,
    pub(in crate::tui) loading: Option<ResourceKind>,
    pub(in crate::tui) scope: ResourceScope,
    scopes: BTreeMap<ResourceScope, ResourceScopeState>,
}

impl Default for ResourcesState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: None,
            scope: ResourceScope::Environment,
            scopes: ResourceScope::ALL
                .into_iter()
                .map(|scope| (scope, ResourceScopeState::default()))
                .collect(),
        }
    }
}

impl ResourcesState {
    pub(in crate::tui) fn invalidate(&mut self) {
        let generation = self.generation.saturating_add(1);
        *self = Self::default();
        self.generation = generation;
    }

    fn current(&self) -> &ResourceScopeState {
        &self.scopes[&self.scope]
    }

    fn current_mut(&mut self) -> &mut ResourceScopeState {
        self.scopes.get_mut(&self.scope).expect("initialized scope")
    }

    pub(in crate::tui) fn selected_kind(&self) -> ResourceKind {
        let kinds = ResourceKind::for_scope(self.scope);
        kinds[self.current().selected_kind.min(kinds.len() - 1)]
    }

    pub(in crate::tui) fn selected_kind_index(&self) -> usize {
        self.current().selected_kind
    }

    pub(in crate::tui) fn selected_row_index(&self) -> usize {
        self.current().selected_row
    }

    pub(in crate::tui) fn focus(&self) -> ResourceFocus {
        self.current().focus
    }

    pub(in crate::tui) fn cycle_focus(&mut self, forward: bool) {
        let state = self.current_mut();
        state.focus = if forward {
            state.focus.next()
        } else {
            state.focus.previous()
        };
    }

    pub(in crate::tui) fn visible_rows(&self) -> &[ResourceRow] {
        self.current()
            .cache
            .get(&self.selected_kind())
            .map(|cache| cache.rows.as_slice())
            .unwrap_or_default()
    }

    pub(in crate::tui) fn current_error(&self) -> Option<&str> {
        self.kind_error(self.selected_kind())
    }

    pub(in crate::tui) fn kind_error(&self, kind: ResourceKind) -> Option<&str> {
        self.current()
            .cache
            .get(&kind)
            .and_then(|cache| cache.error.as_deref())
    }

    pub(in crate::tui) fn kind_count(&self, kind: ResourceKind) -> Option<usize> {
        self.current()
            .cache
            .get(&kind)
            .filter(|cache| cache.error.is_none())
            .map(|cache| cache.rows.len())
    }

    pub(in crate::tui) fn is_loaded(&self, kind: ResourceKind) -> bool {
        self.current().cache.contains_key(&kind)
    }

    pub(in crate::tui) fn select_scope(&mut self, scope: ResourceScope) {
        self.scope = scope;
        self.loading = None;
    }

    pub(in crate::tui) fn move_selection(&mut self, down: bool) -> bool {
        let scope = self.scope;
        let kind = self.selected_kind();
        let state = self.current_mut();
        match state.focus {
            ResourceFocus::Kinds => {
                let old = state.selected_kind;
                let last = ResourceKind::for_scope(scope).len().saturating_sub(1);
                state.selected_kind = if down {
                    state.selected_kind.saturating_add(1).min(last)
                } else {
                    state.selected_kind.saturating_sub(1)
                };
                state.selected_row = 0;
                state.details_scroll = 0;
                old != state.selected_kind
            }
            ResourceFocus::Items => {
                let last = state
                    .cache
                    .get(&kind)
                    .map(|cache| cache.rows.len().saturating_sub(1))
                    .unwrap_or(0);
                state.selected_row = if down {
                    state.selected_row.saturating_add(1).min(last)
                } else {
                    state.selected_row.saturating_sub(1)
                };
                state.details_scroll = 0;
                false
            }
            ResourceFocus::Details => {
                state.details_scroll = if down {
                    state.details_scroll.saturating_add(1)
                } else {
                    state.details_scroll.saturating_sub(1)
                };
                false
            }
        }
    }

    pub(in crate::tui) fn details_scroll(&self) -> u16 {
        self.current().details_scroll
    }

    pub(in crate::tui) fn begin_load(
        &mut self,
        context_id: super::super::context_executor::TuiContextId,
    ) -> (u64, ResourceScope, ResourceKind) {
        self.generation = self.generation.saturating_add(1);
        self.context_id = Some(context_id);
        let kind = self.selected_kind();
        self.loading = Some(kind);
        self.current_mut().cache.remove(&kind);
        (self.generation, self.scope, kind)
    }

    pub(in crate::tui) fn finish(
        &mut self,
        generation: u64,
        context_id: super::super::context_executor::TuiContextId,
        result: anyhow::Result<ResourcesRefreshPayload>,
    ) {
        if generation != self.generation || Some(context_id) != self.context_id {
            return;
        }
        let expected = self.loading.take();
        match result {
            Ok(payload) if Some(payload.kind) == expected && payload.scope == self.scope => {
                let state = self.current_mut();
                let len = payload.rows.len();
                state.cache.insert(
                    payload.kind,
                    ResourceKindCache {
                        rows: payload.rows,
                        error: None,
                    },
                );
                state.selected_row = state.selected_row.min(len.saturating_sub(1));
            }
            Ok(_) => {}
            Err(error) => {
                if let Some(kind) = expected {
                    self.current_mut().cache.insert(
                        kind,
                        ResourceKindCache {
                            rows: Vec::new(),
                            error: Some(error.to_string()),
                        },
                    );
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::tui) struct ServerResourceSnapshot {
    pub(in crate::tui) status: String,
    pub(in crate::tui) command: Option<String>,
    pub(in crate::tui) router_port: Option<u16>,
    pub(in crate::tui) custom_request_port: Option<u16>,
    pub(in crate::tui) mcp_port: Option<u16>,
    pub(in crate::tui) exit_code: Option<i32>,
}

pub(in crate::tui) async fn load(
    context: Arc<Context>,
    environment_reference: Option<&EnvironmentReference>,
    scope: ResourceScope,
    kind: ResourceKind,
    server: ServerResourceSnapshot,
    refresh: bool,
) -> anyhow::Result<ResourcesRefreshPayload> {
    debug_assert_eq!(kind.scope(), scope);
    let rows = match scope {
        ResourceScope::Environment => {
            let environment = context
                .environment_handler()
                .resolve_opt_environment_reference_read_only(
                    EnvironmentResolveMode::Any,
                    environment_reference,
                )
                .await?;
            load_environment_kind(&context, &environment, kind, refresh).await?
        }
        ResourceScope::Account => load_account_kind(&context, kind).await?,
        ResourceScope::Server if kind == ResourceKind::Observability => {
            load_server_observability(&context).await?
        }
        ResourceScope::Server => load_server_kind(&context, kind, &server),
    };
    Ok(ResourcesRefreshPayload { scope, kind, rows })
}

async fn load_environment_kind(
    context: &Arc<Context>,
    environment: &ResolvedEnvironmentIdentity,
    kind: ResourceKind,
    refresh: bool,
) -> anyhow::Result<Vec<ResourceRow>> {
    let clients = context.golem_clients().await?;
    let environment_id = &environment.environment_id.0;
    let rows = match kind {
        ResourceKind::Secrets => clients
            .agent_secrets
            .list_environment_agent_secrets(environment_id, false)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|secret| {
                ResourceRow::new(
                    secret.path.to_string(),
                    format!("revision {}", secret.revision),
                    vec![
                        field("Path", secret.path),
                        field("ID", secret.id),
                        field("Revision", secret.revision),
                        field("Value", "Masked — secret values are never requested"),
                    ],
                )
            })
            .collect(),
        ResourceKind::RetryPolicies => clients
            .retry_policies
            .list_environment_retry_policies(environment_id)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|policy| {
                ResourceRow::new(
                    policy.name,
                    format!(
                        "priority {} · revision {}",
                        policy.priority, policy.revision
                    ),
                    vec![
                        field("ID", policy.id),
                        field("Revision", policy.revision),
                        field("Priority", policy.priority),
                        debug_field("Predicate", policy.predicate),
                        debug_field("Policy", policy.policy),
                    ],
                )
            })
            .collect(),
        ResourceKind::ResourceDefinitions => clients
            .resources
            .list_environment_resources(environment_id)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|resource| {
                ResourceRow::new(
                    resource.name.0.clone(),
                    format!("{} · revision {}", resource.units, resource.revision),
                    vec![
                        field("ID", resource.id),
                        field("Revision", resource.revision),
                        field("Unit", resource.unit),
                        field("Units", resource.units),
                        debug_field("Limit", resource.limit),
                        debug_field("Enforcement", resource.enforcement_action),
                    ],
                )
            })
            .collect(),
        ResourceKind::Domains => clients
            .api_domain
            .list_environment_domain_registrations(environment_id)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|domain| {
                ResourceRow::new(
                    domain.domain.0.clone(),
                    domain.id.to_string(),
                    vec![field("Domain", domain.domain), field("ID", domain.id)],
                )
            })
            .collect(),
        ResourceKind::SecuritySchemes => clients
            .api_security
            .list_environment_security_schemes(environment_id)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|scheme| {
                ResourceRow::new(
                    scheme.name.0.clone(),
                    format!("{:?} · revision {}", scheme.provider_type, scheme.revision),
                    vec![
                        field("ID", scheme.id),
                        field("Revision", scheme.revision),
                        debug_field("Provider", scheme.provider_type),
                        field("Client ID", scheme.client_id),
                        field("Redirect URL", scheme.redirect_url),
                        field("Scopes", scheme.scopes.join(", ")),
                        field("Client secret", "Masked — not returned by the list API"),
                    ],
                )
            })
            .collect(),
        ResourceKind::PluginGrants => {
            if refresh {
                context
                    .caches()
                    .plugin_grants
                    .remove(&environment.environment_id)
                    .await;
            }
            context
                .environment_handler()
                .plugin_grants(environment)
                .await?
                .into_values()
                .map(|grant| {
                    ResourceRow::new(
                        format!("{}@{}", grant.plugin.name, grant.plugin.version),
                        grant.plugin.description.clone(),
                        vec![
                            field("Grant ID", grant.id),
                            field("Plugin ID", grant.plugin.id),
                            field("Owner", grant.plugin_account.email),
                            field("Homepage", grant.plugin.homepage),
                            debug_field("Specification", grant.plugin.spec),
                        ],
                    )
                })
                .collect()
        }
        ResourceKind::ToolGrants => clients
            .environment_tool_grants
            .list_environment_tool_grants(environment_id)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|details| {
                let grant = details.grant;
                let release = details.release;
                ResourceRow::new(
                    format!("{}@{}", release.name, release.version),
                    format!("{:?}", grant.lifecycle),
                    vec![
                        field("Grant ID", grant.id),
                        field("Release ID", release.id),
                        field("Owner", details.release_owner.email),
                        debug_field("Lifecycle", grant.lifecycle),
                        field("Automatic", grant.automatic),
                        field("Protected", grant.protected),
                        field("Follow coordinates", grant.follow_coordinates),
                        field("Metadata version", release.metadata_version),
                        field("Metadata digest", release.metadata_digest),
                    ],
                )
            })
            .collect(),
        ResourceKind::MiddlewareGrants => clients
            .environment_tool_middleware_grants
            .list_environment_tool_middleware_grants(environment_id)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|details| {
                let grant = details.grant;
                let release = details.release;
                ResourceRow::new(
                    format!("{}@{}", release.name, release.version),
                    format!("{:?}", grant.lifecycle),
                    vec![
                        field("Grant ID", grant.id),
                        field("Release ID", release.id),
                        field("Owner", details.release_owner.email),
                        debug_field("Lifecycle", grant.lifecycle),
                        field("Automatic", grant.automatic),
                        field("Protected", grant.protected),
                        field("Follow coordinates", grant.follow_coordinates),
                        field("Metadata version", release.metadata_version),
                        field("Metadata digest", release.metadata_digest),
                    ],
                )
            })
            .collect(),
        _ => unreachable!("resource kind is scoped before loading"),
    };
    Ok(rows)
}

async fn load_account_kind(
    context: &Arc<Context>,
    kind: ResourceKind,
) -> anyhow::Result<Vec<ResourceRow>> {
    let clients = context.golem_clients().await?;
    let account_id = *clients.account_id();
    let rows = match kind {
        ResourceKind::Identity => {
            let account = clients
                .account
                .get_account(&account_id.0)
                .await
                .map_service_error()?;
            vec![ResourceRow::new(
                account.name.clone(),
                account.email.to_string(),
                vec![
                    field("Account ID", account.id),
                    field("Name", account.name),
                    field("Email", account.email),
                    field("Revision", account.revision),
                    field("Plan", account.plan_id),
                    field(
                        "Roles",
                        account
                            .roles
                            .into_iter()
                            .map(|role| format!("{role:?}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    field("Root card", account.account_root_card_id),
                ],
            )]
        }
        ResourceKind::CurrentUsage => {
            let usage = clients
                .account
                .get_account_usage(&account_id.0, None)
                .await
                .map_service_error()?;
            vec![usage_row(usage)]
        }
        ResourceKind::UsageHistory => clients
            .account
            .get_account_usage_history(&account_id.0, Some(12))
            .await
            .map_service_error()?
            .into_iter()
            .map(usage_row)
            .collect(),
        ResourceKind::Limits => {
            let storage = clients
                .account
                .get_account_storage_override(&account_id.0)
                .await
                .map_service_error()?;
            let max_memory = clients
                .account
                .get_account_max_memory_override(&account_id.0)
                .await
                .map_service_error()?;
            let monthly_memory = clients
                .account
                .get_account_monthly_memory_override(&account_id.0)
                .await
                .map_service_error()?;
            vec![
                storage_limit_row("Storage per agent", storage),
                memory_limit_row("Memory per agent", max_memory),
                memory_limit_row("Monthly memory", monthly_memory),
            ]
        }
        ResourceKind::OwnedShares | ResourceKind::ReceivedShares => {
            use golem_client::api::PermissionSharesClient;
            let shares = if kind == ResourceKind::OwnedShares {
                clients
                    .permission_shares
                    .list_owned_permission_shares(&account_id.0)
                    .await
                    .map_service_error()?
                    .values
            } else {
                clients
                    .permission_shares
                    .list_received_permission_shares(&account_id.0)
                    .await
                    .map_service_error()?
                    .values
            };
            shares
                .into_iter()
                .map(|share| {
                    ResourceRow::new(
                        share.name.0.clone(),
                        format!("revision {}", share.revision),
                        vec![
                            field("ID", share.id),
                            field("Revision", share.revision),
                            field("Owner", share.owner_account_id),
                            field("Recipient", share.target_account_id),
                            field("Lower allow", share.data.lower_positive.join(", ")),
                            field("Lower deny", share.data.lower_negative.join(", ")),
                            field("Upper allow", share.data.upper_positive.join(", ")),
                            field("Upper deny", share.data.upper_negative.join(", ")),
                        ],
                    )
                })
                .collect()
        }
        ResourceKind::ApiTokens => clients
            .account
            .list_account_tokens(&account_id.0)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|token| {
                ResourceRow::new(
                    token.id.to_string(),
                    format!("expires {}", token.expires_at),
                    vec![
                        field("Token ID", token.id),
                        field("Created", token.created_at),
                        field("Expires", token.expires_at),
                        field("Secret", "Masked — token secrets are never requested"),
                    ],
                )
            })
            .collect(),
        ResourceKind::Plugins => clients
            .plugin
            .list_account_plugins(&account_id.0)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|plugin| {
                ResourceRow::new(
                    format!("{}@{}", plugin.name, plugin.version),
                    plugin.description.clone(),
                    vec![
                        field("ID", plugin.id),
                        field("Homepage", plugin.homepage),
                        debug_field("Specification", plugin.spec),
                    ],
                )
            })
            .collect(),
        ResourceKind::ToolReleases => clients
            .tool_releases
            .list_account_tool_releases(&account_id.0)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|release| {
                ResourceRow::new(
                    format!("{}@{}", release.name, release.version),
                    format!("{:?} · {:?}", release.lifecycle, release.origin),
                    vec![
                        field("ID", release.id),
                        debug_field("Lifecycle", release.lifecycle),
                        debug_field("Origin", release.origin),
                        field("Immutable", release.immutable),
                        field("Metadata version", release.metadata_version),
                        field("Metadata digest", release.metadata_digest),
                        field("Created", release.created_at),
                        field("State changed", release.state_changed_at),
                    ],
                )
            })
            .collect(),
        ResourceKind::MiddlewareReleases => clients
            .tool_middleware_releases
            .list_account_tool_middleware_releases(&account_id.0)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .map(|release| {
                ResourceRow::new(
                    format!("{}@{}", release.name, release.version),
                    format!("{:?} · {:?}", release.lifecycle, release.origin),
                    vec![
                        field("ID", release.id),
                        debug_field("Lifecycle", release.lifecycle),
                        debug_field("Origin", release.origin),
                        field("Immutable", release.immutable),
                        field("Metadata version", release.metadata_version),
                        field("Metadata digest", release.metadata_digest),
                        field("Created", release.created_at),
                        field("State changed", release.state_changed_at),
                    ],
                )
            })
            .collect(),
        ResourceKind::Cards => clients
            .card
            .list_account_cards(
                &account_id.0,
                Some(true),
                Some(true),
                Some(true),
                Some(true),
            )
            .await
            .map_service_error()?
            .into_iter()
            .map(stored_card_row)
            .collect(),
        _ => unreachable!("resource kind is scoped before loading"),
    };
    Ok(rows)
}

fn stored_card_row(card: StoredCard) -> ResourceRow {
    match card {
        StoredCard::Concrete(card) => ResourceRow::new(
            card.card_id.to_string(),
            if card.system_card {
                "system concrete card"
            } else {
                "concrete card"
            },
            vec![
                field("Card ID", card.card_id),
                field("Parents", join_display(card.parent_ids)),
                field("Created", card.created_at),
                field(
                    "Expires",
                    card.expires_at
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "never".to_string()),
                ),
                field("System", card.system_card),
                debug_field("Managed by", card.managed_by),
                debug_field("Lower allow", card.lower_positive),
                debug_field("Lower deny", card.lower_negative),
                debug_field("Upper allow", card.upper_positive),
                debug_field("Upper deny", card.upper_negative),
            ],
        ),
        StoredCard::Polymorphic(card) => ResourceRow::new(
            card.card_id.to_string(),
            if card.system_card {
                "system polymorphic card"
            } else {
                "polymorphic card"
            },
            vec![
                field("Card ID", card.card_id),
                field("Parents", join_display(card.parent_ids)),
                field("Created", card.created_at),
                field(
                    "Expires",
                    card.expires_at
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "never".to_string()),
                ),
                field("System", card.system_card),
                debug_field("Lower allow", card.lower_positive),
                debug_field("Lower deny", card.lower_negative),
                debug_field("Upper allow", card.upper_positive),
                debug_field("Upper deny", card.upper_negative),
            ],
        ),
    }
}

fn load_server_kind(
    context: &Context,
    kind: ResourceKind,
    runtime: &ServerResourceSnapshot,
) -> Vec<ResourceRow> {
    let local = context.manifest_local_server();
    match kind {
        ResourceKind::Manifest => match (context.uses_local_server(), local) {
            (false, _) => {
                vec![ResourceRow::unavailable(
                    "The selected target is external and has no localServer manifest settings",
                )]
            }
            (true, None) => vec![ResourceRow::new(
                "Built-in local defaults",
                "no localServer section; defaults apply",
                vec![
                    field("Source", "No localServer section in the loaded manifest"),
                    field("Router address", "default"),
                    field("Router port", "default"),
                    field("Custom request port", "default"),
                    field("MCP port", "default"),
                    field("Observability", true),
                ],
            )],
            (true, Some(server)) => {
                vec![ResourceRow::new(
                    "Resolved localServer",
                    "manifest values after path resolution",
                    vec![
                        field(
                            "Source",
                            context
                                .manifest_local_server_source()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "Unavailable".to_string()),
                        ),
                        field(
                            "Router address",
                            optional(server.router_addr.as_deref(), "default"),
                        ),
                        field("Router port", optional_copy(server.router_port, "default")),
                        field(
                            "Custom request port",
                            optional_copy(server.custom_request_port, "default"),
                        ),
                        field("MCP port", optional_copy(server.mcp_port, "default")),
                        field("Observability", server.observability),
                    ],
                )]
            }
        },
        ResourceKind::SessionOverrides => {
            if runtime.command.is_none() {
                vec![ResourceRow::unavailable(
                    "No nested local server session has been started by this TUI",
                )]
            } else {
                vec![ResourceRow::new(
                    "Nested server session",
                    runtime.status.clone(),
                    vec![
                        field("Status", runtime.status.clone()),
                        field("Command", runtime.command.clone().unwrap_or_default()),
                        field(
                            "Exit code",
                            runtime
                                .exit_code
                                .map(|code| code.to_string())
                                .unwrap_or_else(|| "not exited".to_string()),
                        ),
                    ],
                )]
            }
        }
        ResourceKind::EndpointPorts => vec![ResourceRow::new(
            context.selected_server_description(),
            context.worker_service_url().to_string(),
            vec![
                field("Connected endpoint", context.worker_service_url()),
                field("Built-in local endpoint", context.builtin_local_url()),
                field("Allow insecure", context.allow_insecure()),
                field(
                    "Router port",
                    observed_port(
                        runtime.router_port,
                        local.and_then(|value| value.router_port),
                    ),
                ),
                field(
                    "Custom request port",
                    observed_port(
                        runtime.custom_request_port,
                        local.and_then(|value| value.custom_request_port),
                    ),
                ),
                field(
                    "MCP port",
                    observed_port(runtime.mcp_port, local.and_then(|value| value.mcp_port)),
                ),
            ],
        )],
        ResourceKind::StoragePaths => match (context.uses_local_server(), local) {
            (false, _) => {
                vec![ResourceRow::unavailable(
                    "Process-local paths are unavailable for an external server",
                )]
            }
            (true, None) => vec![ResourceRow::new(
                "Local storage defaults",
                "server-managed defaults",
                vec![
                    field("Data directory", "default"),
                    field("Agent filesystem root", "disabled"),
                    field("Ports file", "disabled"),
                ],
            )],
            (true, Some(server)) => {
                vec![ResourceRow::new(
                    "Local storage",
                    optional_path(server.data_dir.as_deref(), "default"),
                    vec![
                        field(
                            "Data directory",
                            optional_path(server.data_dir.as_deref(), "default"),
                        ),
                        field(
                            "Agent filesystem root",
                            optional_path(server.agent_filesystem_root.as_deref(), "disabled"),
                        ),
                        field(
                            "Ports file",
                            optional_path(server.ports_file.as_deref(), "disabled"),
                        ),
                    ],
                )]
            }
        },
        ResourceKind::Memory => match (context.uses_local_server(), local) {
            (false, _) => {
                vec![ResourceRow::unavailable(
                    "Process-local memory settings are unavailable for an external server",
                )]
            }
            (true, None) => vec![ResourceRow::new(
                "System memory",
                "automatic",
                vec![field("Override", "automatic")],
            )],
            (true, Some(server)) => {
                vec![ResourceRow::new(
                    "System memory",
                    optional_copy(server.system_memory_override, "automatic"),
                    vec![field(
                        "Override",
                        optional_copy(server.system_memory_override, "automatic"),
                    )],
                )]
            }
        },
        ResourceKind::Observability => unreachable!("observability is loaded asynchronously"),
        _ => unreachable!("resource kind is scoped before loading"),
    }
}

async fn load_server_observability(context: &Context) -> anyhow::Result<Vec<ResourceRow>> {
    if !context.uses_local_server() {
        return Ok(vec![ResourceRow::unavailable(
            "Local observability capability is only available on the built-in local server",
        )]);
    }
    let configured = context
        .manifest_local_server()
        .is_none_or(|server| server.observability);
    if !configured {
        return Ok(vec![ResourceRow::new(
            "Local observability",
            "disabled in localServer",
            vec![
                field("Configured", false),
                field("Capability", "Not queried while disabled"),
            ],
        )]);
    }
    let client = LocalObservabilityClient::new(
        context.file_download_client().clone(),
        context.worker_service_url().clone(),
    );
    let Some(capability) = client.capability().await? else {
        return Ok(vec![ResourceRow::unavailable(
            "The connected server does not expose local observability capability",
        )]);
    };
    Ok(vec![ResourceRow::new(
        "Local observability",
        format!("{:?}", capability.status),
        vec![
            debug_field("Status", capability.status),
            field(
                "Message",
                capability.message.unwrap_or_else(|| "none".to_string()),
            ),
            field(
                "Exporter",
                format!(
                    "{} {}",
                    capability.exporter_name, capability.exporter_version
                ),
            ),
            field(
                "Sample interval",
                format!("{} seconds", capability.sample_interval_seconds),
            ),
            field(
                "Retention age",
                format!("{} days", capability.retention_days),
            ),
            field("Retention size", format_bytes(capability.retention_bytes)),
            field("Discarded points", capability.discarded_points),
            field(
                "Last service sample",
                optional_copy(capability.last_service_sample_unix_nanos, "none"),
            ),
            field(
                "Last Agent ingest",
                optional_copy(capability.last_agent_ingest_unix_nanos, "none"),
            ),
        ],
    )])
}

fn usage_row(usage: golem_common::model::account_usage::AccountUsage) -> ResourceRow {
    let value = usage.usage;
    ResourceRow::new(
        value.period.to_string(),
        format!("as of {}", value.as_of),
        vec![
            field("Compute", format!("{} GCU", value.compute_gcu)),
            field("Memory", format!("{} GB-seconds", value.memory_gb_seconds)),
            field(
                "Durable storage",
                format!("{} GB-month", value.durable_storage_gb_month),
            ),
            field(
                "Ephemeral storage",
                format!("{} GB-month", value.ephemeral_storage_gb_month),
            ),
            field("As of", value.as_of),
            debug_field("Metering", value.metering),
        ],
    )
}

fn storage_limit_row(name: &str, limit: StorageLimit) -> ResourceRow {
    ResourceRow::new(
        name,
        format!("effective {}", limit.effective_value),
        limit_fields(
            limit.effective_value,
            limit.plan_default,
            limit.override_value,
            limit.ceiling,
            limit.user_configurable,
        ),
    )
}

fn memory_limit_row(name: &str, limit: MemoryLimit) -> ResourceRow {
    ResourceRow::new(
        name,
        format!("effective {}", limit.effective_value),
        limit_fields(
            limit.effective_value,
            limit.plan_default,
            limit.override_value,
            limit.ceiling,
            limit.user_configurable,
        ),
    )
}

fn limit_fields(
    effective: u64,
    plan_default: u64,
    override_value: Option<u64>,
    ceiling: u64,
    configurable: bool,
) -> Vec<ResourceField> {
    vec![
        field("Effective", effective),
        field("Plan default", plan_default),
        field(
            "Override",
            override_value
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
        ),
        field("Ceiling", ceiling),
        field("User configurable", configurable),
    ]
}

fn field(label: impl Into<String>, value: impl ToString) -> ResourceField {
    ResourceField {
        label: label.into(),
        value: value.to_string(),
    }
}

fn debug_field(label: impl Into<String>, value: impl Debug) -> ResourceField {
    field(label, format!("{value:?}"))
}

fn join_display(values: impl IntoIterator<Item = impl ToString>) -> String {
    values
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn optional(value: Option<&str>, fallback: &str) -> String {
    value.unwrap_or(fallback).to_string()
}

fn optional_copy(value: Option<impl ToString>, fallback: &str) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn optional_path(value: Option<&std::path::Path>, fallback: &str) -> String {
    value
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn observed_port(observed: Option<u16>, configured: Option<u16>) -> String {
    match (observed, configured) {
        (Some(port), _) => format!("{port} (observed)"),
        (None, Some(port)) => format!("{port} (configured)"),
        (None, None) => "not observed".to_string(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes >= GIB && bytes.is_multiple_of(GIB) {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::context_executor::TuiContextId;
    use test_r::test;

    #[test]
    fn catalogs_are_scoped_and_have_stable_order() {
        assert_eq!(
            ResourceKind::for_scope(ResourceScope::Environment)[0],
            ResourceKind::Secrets
        );
        assert_eq!(
            ResourceKind::for_scope(ResourceScope::Account)[0],
            ResourceKind::Identity
        );
        assert_eq!(
            ResourceKind::for_scope(ResourceScope::Server)[0],
            ResourceKind::Manifest
        );
        for scope in ResourceScope::ALL {
            assert!(
                ResourceKind::for_scope(scope)
                    .iter()
                    .all(|kind| kind.scope() == scope)
            );
        }
    }

    #[test]
    fn scope_navigation_remembers_kind_and_focus() {
        let mut state = ResourcesState::default();
        state.move_selection(true);
        state.cycle_focus(true);
        assert_eq!(state.selected_kind(), ResourceKind::RetryPolicies);
        assert_eq!(state.focus(), ResourceFocus::Items);

        state.select_scope(ResourceScope::Account);
        assert_eq!(state.selected_kind(), ResourceKind::Identity);
        assert_eq!(state.focus(), ResourceFocus::Kinds);
        state.select_scope(ResourceScope::Environment);
        assert_eq!(state.selected_kind(), ResourceKind::RetryPolicies);
        assert_eq!(state.focus(), ResourceFocus::Items);
    }

    #[test]
    fn finishing_a_kind_load_only_populates_that_kind() {
        let mut state = ResourcesState::default();
        let context_id = TuiContextId::new(7);
        let (generation, scope, kind) = state.begin_load(context_id);
        state.finish(
            generation,
            context_id,
            Ok(ResourcesRefreshPayload {
                scope,
                kind,
                rows: vec![ResourceRow::new(
                    "/payments/token",
                    "revision 7",
                    vec![field("Value", "Masked")],
                )],
            }),
        );

        assert_eq!(state.kind_count(ResourceKind::Secrets), Some(1));
        assert!(!state.is_loaded(ResourceKind::RetryPolicies));
        assert_eq!(state.visible_rows()[0].fields[0].value, "Masked");
    }

    #[test]
    fn stale_load_does_not_replace_current_scope() {
        let mut state = ResourcesState::default();
        let context_id = TuiContextId::new(7);
        let (generation, scope, kind) = state.begin_load(context_id);
        state.select_scope(ResourceScope::Account);
        state.generation = state.generation.saturating_add(1);
        state.finish(
            generation,
            context_id,
            Ok(ResourcesRefreshPayload {
                scope,
                kind,
                rows: vec![ResourceRow::new("stale", "", Vec::new())],
            }),
        );
        assert!(!state.is_loaded(ResourceKind::Identity));
    }

    #[test]
    fn rapidly_opening_another_kind_ignores_the_older_result() {
        let mut state = ResourcesState::default();
        let context_id = TuiContextId::new(7);
        let (old_generation, old_scope, old_kind) = state.begin_load(context_id);
        state.move_selection(true);
        let (generation, scope, kind) = state.begin_load(context_id);

        state.finish(
            old_generation,
            context_id,
            Ok(ResourcesRefreshPayload {
                scope: old_scope,
                kind: old_kind,
                rows: vec![ResourceRow::new("stale", "", Vec::new())],
            }),
        );
        state.finish(
            generation,
            context_id,
            Ok(ResourcesRefreshPayload {
                scope,
                kind,
                rows: vec![ResourceRow::new("current", "", Vec::new())],
            }),
        );

        assert!(!state.is_loaded(ResourceKind::Secrets));
        assert_eq!(state.visible_rows()[0].name, "current");
    }

    #[test]
    fn unavailable_rows_explain_why() {
        let row = ResourceRow::unavailable("external server");
        assert_eq!(row.name, "Unavailable");
        assert_eq!(row.fields[0].label, "Reason");
        assert_eq!(row.fields[0].value, "external server");
    }

    #[test]
    fn retention_bytes_use_compact_binary_units() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 GiB");
        assert_eq!(format_bytes(8 * 1024 * 1024), "8 MiB");
        assert_eq!(format_bytes(17), "17 bytes");
    }
}
