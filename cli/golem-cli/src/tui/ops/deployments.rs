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
use crate::model::environment::EnvironmentReference;
use crate::tui::context_executor::TuiContextId;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::deployment::{Deployment, DeploymentRevision, DeploymentSummary};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(in crate::tui) const RECENT_LIMIT: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct DeploymentRecord {
    pub(in crate::tui) revision: u64,
    pub(in crate::tui) version: String,
    pub(in crate::tui) hash: String,
}

impl From<Deployment> for DeploymentRecord {
    fn from(deployment: Deployment) -> Self {
        Self {
            revision: deployment.revision.get(),
            version: deployment.version.0,
            hash: deployment.deployment_hash.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::tui) enum DeploymentEntityKind {
    Component,
    AgentType,
    HttpApi,
    Mcp,
    RemoteTool,
    PublishedTool,
    RemoteMiddleware,
    PublishedMiddleware,
    UniversalMiddleware,
    EnvironmentMiddlewareBinding,
    AgentMiddlewareBinding,
}

impl DeploymentEntityKind {
    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Self::Component => "Components",
            Self::AgentType => "Agent Types",
            Self::HttpApi => "HTTP deployments",
            Self::Mcp => "MCP deployments",
            Self::RemoteTool => "Remote tools",
            Self::PublishedTool => "Published tools",
            Self::RemoteMiddleware => "Remote middleware",
            Self::PublishedMiddleware => "Published middleware",
            Self::UniversalMiddleware => "Universal middleware",
            Self::EnvironmentMiddlewareBinding => "Environment middleware bindings",
            Self::AgentMiddlewareBinding => "Agent middleware bindings",
        }
    }

    pub(in crate::tui) fn item_label(self) -> &'static str {
        match self {
            Self::Component => "Component",
            Self::AgentType => "Agent Type",
            Self::HttpApi => "HTTP deployment",
            Self::Mcp => "MCP deployment",
            Self::RemoteTool => "Remote tool",
            Self::PublishedTool => "Published tool",
            Self::RemoteMiddleware => "Remote middleware",
            Self::PublishedMiddleware => "Published middleware",
            Self::UniversalMiddleware => "Universal middleware",
            Self::EnvironmentMiddlewareBinding => "Environment binding",
            Self::AgentMiddlewareBinding => "Agent binding",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct DeploymentEntity {
    pub(in crate::tui) kind: DeploymentEntityKind,
    pub(in crate::tui) name: String,
    pub(in crate::tui) detail: String,
    fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct DeploymentRevisionDetails {
    pub(in crate::tui) revision: u64,
    pub(in crate::tui) hash: String,
    pub(in crate::tui) entities: Vec<DeploymentEntity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum DeploymentChangeKind {
    Added,
    Changed,
    Removed,
}

impl DeploymentChangeKind {
    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Changed => "Changed",
            Self::Removed => "Removed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct DeploymentChange {
    pub(in crate::tui) change: DeploymentChangeKind,
    pub(in crate::tui) kind: DeploymentEntityKind,
    pub(in crate::tui) name: String,
    pub(in crate::tui) before: Option<String>,
    pub(in crate::tui) after: Option<String>,
}

#[derive(Clone, Debug)]
pub(in crate::tui) enum DeploymentsPayload {
    History(Vec<DeploymentRecord>),
    Revisions(Vec<DeploymentRevisionDetails>),
}

pub(in crate::tui) struct DeploymentsState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) history: Vec<DeploymentRecord>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) details: BTreeMap<u64, DeploymentRevisionDetails>,
    pub(in crate::tui) selected_change: usize,
    pub(in crate::tui) table_first_visible: Cell<usize>,
    pub(in crate::tui) scroll: usize,
    pub(in crate::tui) content_height: Cell<usize>,
    pub(in crate::tui) viewport_height: Cell<usize>,
    pub(in crate::tui) error: Option<String>,
    pub(in crate::tui) task: Option<tokio::task::JoinHandle<()>>,
}

impl Default for DeploymentsState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            history: Vec::new(),
            selected: 0,
            details: BTreeMap::new(),
            selected_change: 0,
            table_first_visible: Cell::new(0),
            scroll: 0,
            content_height: Cell::new(0),
            viewport_height: Cell::new(0),
            error: None,
            task: None,
        }
    }
}

impl DeploymentsState {
    pub(in crate::tui) fn begin(&mut self, context_id: TuiContextId) -> u64 {
        self.abort_task();
        self.generation = self.generation.saturating_add(1);
        self.context_id = Some(context_id);
        self.loading = true;
        self.error = None;
        self.generation
    }

    pub(in crate::tui) fn attach_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.task = Some(task);
    }

    pub(in crate::tui) fn finish(
        &mut self,
        generation: u64,
        context_id: TuiContextId,
        result: Result<DeploymentsPayload, String>,
    ) {
        if generation != self.generation || Some(context_id) != self.context_id {
            return;
        }
        self.task = None;
        self.loading = false;
        match result {
            Ok(DeploymentsPayload::History(mut history)) => {
                history.sort_by(|left, right| right.revision.cmp(&left.revision));
                let selected_revision = self.selected_record().map(|record| record.revision);
                self.history = history;
                self.selected = selected_revision
                    .and_then(|revision| {
                        self.history
                            .iter()
                            .position(|record| record.revision == revision)
                    })
                    .unwrap_or(0)
                    .min(self.history.len().saturating_sub(1));
                self.error = None;
            }
            Ok(DeploymentsPayload::Revisions(details)) => {
                for detail in details {
                    self.details.insert(detail.revision, detail);
                }
                self.selected_change = 0;
                self.table_first_visible.set(0);
                self.scroll = 0;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub(in crate::tui) fn reset(&mut self) {
        self.abort_task();
        self.generation = self.generation.saturating_add(1);
        self.context_id = None;
        self.loading = false;
        self.history.clear();
        self.selected = 0;
        self.details.clear();
        self.selected_change = 0;
        self.table_first_visible.set(0);
        self.scroll = 0;
        self.content_height.set(0);
        self.viewport_height.set(0);
        self.error = None;
    }

    pub(in crate::tui) fn selected_record(&self) -> Option<&DeploymentRecord> {
        self.history.get(self.selected)
    }

    pub(in crate::tui) fn select_previous(&mut self, visible_count: usize) {
        if visible_count > 0 {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub(in crate::tui) fn select_next(&mut self, visible_count: usize) {
        if visible_count > 0 {
            self.selected = self
                .selected
                .saturating_add(1)
                .min(visible_count.saturating_sub(1));
        }
    }

    pub(in crate::tui) fn predecessor(&self, revision: u64) -> Option<u64> {
        self.history
            .iter()
            .filter(|record| record.revision < revision)
            .map(|record| record.revision)
            .max()
    }

    pub(in crate::tui) fn changes(&self, base: u64, target: u64) -> Option<Vec<DeploymentChange>> {
        Some(compare(
            self.details.get(&base)?,
            self.details.get(&target)?,
        ))
    }

    pub(in crate::tui) fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub(in crate::tui) fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_add(amount).min(
            self.content_height
                .get()
                .saturating_sub(self.viewport_height.get()),
        );
    }

    pub(in crate::tui) fn scroll_to_end(&mut self) {
        self.scroll = self
            .content_height
            .get()
            .saturating_sub(self.viewport_height.get());
    }

    fn abort_task(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for DeploymentsState {
    fn drop(&mut self) {
        self.abort_task();
    }
}

pub(in crate::tui) async fn load_history(
    context: Arc<Context>,
    environment_reference: Option<EnvironmentReference>,
) -> anyhow::Result<DeploymentsPayload> {
    let history = context
        .app_handler()
        .list_deployment_history(environment_reference.as_ref())
        .await?
        .into_iter()
        .map(DeploymentRecord::from)
        .collect();
    Ok(DeploymentsPayload::History(history))
}

pub(in crate::tui) async fn load_revisions(
    context: Arc<Context>,
    environment_reference: Option<EnvironmentReference>,
    revisions: Vec<u64>,
) -> anyhow::Result<DeploymentsPayload> {
    let mut details = Vec::with_capacity(revisions.len());
    for revision in revisions.into_iter().collect::<BTreeSet<_>>() {
        let revision = DeploymentRevision::new(revision)?;
        let Some((summary, agent_types)) = context
            .app_handler()
            .get_deployment_revision(environment_reference.as_ref(), revision)
            .await?
        else {
            anyhow::bail!("Deployment revision {} was not found", revision.get());
        };
        details.push(project_revision(summary, agent_types));
    }
    Ok(DeploymentsPayload::Revisions(details))
}

fn project_revision(
    summary: DeploymentSummary,
    agent_types: Vec<DeployedRegisteredAgentType>,
) -> DeploymentRevisionDetails {
    let mut entities = Vec::new();
    for component in summary.components {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::Component,
            name: component.name.0,
            detail: format!("revision {} · {}", component.revision, component.id),
            fingerprint: component.hash.to_string(),
        });
    }
    for agent_type in agent_types {
        let name = agent_type.agent_type.type_name.0.clone();
        let detail = format!(
            "{} · component revision {}",
            agent_type.implemented_by.component_name, agent_type.implemented_by.component_revision
        );
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::AgentType,
            name,
            detail,
            fingerprint: serde_json::to_string(&agent_type).unwrap_or_default(),
        });
    }
    for deployment in summary.http_api_deployments {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::HttpApi,
            name: deployment.domain.0,
            detail: format!("revision {} · {}", deployment.revision, deployment.id),
            fingerprint: deployment.hash.to_string(),
        });
    }
    for deployment in summary.mcp_deployments {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::Mcp,
            name: deployment.domain.0,
            detail: format!("revision {} · {}", deployment.revision, deployment.id),
            fingerprint: deployment.hash.to_string(),
        });
    }
    for tool in summary.remote_tools {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::RemoteTool,
            name: tool.name.to_string(),
            detail: tool.hash.to_string(),
            fingerprint: tool.hash.to_string(),
        });
    }
    for tool in summary.published_tools {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::PublishedTool,
            name: tool.to_string(),
            detail: "published".to_string(),
            fingerprint: "published".to_string(),
        });
    }
    for middleware in summary.remote_tool_middlewares {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::RemoteMiddleware,
            name: middleware.name.to_string(),
            detail: middleware.hash.to_string(),
            fingerprint: middleware.hash.to_string(),
        });
    }
    for middleware in summary.published_tool_middlewares {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::PublishedMiddleware,
            name: middleware.to_string(),
            detail: "published".to_string(),
            fingerprint: "published".to_string(),
        });
    }
    let mut universal_occurrences = BTreeMap::<String, usize>::new();
    for middleware in summary.universal_tool_middlewares {
        let occurrence = universal_occurrences
            .entry(middleware.name.to_string())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        let name = if *occurrence == 1 {
            middleware.name.to_string()
        } else {
            format!("{}#{}", middleware.name, occurrence)
        };
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::UniversalMiddleware,
            name,
            detail: middleware
                .version
                .clone()
                .unwrap_or_else(|| "latest".to_string()),
            fingerprint: serde_json::to_string(&middleware).unwrap_or_default(),
        });
    }
    for (tool, binding) in summary.environment_tool_middleware_bindings {
        entities.push(DeploymentEntity {
            kind: DeploymentEntityKind::EnvironmentMiddlewareBinding,
            name: tool.to_string(),
            detail: "environment binding".to_string(),
            fingerprint: serde_json::to_string(&binding).unwrap_or_default(),
        });
    }
    for (agent, bindings) in summary.agent_tool_middleware_bindings {
        for (tool, binding) in bindings {
            entities.push(DeploymentEntity {
                kind: DeploymentEntityKind::AgentMiddlewareBinding,
                name: format!("{agent}/{tool}"),
                detail: "Agent Type binding".to_string(),
                fingerprint: serde_json::to_string(&binding).unwrap_or_default(),
            });
        }
    }
    entities.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    DeploymentRevisionDetails {
        revision: summary.deployment_revision.get(),
        hash: summary.deployment_hash.to_string(),
        entities,
    }
}

fn compare(
    base: &DeploymentRevisionDetails,
    target: &DeploymentRevisionDetails,
) -> Vec<DeploymentChange> {
    let base = base
        .entities
        .iter()
        .map(|entity| ((entity.kind, entity.name.as_str()), entity))
        .collect::<BTreeMap<_, _>>();
    let target = target
        .entities
        .iter()
        .map(|entity| ((entity.kind, entity.name.as_str()), entity))
        .collect::<BTreeMap<_, _>>();
    let keys = base
        .keys()
        .chain(target.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .filter_map(|key| match (base.get(&key), target.get(&key)) {
            (None, Some(after)) => Some(DeploymentChange {
                change: DeploymentChangeKind::Added,
                kind: after.kind,
                name: after.name.clone(),
                before: None,
                after: Some(after.detail.clone()),
            }),
            (Some(before), None) => Some(DeploymentChange {
                change: DeploymentChangeKind::Removed,
                kind: before.kind,
                name: before.name.clone(),
                before: Some(before.detail.clone()),
                after: None,
            }),
            (Some(before), Some(after)) if before.fingerprint != after.fingerprint => {
                Some(DeploymentChange {
                    change: DeploymentChangeKind::Changed,
                    kind: after.kind,
                    name: after.name.clone(),
                    before: Some(before.detail.clone()),
                    after: Some(after.detail.clone()),
                })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DeploymentChangeKind, DeploymentEntity, DeploymentEntityKind, DeploymentRevisionDetails,
        DeploymentsState, compare,
    };
    use test_r::test;

    fn entity(name: &str, detail: &str, fingerprint: &str) -> DeploymentEntity {
        DeploymentEntity {
            kind: DeploymentEntityKind::Component,
            name: name.to_string(),
            detail: detail.to_string(),
            fingerprint: fingerprint.to_string(),
        }
    }

    #[test]
    fn comparison_is_typed_and_directional() {
        let base = DeploymentRevisionDetails {
            revision: 1,
            hash: "one".to_string(),
            entities: vec![entity("removed", "r1", "a"), entity("changed", "r1", "a")],
        };
        let target = DeploymentRevisionDetails {
            revision: 2,
            hash: "two".to_string(),
            entities: vec![entity("added", "r2", "b"), entity("changed", "r2", "b")],
        };

        let changes = compare(&base, &target);

        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].change, DeploymentChangeKind::Added);
        assert_eq!(changes[1].change, DeploymentChangeKind::Changed);
        assert_eq!(changes[2].change, DeploymentChangeKind::Removed);
    }

    #[test]
    fn predecessor_uses_revision_identity_not_list_position() {
        let mut state = DeploymentsState::default();
        state.history = vec![
            super::DeploymentRecord {
                revision: 9,
                version: String::new(),
                hash: String::new(),
            },
            super::DeploymentRecord {
                revision: 3,
                version: String::new(),
                hash: String::new(),
            },
            super::DeploymentRecord {
                revision: 7,
                version: String::new(),
                hash: String::new(),
            },
        ];

        assert_eq!(state.predecessor(9), Some(7));
        assert_eq!(state.predecessor(3), None);
    }

    #[test]
    fn stale_context_result_does_not_replace_history() {
        let mut state = DeploymentsState::default();
        state.generation = 4;
        state.context_id = Some(crate::tui::context_executor::TuiContextId::new(9));
        state.loading = true;
        state.history = vec![super::DeploymentRecord {
            revision: 8,
            version: "current".to_string(),
            hash: "current".to_string(),
        }];

        state.finish(
            3,
            crate::tui::context_executor::TuiContextId::new(8),
            Ok(super::DeploymentsPayload::History(vec![
                super::DeploymentRecord {
                    revision: 1,
                    version: "stale".to_string(),
                    hash: "stale".to_string(),
                },
            ])),
        );

        assert!(state.loading);
        assert_eq!(state.history[0].revision, 8);
    }
}
