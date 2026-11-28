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

//! Read-only queries over a public oplog, as `get_oplog` returns it. The queries only select
//! entries; every expectation stays in the test that states it.

use anyhow::{anyhow, ensure};
use golem_common::model::InvocationId;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::schema::FromSchema;

/// The entries that refer to one durable call, selected by the oplog index of its `Start`.
#[derive(Debug)]
pub struct CallRecord<'a> {
    pub start: Option<&'a PublicOplogEntryWithIndex>,
    pub ends: Vec<&'a PublicOplogEntryWithIndex>,
    pub cancellations: Vec<&'a PublicOplogEntryWithIndex>,
    pub deliveries: Vec<&'a PublicOplogEntryWithIndex>,
    pub discards: Vec<&'a PublicOplogEntryWithIndex>,
}

impl CallRecord<'_> {
    /// The number of `End` and `Cancelled` entries of the call.
    pub fn terminal_count(&self) -> usize {
        self.ends.len() + self.cancellations.len()
    }

    /// Decodes the response of the call's single `End`. Fails when the call has no `End`,
    /// more than one, or an `End` without a response.
    pub fn end_response<T: FromSchema>(&self) -> anyhow::Result<T> {
        ensure!(
            self.ends.len() == 1,
            "expected exactly one End, found {}",
            self.ends.len()
        );
        let PublicOplogEntry::End(end) = &self.ends[0].entry else {
            unreachable!("CallRecord::ends holds only End entries")
        };
        let response = end
            .response
            .as_ref()
            .ok_or_else(|| anyhow!("the End at {} has no response", self.ends[0].oplog_index))?;
        T::from_value(response.value()).map_err(|error| anyhow!("{error:?}"))
    }
}

/// The entries of one agent invocation, selected by its invocation ID.
#[derive(Debug)]
pub struct InvocationRecord<'a> {
    pub started: Vec<&'a PublicOplogEntryWithIndex>,
    /// `AgentInvocationFinished` and `AgentInvocationFailed` entries.
    pub terminals: Vec<&'a PublicOplogEntryWithIndex>,
    pub settled: Vec<&'a PublicOplogEntryWithIndex>,
}

/// Queries over a public oplog.
pub trait OplogView {
    /// The `Start` entries of the host function with this name, in oplog order.
    fn starts_of(&self, function_name: &str) -> Vec<&PublicOplogEntryWithIndex>;

    /// The `Start` and the terminals and completion markers of the call that starts at `start`.
    fn call(&self, start: OplogIndex) -> CallRecord<'_>;

    /// The `AgentInvocationStarted`, terminal and `AgentInvocationSettled` entries of an
    /// invocation. Snapshot and manual-update starts carry no invocation ID, so they are never
    /// in `started`.
    fn invocation(&self, invocation_id: &InvocationId) -> InvocationRecord<'_>;

    /// One line per entry, such as `12: Start(golem:websocket/client::receive), 13: End(@12)`,
    /// for assertion messages.
    fn describe(&self) -> String;
}

impl OplogView for [PublicOplogEntryWithIndex] {
    fn starts_of(&self, function_name: &str) -> Vec<&PublicOplogEntryWithIndex> {
        self.iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == function_name)
            })
            .collect()
    }

    fn call(&self, start: OplogIndex) -> CallRecord<'_> {
        let mut record = CallRecord {
            start: None,
            ends: Vec::new(),
            cancellations: Vec::new(),
            deliveries: Vec::new(),
            discards: Vec::new(),
        };
        for entry in self {
            match &entry.entry {
                PublicOplogEntry::Start(_) if entry.oplog_index == start => {
                    record.start = Some(entry)
                }
                PublicOplogEntry::End(end) if end.start_index == start => record.ends.push(entry),
                PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start => {
                    record.cancellations.push(entry)
                }
                PublicOplogEntry::CompletionDelivered(delivered)
                    if delivered.start_index == start =>
                {
                    record.deliveries.push(entry)
                }
                PublicOplogEntry::CompletionDiscarded(discarded)
                    if discarded.start_index == start =>
                {
                    record.discards.push(entry)
                }
                _ => {}
            }
        }
        record
    }

    fn invocation(&self, invocation_id: &InvocationId) -> InvocationRecord<'_> {
        let mut record = InvocationRecord {
            started: Vec::new(),
            terminals: Vec::new(),
            settled: Vec::new(),
        };
        for entry in self {
            match &entry.entry {
                PublicOplogEntry::AgentInvocationStarted(started)
                    if started_invocation_id(&started.invocation) == Some(invocation_id) =>
                {
                    record.started.push(entry)
                }
                PublicOplogEntry::AgentInvocationFinished(finished)
                    if &finished.invocation_id == invocation_id =>
                {
                    record.terminals.push(entry)
                }
                PublicOplogEntry::AgentInvocationFailed(failed)
                    if &failed.invocation_id == invocation_id =>
                {
                    record.terminals.push(entry)
                }
                PublicOplogEntry::AgentInvocationSettled(settled)
                    if &settled.invocation_id == invocation_id =>
                {
                    record.settled.push(entry)
                }
                _ => {}
            }
        }
        record
    }

    fn describe(&self) -> String {
        self.iter()
            .map(|entry| format!("{}: {}", entry.oplog_index, describe_entry(&entry.entry)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The invocation ID an `AgentInvocationStarted` carries, when its kind has one.
pub(crate) fn started_invocation_id(invocation: &PublicAgentInvocation) -> Option<&InvocationId> {
    match invocation {
        PublicAgentInvocation::AgentInitialization(params) => Some(&params.invocation_id),
        PublicAgentInvocation::AgentMethodInvocation(params) => Some(&params.invocation_id),
        PublicAgentInvocation::ExternalTool(params) => Some(&params.invocation_id),
        PublicAgentInvocation::ProcessOplogEntries(params) => Some(&params.invocation_id),
        PublicAgentInvocation::SaveSnapshot(_)
        | PublicAgentInvocation::LoadSnapshot(_)
        | PublicAgentInvocation::ManualUpdate(_) => None,
    }
}

fn describe_entry(entry: &PublicOplogEntry) -> String {
    match entry {
        PublicOplogEntry::Start(params) => format!("Start({})", params.function_name),
        PublicOplogEntry::End(params) => format!("End(@{})", params.start_index),
        PublicOplogEntry::Cancelled(params) => format!("Cancelled(@{})", params.start_index),
        PublicOplogEntry::CompletionDelivered(params) => {
            format!("CompletionDelivered(@{})", params.start_index)
        }
        PublicOplogEntry::CompletionDiscarded(params) => {
            format!("CompletionDiscarded(@{})", params.start_index)
        }
        PublicOplogEntry::AgentInvocationStarted(params) => match &params.invocation {
            PublicAgentInvocation::AgentMethodInvocation(invocation) => {
                format!("AgentInvocationStarted({})", invocation.method_name)
            }
            _ => "AgentInvocationStarted".to_string(),
        },
        PublicOplogEntry::AgentInvocationFinished(params) => format!(
            "AgentInvocationFinished({})",
            params.method_name.as_deref().unwrap_or("?")
        ),
        other => other.entry_type().to_string(),
    }
}
