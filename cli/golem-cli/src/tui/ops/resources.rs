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
use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use golem_client::api::{
    AgentSecretsClient, ApiDomainClient, ApiSecurityClient, EnvironmentClient,
    EnvironmentToolGrantsClient, EnvironmentToolMiddlewareGrantsClient, ResourcesClient,
    RetryPoliciesClient,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ResourceScope {
    Deployment,
    Environment,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ResourceRow {
    pub(in crate::tui) kind: String,
    pub(in crate::tui) name: String,
    pub(in crate::tui) detail: String,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ResourcesRefreshPayload {
    pub(in crate::tui) scope: ResourceScope,
    pub(in crate::tui) rows: Vec<ResourceRow>,
}

pub(in crate::tui) struct ResourcesState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<super::super::context_executor::TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) scope: ResourceScope,
    pub(in crate::tui) rows: Vec<ResourceRow>,
    pub(in crate::tui) kinds: Vec<(String, usize)>,
    pub(in crate::tui) selected_kind: usize,
    pub(in crate::tui) selected_row: usize,
    pub(in crate::tui) focus_rows: bool,
    pub(in crate::tui) error: Option<String>,
}

impl Default for ResourcesState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            scope: ResourceScope::Environment,
            rows: Vec::new(),
            kinds: Vec::new(),
            selected_kind: 0,
            selected_row: 0,
            focus_rows: false,
            error: None,
        }
    }
}

impl ResourcesState {
    pub(in crate::tui) fn selected_kind_name(&self) -> Option<&str> {
        self.kinds
            .get(self.selected_kind)
            .map(|(kind, _)| kind.as_str())
    }

    pub(in crate::tui) fn visible_rows(&self) -> Vec<&ResourceRow> {
        let Some(kind) = self.selected_kind_name() else {
            return Vec::new();
        };
        self.rows.iter().filter(|row| row.kind == kind).collect()
    }

    pub(in crate::tui) fn rebuild_kinds(&mut self) {
        let selected = self.selected_kind_name().map(ToString::to_string);
        let mut counts = BTreeMap::<String, usize>::new();
        for row in &self.rows {
            *counts.entry(row.kind.clone()).or_default() += 1;
        }
        self.kinds = counts.into_iter().collect();
        self.selected_kind = selected
            .and_then(|selected| self.kinds.iter().position(|(kind, _)| *kind == selected))
            .unwrap_or(0)
            .min(self.kinds.len().saturating_sub(1));
        self.selected_row = self
            .selected_row
            .min(self.visible_rows().len().saturating_sub(1));
    }
}

pub(in crate::tui) async fn load(
    context: Arc<Context>,
    environment_reference: Option<&EnvironmentReference>,
    scope: ResourceScope,
) -> anyhow::Result<ResourcesRefreshPayload> {
    let environment = context
        .environment_handler()
        .resolve_opt_environment_reference_read_only(
            EnvironmentResolveMode::Any,
            environment_reference,
        )
        .await?;
    let rows = match scope {
        ResourceScope::Deployment => deployment_rows(&context, &environment).await?,
        ResourceScope::Environment => environment_rows(&context, &environment).await?,
    };
    Ok(ResourcesRefreshPayload { scope, rows })
}

async fn deployment_rows(
    context: &Arc<Context>,
    environment: &crate::model::environment::ResolvedEnvironmentIdentity,
) -> anyhow::Result<Vec<ResourceRow>> {
    let current = environment.current_deployment_or_err()?;
    let summary = context
        .golem_clients()
        .await?
        .environment
        .get_deployment_summary(
            &environment.environment_id.0,
            current.deployment_revision.get(),
        )
        .await
        .map_service_error()?;
    let mut rows = Vec::new();
    rows.extend(summary.components.into_iter().map(|component| ResourceRow {
        kind: "Components".to_string(),
        name: component.name.0,
        detail: format!("revision {} · {}", component.revision, component.id),
    }));
    let agent_types = context.app_handler().list_agent_types(environment).await?;
    rows.extend(agent_types.into_iter().map(|agent_type| ResourceRow {
        kind: "Agent Types".to_string(),
        name: agent_type.agent_type.type_name.0,
        detail: format!("component {}", agent_type.implemented_by.component_name),
    }));
    rows.extend(
        summary
            .http_api_deployments
            .into_iter()
            .map(|deployment| ResourceRow {
                kind: "HTTP API Deployments".to_string(),
                name: deployment.domain.0,
                detail: format!("revision {} · {}", deployment.revision, deployment.id),
            }),
    );
    rows.extend(
        summary
            .mcp_deployments
            .into_iter()
            .map(|deployment| ResourceRow {
                kind: "MCP Deployments".to_string(),
                name: deployment.domain.0,
                detail: format!("revision {} · {}", deployment.revision, deployment.id),
            }),
    );
    rows.extend(summary.remote_tools.into_iter().map(|tool| ResourceRow {
        kind: "Deployed Tools".to_string(),
        name: tool.name.to_string(),
        detail: "remote".to_string(),
    }));
    rows.extend(summary.published_tools.into_iter().map(|tool| ResourceRow {
        kind: "Deployed Tools".to_string(),
        name: tool.to_string(),
        detail: "published".to_string(),
    }));
    rows.extend(
        summary
            .remote_tool_middlewares
            .into_iter()
            .map(|middleware| ResourceRow {
                kind: "Tool Middleware".to_string(),
                name: middleware.name.to_string(),
                detail: "remote".to_string(),
            }),
    );
    rows.extend(
        summary
            .published_tool_middlewares
            .into_iter()
            .map(|middleware| ResourceRow {
                kind: "Tool Middleware".to_string(),
                name: middleware.to_string(),
                detail: "published".to_string(),
            }),
    );
    Ok(rows)
}

async fn environment_rows(
    context: &Arc<Context>,
    environment: &crate::model::environment::ResolvedEnvironmentIdentity,
) -> anyhow::Result<Vec<ResourceRow>> {
    let clients = context.golem_clients().await?;
    let environment_id = &environment.environment_id.0;
    let mut rows = Vec::new();
    push_safe_rows(
        &mut rows,
        "Secrets",
        clients
            .agent_secrets
            .list_environment_agent_secrets(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    push_safe_rows(
        &mut rows,
        "Retry Policies",
        clients
            .retry_policies
            .list_environment_retry_policies(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    push_safe_rows(
        &mut rows,
        "Resource Definitions",
        clients
            .resources
            .list_environment_resources(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    push_safe_rows(
        &mut rows,
        "Domains",
        clients
            .api_domain
            .list_environment_domain_registrations(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    push_safe_rows(
        &mut rows,
        "Security Schemes",
        clients
            .api_security
            .list_environment_security_schemes(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    push_safe_rows(
        &mut rows,
        "Plugin Grants",
        context
            .environment_handler()
            .plugin_grants(environment)
            .await?
            .into_values()
            .collect::<Vec<_>>(),
    );
    push_safe_rows(
        &mut rows,
        "Tool Grants",
        clients
            .environment_tool_grants
            .list_environment_tool_grants(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    push_safe_rows(
        &mut rows,
        "Tool Middleware Grants",
        clients
            .environment_tool_middleware_grants
            .list_environment_tool_middleware_grants(environment_id)
            .await
            .map_service_error()?
            .values,
    );
    Ok(rows)
}

fn push_safe_rows<T: Serialize>(rows: &mut Vec<ResourceRow>, kind: &str, values: Vec<T>) {
    rows.extend(values.into_iter().enumerate().map(|(index, value)| {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        ResourceRow {
            kind: kind.to_string(),
            name: safe_identity(&value).unwrap_or_else(|| format!("{} {}", kind, index + 1)),
            detail: safe_detail(&value),
        }
    }));
}

fn safe_identity(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["name", "path", "domain"] {
        if let Some(value) = object.get(key) {
            return Some(match value {
                Value::String(value) => value.clone(),
                Value::Array(parts) => format!(
                    "/{}",
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("/")
                ),
                value => value.to_string(),
            });
        }
    }
    object
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn safe_detail(value: &Value) -> String {
    let Some(object) = value.as_object() else {
        return String::new();
    };
    ["revision", "id", "version"]
        .into_iter()
        .filter_map(|key| object.get(key).map(|value| format!("{key} {value}")))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn secret_projection_keeps_identity_and_drops_values() {
        let value = serde_json::json!({
            "path": ["payments", "token"],
            "value": "must-not-render",
            "revision": 7
        });

        assert_eq!(safe_identity(&value).as_deref(), Some("/payments/token"));
        let detail = safe_detail(&value);
        assert_eq!(detail, "revision 7");
        assert!(!detail.contains("must-not-render"));
    }

    #[test]
    fn rebuilds_homogeneous_kind_counts_and_bounds_selection() {
        let mut state = ResourcesState {
            rows: vec![
                ResourceRow {
                    kind: "Components".to_string(),
                    name: "api".to_string(),
                    detail: String::new(),
                },
                ResourceRow {
                    kind: "Components".to_string(),
                    name: "worker".to_string(),
                    detail: String::new(),
                },
                ResourceRow {
                    kind: "Agent Types".to_string(),
                    name: "Counter".to_string(),
                    detail: String::new(),
                },
            ],
            selected_kind: 99,
            selected_row: 99,
            ..ResourcesState::default()
        };

        state.rebuild_kinds();

        assert_eq!(state.kinds.len(), 2);
        assert_eq!(state.visible_rows().len(), 1);
        assert_eq!(state.selected_row, 0);
    }
}
