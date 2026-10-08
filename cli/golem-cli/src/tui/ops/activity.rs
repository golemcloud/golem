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
use futures_util::StreamExt;
use golem_common::model::component::ComponentName;
use serde_json::Value;
use std::sync::Arc;

pub(in crate::tui) const SOFT_AGENT_LIMIT: usize = 10;
pub(in crate::tui) const HARD_AGENT_LIMIT: usize = 20;
pub(in crate::tui) const ENTRIES_PER_AGENT: u64 = 200;
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::tui) struct ActivityAgent {
    pub(in crate::tui) component: String,
    pub(in crate::tui) agent_id: String,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityEntry {
    pub(in crate::tui) component: String,
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) index: u64,
    pub(in crate::tui) kind: String,
    pub(in crate::tui) timestamp: Option<String>,
    pub(in crate::tui) detail: Value,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityFailure {
    pub(in crate::tui) agent_id: String,
    pub(in crate::tui) message: String,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct ActivityRefreshPayload {
    pub(in crate::tui) agents: Vec<ActivityAgent>,
    pub(in crate::tui) entries: Vec<ActivityEntry>,
    pub(in crate::tui) failures: Vec<ActivityFailure>,
}

pub(in crate::tui) struct ActivityState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<super::super::context_executor::TuiContextId>,
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) representation: ActivityRepresentation,
    pub(in crate::tui) agents: Vec<ActivityAgent>,
    pub(in crate::tui) entries: Vec<ActivityEntry>,
    pub(in crate::tui) failures: Vec<ActivityFailure>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) error: Option<String>,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            loading: false,
            representation: ActivityRepresentation::Grouped,
            agents: Vec::new(),
            entries: Vec::new(),
            failures: Vec::new(),
            selected: 0,
            error: None,
        }
    }
}

pub(in crate::tui) async fn load(
    context: Arc<Context>,
    agents: Vec<ActivityAgent>,
) -> anyhow::Result<ActivityRefreshPayload> {
    anyhow::ensure!(
        agents.len() <= HARD_AGENT_LIMIT,
        "Oplog is limited to {HARD_AGENT_LIMIT} Agents; narrow the scope or include fewer Agents"
    );
    let results = futures_util::stream::iter(agents.iter().cloned().map(|agent| {
        let context = context.clone();
        async move {
            let result = context
                .agent_handler()
                .oplog_snapshot(
                    &ComponentName(agent.component.clone()),
                    &RawAgentId(agent.agent_id.clone()),
                    ENTRIES_PER_AGENT,
                    None,
                )
                .await;
            (agent, result)
        }
    }))
    .buffer_unordered(MAX_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for (agent, result) in results {
        match result {
            Ok(agent_entries) => entries.extend(agent_entries.into_iter().map(|entry| {
                let detail = serde_json::to_value(&entry.entry).unwrap_or(Value::Null);
                ActivityEntry {
                    component: agent.component.clone(),
                    agent_id: agent.agent_id.clone(),
                    index: entry.index,
                    kind: entry_kind(&detail),
                    timestamp: entry_timestamp(&detail),
                    detail,
                }
            })),
            Err(error) => failures.push(ActivityFailure {
                agent_id: agent.agent_id,
                message: format!("{error:#}"),
            }),
        }
    }
    entries.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
            .then_with(|| right.index.cmp(&left.index))
    });
    Ok(ActivityRefreshPayload {
        agents,
        entries,
        failures,
    })
}

fn entry_kind(value: &Value) -> String {
    value
        .as_object()
        .and_then(|object| object.keys().next())
        .cloned()
        .unwrap_or_else(|| "entry".to_string())
}

fn entry_timestamp(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            if matches!(key.as_str(), "timestamp" | "time" | "createdAt") {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| Some(value.to_string()))
            } else {
                entry_timestamp(value)
            }
        }),
        Value::Array(values) => values.iter().find_map(entry_timestamp),
        _ => None,
    }
}
