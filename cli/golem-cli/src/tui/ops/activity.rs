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
use crate::model::agent::RawAgentId;
use crate::model::agent::oplog::OplogEntryPresentation;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use golem_common::model::component::ComponentName;
use golem_common::model::oplog::OplogCursor;
use std::collections::HashMap;
use std::sync::Arc;

pub(in crate::tui) const SOFT_AGENT_LIMIT: usize = 10;
pub(in crate::tui) const HARD_AGENT_LIMIT: usize = 20;
pub(in crate::tui) const ENTRIES_PER_AGENT: u64 = 100;
const MAX_CONCURRENCY: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ActivityRepresentation {
    Grouped,
    Entries,
}

impl ActivityRepresentation {
    pub(in crate::tui) fn next(self) -> Self {
        match self {
            Self::Grouped => Self::Entries,
            Self::Entries => Self::Grouped,
        }
    }

    pub(in crate::tui) fn previous(self) -> Self {
        match self {
            Self::Grouped => Self::Entries,
            Self::Entries => Self::Grouped,
        }
    }

    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Self::Grouped => "Grouped",
            Self::Entries => "Entries",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ActivityOrder {
    Newest,
    Oldest,
}

impl ActivityOrder {
    pub(in crate::tui) fn toggle(self) -> Self {
        match self {
            Self::Newest => Self::Oldest,
            Self::Oldest => Self::Newest,
        }
    }

    pub(in crate::tui) fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest",
            Self::Oldest => "Oldest",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::tui) struct ActivityAgent {
    pub(in crate::tui) component: String,
    pub(in crate::tui) agent_id: String,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityEntry {
    pub(in crate::tui) component: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) presentation: OplogEntryPresentation,
}

#[derive(Clone, Debug, Default)]
pub(in crate::tui) struct ActivityQuery {
    pub(in crate::tui) text: String,
    pub(in crate::tui) from: Option<DateTime<Utc>>,
    pub(in crate::tui) to: Option<DateTime<Utc>>,
}

impl ActivityQuery {
    pub(in crate::tui) fn active(&self) -> bool {
        !self.text.trim().is_empty() || self.from.is_some() || self.to.is_some()
    }
}

#[derive(Clone, Debug)]
pub(in crate::tui) enum ActivityLoadPosition {
    Latest,
    Around(u64),
    Before(u64),
    Search(Option<OplogCursor>),
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityAgentRequest {
    pub(in crate::tui) agent: ActivityAgent,
    pub(in crate::tui) position: ActivityLoadPosition,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityAgentProgress {
    pub(in crate::tui) agent: ActivityAgent,
    pub(in crate::tui) next: Option<OplogCursor>,
    pub(in crate::tui) scanned_through_index: Option<u64>,
    pub(in crate::tui) last_index: u64,
    pub(in crate::tui) oldest_loaded_index: Option<u64>,
    pub(in crate::tui) has_more: bool,
    pub(in crate::tui) scan_limit_reached: bool,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityGroup {
    pub(in crate::tui) component: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) group: Option<crate::model::agent::oplog::OplogEntryGroupKey>,
    pub(in crate::tui) first_index: u64,
    pub(in crate::tui) timestamp: String,
    pub(in crate::tui) kinds: Vec<String>,
    pub(in crate::tui) entry_count: usize,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityFailure {
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) message: String,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityRefreshPayload {
    pub(in crate::tui) append: bool,
    pub(in crate::tui) agents: Vec<ActivityAgent>,
    pub(in crate::tui) entries: Vec<ActivityEntry>,
    pub(in crate::tui) progress: Vec<ActivityAgentProgress>,
    pub(in crate::tui) failures: Vec<ActivityFailure>,
}

pub(in crate::tui) struct ActivityState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<super::super::context_executor::TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) task: Option<tokio::task::JoinHandle<()>>,
    pub(in crate::tui) representation: ActivityRepresentation,
    pub(in crate::tui) order: ActivityOrder,
    pub(in crate::tui) query: String,
    pub(in crate::tui) applied_query: ActivityQuery,
    pub(in crate::tui) agents: Vec<ActivityAgent>,
    pub(in crate::tui) entries: Vec<ActivityEntry>,
    pub(in crate::tui) progress: Vec<ActivityAgentProgress>,
    pub(in crate::tui) failures: Vec<ActivityFailure>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) pending_focus: Option<(ActivityAgent, u64)>,
    pub(in crate::tui) paused: bool,
    pub(in crate::tui) stopped: bool,
    pub(in crate::tui) error: Option<String>,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            task: None,
            representation: ActivityRepresentation::Grouped,
            order: ActivityOrder::Newest,
            query: String::new(),
            applied_query: ActivityQuery::default(),
            agents: Vec::new(),
            entries: Vec::new(),
            progress: Vec::new(),
            failures: Vec::new(),
            selected: 0,
            pending_focus: None,
            paused: false,
            stopped: false,
            error: None,
        }
    }
}

impl Drop for ActivityState {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(in crate::tui) async fn load(
    context: Arc<Context>,
    requests: Vec<ActivityAgentRequest>,
    query: ActivityQuery,
    append: bool,
) -> anyhow::Result<ActivityRefreshPayload> {
    anyhow::ensure!(
        requests.len() <= HARD_AGENT_LIMIT,
        "Oplog is limited to {HARD_AGENT_LIMIT} Agents; narrow the scope or include fewer Agents"
    );
    let agents = requests
        .iter()
        .map(|request| request.agent.clone())
        .collect::<Vec<_>>();
    let results = futures_util::stream::iter(requests.into_iter().map(|request| {
        let context = context.clone();
        let query = query.clone();
        async move {
            let component = ComponentName(request.agent.component.clone());
            let agent_id = RawAgentId(request.agent.agent_id.clone());
            let result = match request.position {
                ActivityLoadPosition::Latest => context
                    .agent_handler()
                    .oplog_snapshot(&component, &agent_id, ENTRIES_PER_AGENT, None)
                    .await
                    .map(|entries| {
                        let oldest = entries.iter().map(|entry| entry.index).min();
                        let last_index = entries.iter().map(|entry| entry.index).max().unwrap_or(0);
                        let progress = ActivityAgentProgress {
                            agent: request.agent.clone(),
                            next: None,
                            scanned_through_index: None,
                            last_index,
                            oldest_loaded_index: oldest,
                            has_more: oldest.is_some_and(|index| index > 1),
                            scan_limit_reached: false,
                        };
                        (entries, progress)
                    }),
                ActivityLoadPosition::Around(index) => {
                    let from = index.saturating_sub(ENTRIES_PER_AGENT / 2).max(1);
                    context
                        .agent_handler()
                        .oplog_page(&component, &agent_id, from, ENTRIES_PER_AGENT)
                        .await
                        .map(|entries| {
                            let oldest = entries.iter().map(|entry| entry.index).min();
                            let last_index = entries
                                .iter()
                                .map(|entry| entry.index)
                                .max()
                                .unwrap_or(index);
                            let progress = ActivityAgentProgress {
                                agent: request.agent.clone(),
                                next: None,
                                scanned_through_index: None,
                                last_index,
                                oldest_loaded_index: oldest,
                                has_more: oldest.is_some_and(|index| index > 1),
                                scan_limit_reached: false,
                            };
                            (entries, progress)
                        })
                }
                ActivityLoadPosition::Before(end) => {
                    let from = end.saturating_sub(ENTRIES_PER_AGENT).max(1);
                    let count = end.saturating_sub(from);
                    context
                        .agent_handler()
                        .oplog_page(&component, &agent_id, from, count)
                        .await
                        .map(|entries| {
                            let oldest = entries.iter().map(|entry| entry.index).min();
                            let progress = ActivityAgentProgress {
                                agent: request.agent.clone(),
                                next: None,
                                scanned_through_index: None,
                                last_index: end.saturating_sub(1),
                                oldest_loaded_index: oldest,
                                has_more: from > 1 && !entries.is_empty(),
                                scan_limit_reached: false,
                            };
                            (entries, progress)
                        })
                }
                ActivityLoadPosition::Search(cursor) => context
                    .agent_handler()
                    .oplog_search_batch(
                        &component,
                        &agent_id,
                        Some(&query.text),
                        query.from,
                        query.to,
                        ENTRIES_PER_AGENT,
                        cursor,
                    )
                    .await
                    .map(|page| {
                        let has_more = page.next.is_some();
                        let oldest_loaded_index =
                            page.entries.iter().map(|entry| entry.index).min();
                        let progress = ActivityAgentProgress {
                            agent: request.agent.clone(),
                            next: page.next,
                            scanned_through_index: page.scanned_through_index,
                            last_index: page.last_index,
                            oldest_loaded_index,
                            has_more,
                            scan_limit_reached: page.scan_limit_reached,
                        };
                        (page.entries, progress)
                    }),
            };
            (request.agent, result)
        }
    }))
    .buffer_unordered(MAX_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut entries = Vec::new();
    let mut progress = Vec::new();
    let mut failures = Vec::new();
    for (agent, result) in results {
        match result {
            Ok((agent_entries, agent_progress)) => {
                entries.extend(agent_entries.into_iter().map(|entry| ActivityEntry {
                    component: agent.component.clone(),
                    agent_id: agent.agent_id.clone(),
                    presentation: entry.into(),
                }));
                progress.push(agent_progress);
            }
            Err(error) => failures.push(ActivityFailure {
                agent_id: agent.agent_id,
                message: format!("{error:#}"),
            }),
        }
    }
    entries.sort_by(|left, right| {
        right
            .presentation
            .timestamp
            .cmp(&left.presentation.timestamp)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
            .then_with(|| right.presentation.index.cmp(&left.presentation.index))
    });
    Ok(ActivityRefreshPayload {
        append,
        agents,
        entries,
        progress,
        failures,
    })
}

pub(in crate::tui) fn grouped_entries(entries: &[ActivityEntry]) -> Vec<ActivityGroup> {
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    enum Key {
        Group(
            String,
            String,
            crate::model::agent::oplog::OplogEntryGroupKey,
        ),
        Entry(String, String, u64),
    }

    let mut positions = HashMap::<Key, usize>::new();
    let mut groups = Vec::<ActivityGroup>::new();
    for entry in entries {
        let key = entry
            .presentation
            .group
            .as_ref()
            .map(|group| {
                Key::Group(
                    entry.component.clone(),
                    entry.agent_id.clone(),
                    group.clone(),
                )
            })
            .unwrap_or_else(|| {
                Key::Entry(
                    entry.component.clone(),
                    entry.agent_id.clone(),
                    entry.presentation.index,
                )
            });
        if let Some(position) = positions.get(&key).copied() {
            let group = &mut groups[position];
            group.entry_count = group.entry_count.saturating_add(1);
            if !group.kinds.contains(&entry.presentation.kind) {
                group.kinds.push(entry.presentation.kind.clone());
            }
        } else {
            positions.insert(key, groups.len());
            groups.push(ActivityGroup {
                component: entry.component.clone(),
                agent_id: entry.agent_id.clone(),
                group: entry.presentation.group.clone(),
                first_index: entry.presentation.index,
                timestamp: entry.presentation.timestamp.to_string(),
                kinds: vec![entry.presentation.kind.clone()],
                entry_count: 1,
            });
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::agent::oplog::OplogEntryGroupKey;
    use golem_common::model::Timestamp;
    use test_r::test;

    fn entry(
        component: &str,
        agent_id: &str,
        index: u64,
        kind: &str,
        group: Option<u64>,
    ) -> ActivityEntry {
        ActivityEntry {
            component: component.to_string(),
            agent_id: agent_id.to_string(),
            presentation: OplogEntryPresentation {
                index,
                timestamp: Timestamp::from(index),
                kind: kind.to_string(),
                group: group.map(OplogEntryGroupKey::OplogIndex),
                detail: serde_json::json!({"kind": kind}),
            },
        }
    }

    #[test]
    fn typed_groups_do_not_merge_agents_or_ambiguous_entries() {
        let groups = grouped_entries(&[
            entry("cart", "cart-1", 12, "End", Some(10)),
            entry("cart", "cart-1", 10, "Start", Some(10)),
            entry("cart", "cart-2", 11, "End", Some(10)),
            entry("cart", "cart-1", 9, "Log", None),
            entry("cart", "cart-1", 8, "Log", None),
        ]);

        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].entry_count, 2);
        assert_eq!(groups[0].kinds, ["End", "Start"]);
        assert_eq!(groups[1].agent_id, "cart-2");
        assert_eq!(groups[2].entry_count, 1);
        assert_eq!(groups[3].entry_count, 1);
    }
}
