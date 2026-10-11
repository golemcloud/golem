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
use crate::model::agent::oplog::{OplogEntryCategory, OplogEntryGroupKey, OplogEntryPresentation};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use golem_common::model::component::ComponentName;
use golem_common::model::oplog::OplogCursor;
use std::cell::Cell;
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ActivityOrder {
    Newest,
    Oldest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ActivityFocus {
    List,
    Details,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ActivityInputKind {
    Loaded,
    Server,
}

impl ActivityOrder {
    pub(in crate::tui) fn toggle(self) -> Self {
        match self {
            Self::Newest => Self::Oldest,
            Self::Oldest => Self::Newest,
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
    pub(in crate::tui) entry_indices: Vec<u64>,
    pub(in crate::tui) depth: usize,
    pub(in crate::tui) category: OplogEntryCategory,
    pub(in crate::tui) summary: String,
    pub(in crate::tui) status: ActivityGroupStatus,
    pub(in crate::tui) context_only: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) enum ActivityGroupStatus {
    Complete,
    Open,
    Orphan,
    Partial,
    Single,
}

impl ActivityGroupStatus {
    pub(in crate::tui) fn label(self) -> Option<&'static str> {
        match self {
            Self::Complete | Self::Single => None,
            Self::Open => Some("open"),
            Self::Orphan => Some("missing start"),
            Self::Partial => Some("partial"),
        }
    }
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
    pub(in crate::tui) input_kind: ActivityInputKind,
    pub(in crate::tui) query: String,
    pub(in crate::tui) applied_query: ActivityQuery,
    pub(in crate::tui) loaded_filter: String,
    pub(in crate::tui) agents: Vec<ActivityAgent>,
    pub(in crate::tui) entries: Vec<ActivityEntry>,
    pub(in crate::tui) progress: Vec<ActivityAgentProgress>,
    pub(in crate::tui) failures: Vec<ActivityFailure>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) details_visible: bool,
    pub(in crate::tui) focus: ActivityFocus,
    pub(in crate::tui) details_scroll: usize,
    pub(in crate::tui) details_content_height: Cell<usize>,
    pub(in crate::tui) details_viewport_height: Cell<usize>,
    pub(in crate::tui) list_first_visible: Cell<usize>,
    pub(in crate::tui) list_viewport_height: Cell<u16>,
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
            input_kind: ActivityInputKind::Loaded,
            query: String::new(),
            applied_query: ActivityQuery::default(),
            loaded_filter: String::new(),
            agents: Vec::new(),
            entries: Vec::new(),
            progress: Vec::new(),
            failures: Vec::new(),
            selected: 0,
            details_visible: true,
            focus: ActivityFocus::List,
            details_scroll: 0,
            details_content_height: Cell::new(0),
            details_viewport_height: Cell::new(0),
            list_first_visible: Cell::new(0),
            list_viewport_height: Cell::new(0),
            pending_focus: None,
            paused: false,
            stopped: false,
            error: None,
        }
    }
}

impl ActivityState {
    pub(in crate::tui) fn reveal_selection(&self, count: usize) {
        if count == 0 {
            self.list_first_visible.set(0);
            return;
        }
        let capacity = self.list_viewport_height.get().max(1) as usize;
        let mut first = self.list_first_visible.get().min(count.saturating_sub(1));
        if self.selected < first {
            first = self.selected;
        } else if self.selected >= first.saturating_add(capacity) {
            first = self.selected.saturating_add(1).saturating_sub(capacity);
        }
        self.list_first_visible
            .set(first.min(count.saturating_sub(capacity)));
    }

    pub(in crate::tui) fn clamp_details_scroll(&mut self) {
        let max = self
            .details_content_height
            .get()
            .saturating_sub(self.details_viewport_height.get());
        self.details_scroll = self.details_scroll.min(max);
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

pub(in crate::tui) fn filtered_entries<'a>(
    entries: &'a [ActivityEntry],
    loaded_filter: &str,
) -> Vec<&'a ActivityEntry> {
    entries
        .iter()
        .filter(|entry| entry_matches_loaded_filter(entry, loaded_filter))
        .collect()
}

fn entry_matches_loaded_filter(entry: &ActivityEntry, loaded_filter: &str) -> bool {
    let filter = loaded_filter.trim().to_lowercase();
    filter.is_empty()
        || entry.component.to_lowercase().contains(&filter)
        || entry.agent_id.to_lowercase().contains(&filter)
        || entry.presentation.matches_loaded_filter(&filter)
}

pub(in crate::tui) fn grouped_entries(
    entries: &[ActivityEntry],
    loaded_filter: &str,
    allow_inference: bool,
) -> Vec<ActivityGroup> {
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    enum Key {
        Group(String, String, OplogEntryGroupKey),
        Entry(String, String, u64),
    }

    #[derive(Clone)]
    struct Assignment {
        key: Key,
        parent: Option<Key>,
    }

    let mut by_agent = HashMap::<(String, String), Vec<&ActivityEntry>>::new();
    for entry in entries {
        by_agent
            .entry((entry.component.clone(), entry.agent_id.clone()))
            .or_default()
            .push(entry);
    }
    let mut assignments = HashMap::<(String, String, u64), Assignment>::new();
    for ((component, agent_id), mut agent_entries) in by_agent {
        agent_entries.sort_by_key(|entry| entry.presentation.index);
        let mut open_invocations = Vec::<u64>::new();
        for entry in agent_entries {
            let entry_key = || {
                Key::Entry(
                    component.clone(),
                    agent_id.clone(),
                    entry.presentation.index,
                )
            };
            let explicit_group = entry
                .presentation
                .group
                .as_ref()
                .map(|group| Key::Group(component.clone(), agent_id.clone(), group.clone()));
            let active_invocation = open_invocations.last().copied();
            let (key, inferred_parent) =
                if allow_inference && entry.presentation.kind == "AgentInvocationStarted" {
                    let key = Key::Group(
                        component.clone(),
                        agent_id.clone(),
                        OplogEntryGroupKey::OplogIndex(entry.presentation.index),
                    );
                    open_invocations.push(entry.presentation.index);
                    (key, None)
                } else if allow_inference && entry.presentation.kind == "AgentInvocationFinished" {
                    match open_invocations.pop() {
                        Some(start) => (
                            Key::Group(
                                component.clone(),
                                agent_id.clone(),
                                OplogEntryGroupKey::OplogIndex(start),
                            ),
                            None,
                        ),
                        None => (entry_key(), None),
                    }
                } else {
                    let key = explicit_group.unwrap_or_else(entry_key);
                    let parent = allow_inference.then(|| {
                        entry
                            .presentation
                            .structural_parent_group()
                            .map(|group| Key::Group(component.clone(), agent_id.clone(), group))
                            .or_else(|| {
                                active_invocation.map(|start| {
                                    Key::Group(
                                        component.clone(),
                                        agent_id.clone(),
                                        OplogEntryGroupKey::OplogIndex(start),
                                    )
                                })
                            })
                    });
                    (key, parent.flatten())
                };
            assignments.insert(
                (
                    component.clone(),
                    agent_id.clone(),
                    entry.presentation.index,
                ),
                Assignment {
                    key,
                    parent: inferred_parent,
                },
            );
        }
    }

    let mut positions = HashMap::<Key, usize>::new();
    let mut parents = HashMap::<Key, Key>::new();
    let mut matching = HashMap::<Key, bool>::new();
    let mut groups = Vec::<ActivityGroup>::new();
    for entry in entries {
        let assignment = assignments
            .get(&(
                entry.component.clone(),
                entry.agent_id.clone(),
                entry.presentation.index,
            ))
            .cloned()
            .unwrap_or_else(|| Assignment {
                key: Key::Entry(
                    entry.component.clone(),
                    entry.agent_id.clone(),
                    entry.presentation.index,
                ),
                parent: None,
            });
        let key = assignment.key;
        if let Some(parent) = assignment.parent {
            parents.entry(key.clone()).or_insert(parent);
        }
        matching
            .entry(key.clone())
            .and_modify(|matches| *matches |= entry_matches_loaded_filter(entry, loaded_filter))
            .or_insert_with(|| entry_matches_loaded_filter(entry, loaded_filter));
        if let Some(position) = positions.get(&key).copied() {
            let group = &mut groups[position];
            group.entry_count = group.entry_count.saturating_add(1);
            group.entry_indices.push(entry.presentation.index);
            if !group.kinds.contains(&entry.presentation.kind) {
                group.kinds.push(entry.presentation.kind.clone());
            }
            if group.summary.is_empty()
                || matches!(
                    entry.presentation.kind.as_str(),
                    "Start"
                        | "AgentInvocationStarted"
                        | "StartSpan"
                        | "BeginAtomicRegion"
                        | "BeginRemoteTransaction"
                )
            {
                group.summary = entry.presentation.summary();
            }
            if entry.presentation.category() == OplogEntryCategory::Failure {
                group.category = OplogEntryCategory::Failure;
            }
        } else {
            let assigned_group = match &key {
                Key::Group(_, _, group) => Some(group.clone()),
                Key::Entry(_, _, _) => None,
            };
            let first_index = assigned_group
                .as_ref()
                .and_then(OplogEntryGroupKey::start_index)
                .unwrap_or(entry.presentation.index);
            positions.insert(key, groups.len());
            groups.push(ActivityGroup {
                component: entry.component.clone(),
                agent_id: entry.agent_id.clone(),
                group: assigned_group,
                first_index,
                timestamp: entry.presentation.timestamp.to_string(),
                kinds: vec![entry.presentation.kind.clone()],
                entry_count: 1,
                entry_indices: vec![entry.presentation.index],
                depth: 0,
                category: entry.presentation.category(),
                summary: entry.presentation.summary(),
                status: ActivityGroupStatus::Single,
                context_only: false,
            });
        }
    }

    for (key, position) in &positions {
        let group = &mut groups[*position];
        group.depth = parent_depth(key, &parents);
        group.status = group_status(
            &group.kinds,
            group.entry_count,
            allow_inference,
            group.group.is_some(),
        );
    }

    if loaded_filter.trim().is_empty() {
        return groups;
    }
    let mut visible = matching
        .iter()
        .filter_map(|(key, matches)| matches.then_some(key.clone()))
        .collect::<std::collections::HashSet<_>>();
    let mut pending = visible.iter().cloned().collect::<Vec<_>>();
    while let Some(key) = pending.pop() {
        if let Some(parent) = parents.get(&key)
            && visible.insert(parent.clone())
        {
            pending.push(parent.clone());
        }
    }
    groups
        .into_iter()
        .filter_map(|mut group| {
            let key = group
                .group
                .as_ref()
                .map(|group_key| {
                    Key::Group(
                        group.component.clone(),
                        group.agent_id.clone(),
                        group_key.clone(),
                    )
                })
                .unwrap_or_else(|| {
                    Key::Entry(
                        group.component.clone(),
                        group.agent_id.clone(),
                        group.first_index,
                    )
                });
            visible.contains(&key).then(|| {
                group.context_only = !matching.get(&key).copied().unwrap_or(false);
                group
            })
        })
        .collect()
}

fn parent_depth<Key: Eq + std::hash::Hash + Clone>(
    key: &Key,
    parents: &HashMap<Key, Key>,
) -> usize {
    let mut current = key;
    let mut seen = std::collections::HashSet::new();
    let mut depth = 0usize;
    while let Some(parent) = parents.get(current) {
        if !seen.insert(parent.clone()) {
            break;
        }
        depth = depth.saturating_add(1).min(4);
        current = parent;
    }
    depth
}

fn group_status(
    kinds: &[String],
    entry_count: usize,
    allow_inference: bool,
    explicitly_grouped: bool,
) -> ActivityGroupStatus {
    if !allow_inference && !explicitly_grouped {
        return ActivityGroupStatus::Single;
    }
    let has = |kind: &str| kinds.iter().any(|candidate| candidate == kind);
    let has_start = has("Start")
        || has("AgentInvocationStarted")
        || has("StartSpan")
        || has("BeginAtomicRegion")
        || has("BeginRemoteTransaction");
    let has_terminal = has("End")
        || has("Cancelled")
        || has("AgentInvocationFinished")
        || has("FinishSpan")
        || has("EndAtomicRegion")
        || has("CommittedRemoteTransaction")
        || has("RolledBackRemoteTransaction");
    match (has_start, has_terminal) {
        (true, true) => ActivityGroupStatus::Complete,
        (true, false) => ActivityGroupStatus::Open,
        (false, true) => ActivityGroupStatus::Orphan,
        (false, false) if entry_count > 1 || explicitly_grouped => ActivityGroupStatus::Partial,
        (false, false) => ActivityGroupStatus::Single,
    }
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
                attribution: None,
                entry: None,
                search_text: std::sync::OnceLock::new(),
            },
        }
    }

    #[test]
    fn typed_groups_do_not_merge_agents_or_ambiguous_entries() {
        let groups = grouped_entries(
            &[
                entry("cart", "cart-1", 12, "End", Some(10)),
                entry("cart", "cart-1", 10, "Start", Some(10)),
                entry("cart", "cart-2", 11, "End", Some(10)),
                entry("cart", "cart-1", 9, "Log", None),
                entry("cart", "cart-1", 8, "Log", None),
            ],
            "",
            true,
        );

        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].entry_count, 2);
        assert_eq!(groups[0].kinds, ["End", "Start"]);
        assert_eq!(groups[1].agent_id, "cart-2");
        assert_eq!(groups[2].entry_count, 1);
        assert_eq!(groups[3].entry_count, 1);
    }

    #[test]
    fn lenient_invocation_grouping_keeps_partial_and_overlapping_history() {
        let groups = grouped_entries(
            &[
                entry("cart", "cart-1", 8, "AgentInvocationFinished", None),
                entry("cart", "cart-1", 7, "AgentInvocationFinished", None),
                entry("cart", "cart-1", 6, "Log", None),
                entry("cart", "cart-1", 5, "AgentInvocationStarted", None),
                entry("cart", "cart-1", 4, "AgentInvocationStarted", None),
                entry("cart", "cart-1", 3, "AgentInvocationFinished", None),
            ],
            "",
            true,
        );

        assert!(groups.iter().any(|group| {
            group.first_index == 5 && group.status == ActivityGroupStatus::Complete
        }));
        assert!(groups.iter().any(|group| {
            group.first_index == 4 && group.status == ActivityGroupStatus::Complete
        }));
        assert!(groups.iter().any(|group| {
            group.first_index == 3 && group.status == ActivityGroupStatus::Orphan
        }));
    }

    #[test]
    fn sparse_server_results_do_not_infer_invocation_pairs() {
        let groups = grouped_entries(
            &[
                entry("cart", "cart-1", 12, "AgentInvocationFinished", None),
                entry("cart", "cart-1", 10, "AgentInvocationStarted", None),
            ],
            "",
            false,
        );

        assert_eq!(groups.len(), 2);
        assert!(
            groups
                .iter()
                .all(|group| group.status == ActivityGroupStatus::Single)
        );
    }

    #[test]
    fn loaded_filter_keeps_a_matching_childs_inferred_parent() {
        let groups = grouped_entries(
            &[
                entry("cart", "cart-1", 3, "AgentInvocationFinished", None),
                entry("cart", "cart-1", 2, "Needle", None),
                entry("cart", "cart-1", 1, "AgentInvocationStarted", None),
            ],
            "needle",
            true,
        );

        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|group| group.context_only));
        assert!(groups.iter().any(|group| group.kinds == ["Needle"]));
    }
}
