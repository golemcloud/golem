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

use super::metrics::{self, MetricsRange};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::local_observability::LocalObservabilityClient;
use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::local_observability::{
    LocalMetricDashboardRequest, LocalMetricDashboardResponse, LocalMetricScope, LocalMetricSource,
    LocalObservabilityCapability, LocalObservabilityStatus,
};
use crate::tui::context_executor::TuiContextId;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum OverviewCard {
    Deployment,
    AgentSignals,
    ServerCollector,
    Stream,
    Issues,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum OverviewIssueTarget {
    Deployments,
    AgentMetrics,
    ServerCollector,
    Stream,
    Agents,
    Configuration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct OverviewIssue {
    pub(in crate::tui) title: String,
    pub(in crate::tui) detail: String,
    pub(in crate::tui) target: OverviewIssueTarget,
}

impl OverviewCard {
    pub(in crate::tui) const ALL: [Self; 5] = [
        Self::Deployment,
        Self::AgentSignals,
        Self::ServerCollector,
        Self::Stream,
        Self::Issues,
    ];

    pub(in crate::tui) const fn label(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::AgentSignals => "Agent signals · last 15 minutes",
            Self::ServerCollector => "Local server & collector",
            Self::Stream => "Stream",
            Self::Issues => "Issues",
        }
    }
}

pub(in crate::tui) struct OverviewSnapshotState<T> {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) value: Option<T>,
    pub(in crate::tui) error: Option<String>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl<T> Default for OverviewSnapshotState<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            value: None,
            error: None,
            task: None,
        }
    }
}

impl<T> OverviewSnapshotState<T> {
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
        result: Result<T, String>,
    ) {
        if generation != self.generation || Some(context_id) != self.context_id {
            return;
        }
        self.task = None;
        self.loading = false;
        match result {
            Ok(value) => {
                self.value = Some(value);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn reset(&mut self) {
        self.abort_task();
        self.generation = self.generation.saturating_add(1);
        self.context_id = None;
        self.loading = false;
        self.value = None;
        self.error = None;
    }

    fn abort_task(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl<T> Drop for OverviewSnapshotState<T> {
    fn drop(&mut self) {
        self.abort_task();
    }
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct OverviewAgentSignals {
    pub(in crate::tui) capability: Option<LocalObservabilityCapability>,
    pub(in crate::tui) dashboard: Option<LocalMetricDashboardResponse>,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct OverviewObservability {
    pub(in crate::tui) capability: Option<LocalObservabilityCapability>,
}

#[derive(Default)]
pub(in crate::tui) struct OverviewState {
    pub(in crate::tui) selected_card: usize,
    pub(in crate::tui) selected_issue: usize,
    pub(in crate::tui) agent_signals: OverviewSnapshotState<OverviewAgentSignals>,
    pub(in crate::tui) observability: OverviewSnapshotState<OverviewObservability>,
}

impl OverviewState {
    pub(in crate::tui) fn selected_card(&self) -> OverviewCard {
        OverviewCard::ALL[self.selected_card.min(OverviewCard::ALL.len() - 1)]
    }

    pub(in crate::tui) fn move_card(&mut self, down: bool) {
        self.selected_card = if down {
            self.selected_card
                .saturating_add(1)
                .min(OverviewCard::ALL.len() - 1)
        } else {
            self.selected_card.saturating_sub(1)
        };
    }

    pub(in crate::tui) fn move_issue(&mut self, down: bool, count: usize) {
        self.selected_issue = if down {
            self.selected_issue
                .saturating_add(1)
                .min(count.saturating_sub(1))
        } else {
            self.selected_issue.saturating_sub(1)
        };
    }

    pub(in crate::tui) fn reset(&mut self) {
        self.selected_card = 0;
        self.selected_issue = 0;
        self.agent_signals.reset();
        self.observability.reset();
    }
}

pub(in crate::tui) async fn load_agent_signals(
    context: Arc<Context>,
    environment_reference: Option<EnvironmentReference>,
) -> anyhow::Result<OverviewAgentSignals> {
    let range = metrics::current_range(MetricsRange::Minutes15);
    if !context.uses_local_server() {
        return Ok(OverviewAgentSignals {
            capability: None,
            dashboard: None,
        });
    }
    let client = LocalObservabilityClient::new(
        context.file_download_client().clone(),
        context.worker_service_url().clone(),
    );
    let capability = client.capability().await?;
    let dashboard = if capability
        .as_ref()
        .is_some_and(|capability| capability.status == LocalObservabilityStatus::Ready)
    {
        let environment = context
            .environment_handler()
            .resolve_opt_environment_reference_read_only(
                EnvironmentResolveMode::Any,
                environment_reference.as_ref(),
            )
            .await?;
        Some(
            client
                .dashboard(&LocalMetricDashboardRequest {
                    source: LocalMetricSource::Agents,
                    scope: LocalMetricScope {
                        environment_id: Some(environment.environment_id.0),
                        ..Default::default()
                    },
                    range,
                    max_points: Some(40),
                })
                .await?,
        )
    } else {
        None
    };
    Ok(OverviewAgentSignals {
        capability,
        dashboard,
    })
}

pub(in crate::tui) async fn load_observability(
    context: Arc<Context>,
) -> anyhow::Result<OverviewObservability> {
    if !context.uses_local_server() {
        return Ok(OverviewObservability { capability: None });
    }
    let client = LocalObservabilityClient::new(
        context.file_download_client().clone(),
        context.worker_service_url().clone(),
    );
    Ok(OverviewObservability {
        capability: client.capability().await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn card_selection_is_bounded() {
        let mut state = OverviewState::default();
        state.move_card(false);
        assert_eq!(state.selected_card(), OverviewCard::Deployment);
        for _ in 0..10 {
            state.move_card(true);
        }
        assert_eq!(state.selected_card(), OverviewCard::Issues);
    }

    #[test]
    fn stale_card_results_do_not_replace_current_data() {
        let mut state = OverviewSnapshotState::default();
        let context_id = TuiContextId::new(7);
        let old_generation = state.begin(context_id);
        let generation = state.begin(context_id);
        state.finish(old_generation, context_id, Ok("stale"));
        state.finish(generation, context_id, Ok("current"));
        assert_eq!(state.value, Some("current"));
    }
}
