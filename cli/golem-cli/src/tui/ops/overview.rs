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

use crate::context::Context;
use crate::local_observability::LocalObservabilityClient;
use crate::model::local_observability::LocalObservabilityCapability;
use crate::tui::context_executor::TuiContextId;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum OverviewItem {
    Agents,
    Oplog,
    Stream,
    Metrics,
    Deployments,
    Configuration,
    ServerCollector,
    Issues,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum OverviewIssueTarget {
    Deployments,
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

impl OverviewItem {
    pub(in crate::tui) const ALL: [Self; 8] = [
        Self::Agents,
        Self::Oplog,
        Self::Stream,
        Self::Metrics,
        Self::Deployments,
        Self::Configuration,
        Self::ServerCollector,
        Self::Issues,
    ];

    pub(in crate::tui) const fn label(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Oplog => "Oplog",
            Self::Stream => "Stream",
            Self::Metrics => "Metrics",
            Self::Deployments => "Deployments",
            Self::Configuration => "Configuration",
            Self::ServerCollector => "Server & collector",
            Self::Issues => "Issues",
        }
    }

    pub(in crate::tui) const fn description(self) -> &'static str {
        match self {
            Self::Agents => "Find Agents and choose investigation scope",
            Self::Oplog => "Read grouped history or inspect exact entries",
            Self::Stream => "Follow live events for one exact Agent",
            Self::Metrics => "Open curated dashboards or metric Explore",
            Self::Deployments => "Inspect revisions, composition, and compare",
            Self::Configuration => "Browse environment, account, and server data",
            Self::ServerCollector => "Inspect the local process and telemetry receiver",
            Self::Issues => "Review current actionable diagnostics",
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
pub(in crate::tui) struct OverviewObservability {
    pub(in crate::tui) capability: Option<LocalObservabilityCapability>,
}

#[derive(Default)]
pub(in crate::tui) struct OverviewState {
    pub(in crate::tui) selected_item: usize,
    pub(in crate::tui) selected_issue: usize,
    pub(in crate::tui) observability: OverviewSnapshotState<OverviewObservability>,
}

impl OverviewState {
    pub(in crate::tui) fn selected_item(&self) -> OverviewItem {
        OverviewItem::ALL[self.selected_item.min(OverviewItem::ALL.len() - 1)]
    }

    pub(in crate::tui) fn move_item(&mut self, down: bool) {
        self.selected_item = if down {
            self.selected_item
                .saturating_add(1)
                .min(OverviewItem::ALL.len() - 1)
        } else {
            self.selected_item.saturating_sub(1)
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
        self.selected_item = 0;
        self.selected_issue = 0;
        self.observability.reset();
    }
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
    fn item_selection_is_bounded() {
        let mut state = OverviewState::default();
        state.move_item(false);
        assert_eq!(state.selected_item(), OverviewItem::Agents);
        for _ in 0..10 {
            state.move_item(true);
        }
        assert_eq!(state.selected_item(), OverviewItem::Issues);
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
