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

use crate::agent_id_display::SourceLanguage;
use crate::log::logln;
use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::*;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use golem_common::model::Timestamp;
use golem_common::model::oplog::{
    MultipartPartData, PluginInstallationDescription, PublicAgentInvocation,
    PublicAgentInvocationResult, PublicAttributeValue, PublicOplogEntry, PublicSnapshotData,
    PublicUpdateDescription, StringAttributeValue,
};
use golem_common::schema::TypedSchemaValue;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::model::agent::{AgentMetadataView, AgentsMetadataResponseView, RawAgentId};
#[cfg(test)]
use golem_common::model::AgentStatus;
#[cfg(test)]
use golem_common::model::component::ComponentName;
#[cfg(test)]
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOplogEntryView {
    pub index: u64,
    pub entry: PublicOplogEntry,
}

impl StructuredOutput for AgentOplogEntryView {
    const KIND: &'static str = "agent.oplog";
}

impl TextOutput for AgentOplogEntryView {
    fn log(&self) {
        logln(format!(
            "{}: ",
            format_main_id(&format!("#{:0>5}", self.index))
        ));
        self.entry.log()
    }
}

impl TextOutput for PublicOplogEntry {
    fn log(&self) {
        let pad = "          ";
        match self {
            PublicOplogEntry::Create(params) => {
                logln(format_message_highlight("CREATE"));
                logln(format!(
                    "{pad}at:                 {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}component revision: {}",
                    format_id(&params.component_revision),
                ));
                logln(format!("{pad}env:"));
                for (k, v) in &params.env {
                    logln(format!("{pad}  - {}: {}", k, format_id(&v)));
                }
                if let Some(parent) = params.parent.as_ref() {
                    logln(format!("{pad}parent:            {}", format_id(parent)));
                }
                logln(format!("{pad}initial active plugins:"));
                for plugin in &params.initial_active_plugins {
                    logln(format!(
                        "{pad}  - priority: {}",
                        format_id(&plugin.plugin_priority.0)
                    ));
                    let inner_pad = format!("{pad}    ");
                    log_plugin_description(&inner_pad, plugin);
                }
            }
            PublicOplogEntry::Start(params) => {
                logln(format!(
                    "{} {}",
                    format_message_highlight("START"),
                    format_id(&params.function_name),
                ));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                if let Some(parent) = params.parent_start_index {
                    logln(format!("{pad}parent start index: {}", parent));
                }
                if let Some(request) = &params.request {
                    logln(format!(
                        "{pad}input:             {}",
                        typed_schema_value_to_string(request)
                    ));
                }
            }
            PublicOplogEntry::End(params) => {
                logln(format_message_highlight("END"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}start index:       {}", params.start_index));
                if let Some(response) = &params.response {
                    logln(format!(
                        "{pad}result:            {}",
                        typed_schema_value_to_string(response)
                    ));
                }
                if params.forced_commit {
                    logln(format!("{pad}forced commit:     true"));
                }
            }
            PublicOplogEntry::Cancelled(params) => {
                logln(format_message_highlight("CANCELLED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}start index:       {}", params.start_index));
                if let Some(partial) = &params.partial {
                    logln(format!(
                        "{pad}partial result:    {}",
                        typed_schema_value_to_string(partial)
                    ));
                }
            }
            PublicOplogEntry::CompletionDiscarded(params) => {
                logln(format_message_highlight("COMPLETION DISCARDED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}start index:       {}", params.start_index));
            }
            PublicOplogEntry::CompletionDelivered(params) => {
                logln(format_message_highlight("COMPLETION DELIVERED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}start index:       {}", params.start_index));
            }
            PublicOplogEntry::AgentInvocationStarted(params) => {
                log_agent_invocation(
                    AgentInvocationRenderKind::Started,
                    pad,
                    &params.timestamp,
                    &params.invocation,
                );
            }
            PublicOplogEntry::AgentInvocationFinished(params) => {
                let variant_label = match &params.result {
                    PublicAgentInvocationResult::AgentInitialization(_) => "initialization",
                    PublicAgentInvocationResult::AgentMethod(_) => "method",
                    PublicAgentInvocationResult::ManualUpdate(_) => "manual update",
                    PublicAgentInvocationResult::LoadSnapshot(_) => "load snapshot",
                    PublicAgentInvocationResult::SaveSnapshot(_) => "save snapshot",
                    PublicAgentInvocationResult::ProcessOplogEntries(_) => "process oplog entries",
                };
                logln(format!(
                    "{} ({})",
                    format_message_highlight("INVOKE COMPLETED"),
                    variant_label
                ));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}consumed fuel:     {}",
                    format_id(&params.consumed_fuel),
                ));
                match &params.result {
                    PublicAgentInvocationResult::AgentInitialization(output)
                    | PublicAgentInvocationResult::AgentMethod(output) => {
                        logln(format!("{pad}output:"));
                        log_typed_schema_value(pad, &output.output, &SourceLanguage::default());
                    }
                    PublicAgentInvocationResult::ManualUpdate(_) => {}
                    PublicAgentInvocationResult::LoadSnapshot(fallible) => {
                        log_optional_error(pad, &fallible.error);
                    }
                    PublicAgentInvocationResult::ProcessOplogEntries(result) => {
                        log_optional_error(pad, &result.error);
                    }
                    PublicAgentInvocationResult::SaveSnapshot(snapshot_result) => {
                        log_snapshot_data(pad, &snapshot_result.snapshot);
                    }
                }
            }
            PublicOplogEntry::Suspend(params) => {
                logln(format_message_highlight("SUSPEND"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::Error(params) => {
                logln(format_message_highlight("ERROR"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}retry from:        {}", params.retry_from));
                logln(format!(
                    "{pad}error:             {}",
                    format_error(&params.error)
                ));
            }
            PublicOplogEntry::NoOp(params) => {
                logln(format_message_highlight("NOP"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::Jump(params) => {
                logln(format_message_highlight("JUMP"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}from:              {}",
                    format_id(&params.jump.start)
                ));
                logln(format!(
                    "{pad}to:                {}",
                    format_id(&params.jump.end)
                ));
            }
            PublicOplogEntry::Interrupted(params) => {
                logln(format_message_highlight("INTERRUPTED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::Exited(params) => {
                logln(format_message_highlight("EXITED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::BeginAtomicRegion(params) => {
                logln(format_message_highlight("BEGIN ATOMIC REGION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::EndAtomicRegion(params) => {
                logln(format_message_highlight("END ATOMIC REGION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}begin index:       {}",
                    format_id(&params.begin_index)
                ));
            }

            PublicOplogEntry::PendingAgentInvocation(params) => {
                log_agent_invocation(
                    AgentInvocationRenderKind::Pending,
                    pad,
                    &params.timestamp,
                    &params.invocation,
                );
            }
            PublicOplogEntry::PendingUpdate(params) => {
                logln(format_message_highlight("ENQUEUED UPDATE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}target revision:   {}",
                    format_id(&params.target_revision),
                ));
                match &params.description {
                    PublicUpdateDescription::Automatic(_) => {
                        logln(format!(
                            "{pad}type:              {}",
                            format_id("automatic")
                        ));
                    }
                    PublicUpdateDescription::SnapshotBased(inner_params) => {
                        logln(format!(
                            "{pad}type:              {}",
                            format_id("snapshot based")
                        ));
                        logln(format!(
                            "{pad}snapshot:          {}",
                            BASE64_STANDARD.encode(&inner_params.payload),
                        ));
                    }
                }
            }
            PublicOplogEntry::SuccessfulUpdate(params) => {
                logln(format_message_highlight("SUCCESSFUL UPDATE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}target revision:   {}",
                    format_id(&params.target_revision),
                ));
                logln(format!("{pad}new active plugins:"));
                for plugin in &params.new_active_plugins {
                    logln(format!(
                        "{pad}  - priority: {}",
                        format_id(&plugin.plugin_priority.0),
                    ));
                    let inner_pad = format!("{pad}    ");
                    log_plugin_description(&inner_pad, plugin);
                }
            }
            PublicOplogEntry::FailedUpdate(params) => {
                logln(format_message_highlight("FAILED UPDATE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}target revision:   {}",
                    format_id(&params.target_revision),
                ));
                if let Some(details) = &params.details {
                    logln(format!("{pad}error:             {}", format_error(details)));
                }
            }
            PublicOplogEntry::GrowMemory(params) => {
                logln(format_message_highlight("GROW MEMORY"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}increase:          {}",
                    format_id(&format_binary_size(&params.delta)),
                ));
            }
            PublicOplogEntry::FilesystemStorageUsageUpdate(params) => {
                logln(format_message_highlight("STORAGE USAGE UPDATE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}delta:             {}",
                    format_id(&params.delta.to_string()),
                ));
            }
            PublicOplogEntry::CreateResource(params) => {
                logln(format_message_highlight("CREATE RESOURCE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}resource id:       {}", format_id(&params.id)));
            }
            PublicOplogEntry::DropResource(params) => {
                logln(format_message_highlight("DROP RESOURCE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!("{pad}resource id:       {}", format_id(&params.id)));
            }
            PublicOplogEntry::Log(params) => {
                logln(format_message_highlight("LOG"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}level:             {}",
                    format_id(&format!("{:?}", params.level)),
                ));
                logln(format!("{pad}message:           {}", params.message));
            }
            PublicOplogEntry::Restart(params) => {
                logln(format_message_highlight("RESTART"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::ActivatePlugin(params) => {
                logln(format_message_highlight("ACTIVATE PLUGIN"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}priority:   {}",
                    format_id(&params.plugin.plugin_priority.0),
                ));
                log_plugin_description(pad, &params.plugin);
            }
            PublicOplogEntry::DeactivatePlugin(params) => {
                logln(format_message_highlight("DEACTIVATE PLUGIN"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}priority:   {}",
                    format_id(&params.plugin.plugin_priority.0),
                ));
                log_plugin_description(pad, &params.plugin);
            }
            PublicOplogEntry::Revert(params) => {
                logln(format_message_highlight("REVERT"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}to oplog index:    {}",
                    format_id(&params.dropped_region.start.previous()),
                ));
            }
            PublicOplogEntry::CancelPendingInvocation(params) => {
                logln(format_message_highlight("CANCEL INVOCATION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}idempotency key:   {}",
                    format_id(&params.idempotency_key),
                ));
            }
            PublicOplogEntry::StartSpan(params) => {
                logln(format_message_highlight("START SPAN"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}span id:           {}",
                    format_id(&params.span_id)
                ));
                if let Some(parent_id) = &params.parent_id {
                    logln(format!("{pad}parent span:       {}", format_id(&parent_id),));
                }
                if let Some(linked_id) = &params.linked_context {
                    logln(format!("{pad}linked span:       {}", format_id(&linked_id),));
                }
                logln(format!("{pad}attributes:"));
                for kv in &params.attributes {
                    logln(format!(
                        "{pad}  - {}: {}",
                        kv.key,
                        match &kv.value {
                            PublicAttributeValue::String(StringAttributeValue { value }) =>
                                format_id(value),
                        }
                    ));
                }
            }
            PublicOplogEntry::FinishSpan(params) => {
                logln(format_message_highlight("FINISH SPAN"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}span id:           {}",
                    format_id(&params.span_id)
                ));
            }
            PublicOplogEntry::SetSpanAttribute(params) => {
                logln(format_message_highlight("SET SPAN ATTRIBUTE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}span id:           {}",
                    format_id(&params.span_id)
                ));
                logln(format!(
                    "{pad}key:               {}",
                    format_id(&params.key)
                ));
                logln(format!(
                    "{pad}value:             {}",
                    match &params.value {
                        PublicAttributeValue::String(StringAttributeValue { value }) =>
                            format_id(value),
                    }
                ));
            }
            PublicOplogEntry::ChangePersistenceLevel(params) => {
                logln(format_message_highlight("CHANGE PERSISTENCE LEVEL"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}level:             {}",
                    format_id(&format!("{:?}", params.persistence_level))
                ));
            }
            PublicOplogEntry::BeginRemoteTransaction(params) => {
                logln(format_message_highlight("BEGIN REMOTE TRANSACTION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}transaction id:          {}",
                    format_id(&params.transaction_id)
                ));
            }
            PublicOplogEntry::PreCommitRemoteTransaction(params) => {
                logln(format_message_highlight("PRE COMMIT REMOTE TRANSACTION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}begin index:       {}",
                    format_id(&params.begin_index)
                ));
            }
            PublicOplogEntry::PreRollbackRemoteTransaction(params) => {
                logln(format_message_highlight("PRE ROLLBACK REMOTE TRANSACTION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}begin index:       {}",
                    format_id(&params.begin_index)
                ));
            }
            PublicOplogEntry::CommittedRemoteTransaction(params) => {
                logln(format_message_highlight("COMMITTED REMOTE TRANSACTION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}begin index:       {}",
                    format_id(&params.begin_index)
                ));
            }
            PublicOplogEntry::RolledBackRemoteTransaction(params) => {
                logln(format_message_highlight("ROLLED BACK REMOTE TRANSACTION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}begin index:       {}",
                    format_id(&params.begin_index)
                ));
            }
            PublicOplogEntry::Snapshot(params) => {
                logln(format_message_highlight("SNAPSHOT"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                log_snapshot_data(pad, &params.data);
            }
            PublicOplogEntry::OplogProcessorCheckpoint(params) => {
                logln(format_message_highlight("OPLOG PROCESSOR CHECKPOINT"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}plugin:            {} v{}",
                    format_id(&params.plugin.plugin_name),
                    format_id(&params.plugin.plugin_version)
                ));
                logln(format!(
                    "{pad}target:            {}",
                    format_id(&params.target_agent_id)
                ));
                logln(format!(
                    "{pad}confirmed up to:   {}",
                    format_id(&params.confirmed_up_to)
                ));
                logln(format!(
                    "{pad}sending up to:     {}",
                    format_id(&params.sending_up_to)
                ));
            }
            PublicOplogEntry::SetRetryPolicy(params) => {
                logln(format_message_highlight("SET RETRY POLICY"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}name:              {}",
                    format_id(&params.policy.name)
                ));
                logln(format!(
                    "{pad}priority:          {}",
                    format_id(&params.policy.priority)
                ));
            }
            PublicOplogEntry::RemoveRetryPolicy(params) => {
                logln(format_message_highlight("REMOVE RETRY POLICY"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}name:              {}",
                    format_id(&params.name)
                ));
            }
            PublicOplogEntry::CardRevoked(params) => {
                logln(format_message_highlight("CARD REVOKED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}queued event:      {}",
                    format_id(&format!("{:?}", params.queued_event_index))
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.card_id)
                ));
            }
            PublicOplogEntry::HostStreamFrame(params) => {
                logln(format_message_highlight("HOST STREAM FRAME"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}parent start index: {}",
                    params.parent_start_index
                ));
                logln(format!(
                    "{pad}kind:              {}",
                    format_id(&format!("{:?}", params.kind))
                ));
                logln(format!(
                    "{pad}payload:           {}",
                    typed_schema_value_to_string(&params.payload)
                ));
            }
            PublicOplogEntry::CardExpired(params) => {
                logln(format_message_highlight("CARD EXPIRED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.card_id)
                ));
            }
            PublicOplogEntry::CardEventQueued(params) => {
                logln(format_message_highlight("CARD EVENT QUEUED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.event.card_id())
                ));
            }
            PublicOplogEntry::CardInstalled(params) => {
                logln(format_message_highlight("CARD INSTALLED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}queued event:      {}",
                    format_id(&format!("{:?}", params.queued_event_index))
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.card_id)
                ));
            }
            PublicOplogEntry::CardInstallFailed(params) => {
                logln(format_message_highlight("CARD INSTALL FAILED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}queued event:      {}",
                    format_id(&params.queued_event_index)
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.card_id)
                ));
                logln(format!(
                    "{pad}reason:            {}",
                    format_id(&format!("{:?}", params.reason))
                ));
            }
        }
    }
}

fn log_plugin_description(pad: &str, value: &PluginInstallationDescription) {
    logln(format!(
        "{pad}plugin name:       {}",
        format_id(&value.plugin_name)
    ));
    logln(format!(
        "{pad}plugin version:    {}",
        format_id(&value.plugin_version),
    ));
    logln(format!(
        "{pad}plugin parameters:    {}",
        format_id(&value.plugin_version),
    ));
    for (k, v) in &value.parameters {
        logln(format!("{pad}  - {}: {}", k, format_id(&v)));
    }
}

#[derive(Clone, Copy)]
enum AgentInvocationRenderKind {
    Started,
    Pending,
}

fn log_agent_invocation(
    kind: AgentInvocationRenderKind,
    pad: &str,
    timestamp: &Timestamp,
    invocation: &PublicAgentInvocation,
) {
    for line in render_agent_invocation(kind, pad, timestamp, invocation) {
        logln(line);
    }
}

fn render_agent_invocation(
    kind: AgentInvocationRenderKind,
    pad: &str,
    timestamp: &Timestamp,
    invocation: &PublicAgentInvocation,
) -> Vec<String> {
    let mut lines = vec![
        render_agent_invocation_header(kind, invocation),
        format!("{pad}at:                {}", format_id(timestamp)),
    ];

    match invocation {
        PublicAgentInvocation::AgentInitialization(params) => {
            lines.push(format!(
                "{pad}idempotency key:   {}",
                format_id(&params.idempotency_key)
            ));
            lines.push(format!("{pad}input:"));
            lines.push(render_typed_schema_value_line(
                pad,
                &params.constructor_parameters,
                &SourceLanguage::default(),
            ));
        }
        PublicAgentInvocation::AgentMethodInvocation(params) => {
            lines.push(format!(
                "{pad}idempotency key:   {}",
                format_id(&params.idempotency_key)
            ));
            lines.push(format!("{pad}input:"));
            lines.push(render_typed_schema_value_line(
                pad,
                &params.function_input,
                &SourceLanguage::default(),
            ));
        }
        PublicAgentInvocation::SaveSnapshot(_) => {}
        PublicAgentInvocation::LoadSnapshot(params) => {
            lines.extend(render_snapshot_data_lines(pad, &params.snapshot));
        }
        PublicAgentInvocation::ProcessOplogEntries(params) => {
            lines.push(format!(
                "{pad}idempotency key:   {}",
                format_id(&params.idempotency_key)
            ));
        }
        PublicAgentInvocation::ManualUpdate(params) => {
            lines.push(format!(
                "{pad}target revision:   {}",
                format_id(&params.target_revision)
            ));
        }
    }

    lines
}

fn render_agent_invocation_header(
    kind: AgentInvocationRenderKind,
    invocation: &PublicAgentInvocation,
) -> String {
    match (kind, invocation) {
        (AgentInvocationRenderKind::Started, PublicAgentInvocation::AgentInitialization(_)) => {
            format!(
                "{} {}",
                format_message_highlight("INVOKE"),
                format_id("initialize")
            )
        }
        (
            AgentInvocationRenderKind::Started,
            PublicAgentInvocation::AgentMethodInvocation(params),
        ) => {
            format!(
                "{} {}",
                format_message_highlight("INVOKE"),
                format_id(&params.method_name)
            )
        }
        (AgentInvocationRenderKind::Started, PublicAgentInvocation::SaveSnapshot(_)) => {
            format!(
                "{} {}",
                format_message_highlight("INVOKE"),
                format_id("save snapshot")
            )
        }
        (AgentInvocationRenderKind::Started, PublicAgentInvocation::LoadSnapshot(_)) => {
            format!(
                "{} {}",
                format_message_highlight("INVOKE"),
                format_id("load snapshot")
            )
        }
        (AgentInvocationRenderKind::Started, PublicAgentInvocation::ProcessOplogEntries(_)) => {
            format!(
                "{} {}",
                format_message_highlight("INVOKE"),
                format_id("process oplog entries")
            )
        }
        (AgentInvocationRenderKind::Started, PublicAgentInvocation::ManualUpdate(_)) => {
            format!(
                "{} {}",
                format_message_highlight("INVOKE"),
                format_id("manual update")
            )
        }
        (AgentInvocationRenderKind::Pending, PublicAgentInvocation::AgentInitialization(_)) => {
            format_message_highlight("ENQUEUED AGENT INITIALIZATION")
        }
        (
            AgentInvocationRenderKind::Pending,
            PublicAgentInvocation::AgentMethodInvocation(params),
        ) => {
            format!(
                "{} {}",
                format_message_highlight("ENQUEUED INVOCATION"),
                format_id(&params.method_name)
            )
        }
        (AgentInvocationRenderKind::Pending, PublicAgentInvocation::SaveSnapshot(_)) => {
            format_message_highlight("ENQUEUED SAVE SNAPSHOT")
        }
        (AgentInvocationRenderKind::Pending, PublicAgentInvocation::LoadSnapshot(_)) => {
            format_message_highlight("ENQUEUED LOAD SNAPSHOT")
        }
        (AgentInvocationRenderKind::Pending, PublicAgentInvocation::ProcessOplogEntries(_)) => {
            format_message_highlight("ENQUEUED PROCESS OPLOG ENTRIES")
        }
        (AgentInvocationRenderKind::Pending, PublicAgentInvocation::ManualUpdate(_)) => {
            format_message_highlight("ENQUEUED MANUAL UPDATE")
        }
    }
}

fn typed_schema_value_to_string(value: &TypedSchemaValue) -> String {
    golem_common::schema::render::value_to_cli_text(value.graph(), value.root_type(), value.value())
        .unwrap_or_else(|err| format!("<rendering error: {err}>"))
}

fn log_typed_schema_value(pad: &str, value: &TypedSchemaValue, source_language: &SourceLanguage) {
    logln(render_typed_schema_value_line(pad, value, source_language));
}

fn render_typed_schema_value_line(
    pad: &str,
    value: &TypedSchemaValue,
    source_language: &SourceLanguage,
) -> String {
    let rendered = crate::agent_id_display::render_typed_schema_value(value, source_language);
    format!("{pad}  {rendered}")
}

fn log_optional_error(pad: &str, error: &Option<String>) {
    match error {
        None => {
            logln(format!("{pad}result:            ok"));
        }
        Some(err) => {
            logln(format!("{pad}error:             {}", format_error(err)));
        }
    }
}

fn log_snapshot_data(pad: &str, snapshot: &PublicSnapshotData) {
    for line in render_snapshot_data_lines(pad, snapshot) {
        logln(line);
    }
}

fn render_snapshot_data_lines(pad: &str, snapshot: &PublicSnapshotData) -> Vec<String> {
    match snapshot {
        PublicSnapshotData::Raw(raw) => vec![
            format!("{pad}mime type:         {}", format_id(&raw.mime_type)),
            format!("{pad}data:              ({} bytes)", raw.data.len()),
        ],
        PublicSnapshotData::Json(json) => vec![format!(
            "{pad}data:              {}",
            serde_json::to_string_pretty(&json.data).unwrap_or_else(|_| format!("{:?}", json.data))
        )],
        PublicSnapshotData::Multipart(multipart) => {
            let mut lines = vec![format!(
                "{pad}mime type:         {}",
                format_id(&multipart.mime_type)
            )];
            for part in &multipart.parts {
                let data_summary = match &part.data {
                    MultipartPartData::Json(json) => serde_json::to_string_pretty(&json.data)
                        .unwrap_or_else(|_| format!("{:?}", json.data)),
                    MultipartPartData::Raw(raw) => {
                        format!("({} bytes)", raw.data.len())
                    }
                };
                lines.push(format!(
                    "{pad}part:              {} [{}]: {}",
                    part.name, part.content_type, data_summary
                ));
            }
            lines
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use comfy_table::{Cell, Table as ComfyTable};
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::{
        AgentInitializationParameters, AgentMethodInvocationParameters, LoadSnapshotParameters,
        ManualUpdateParameters, ProcessOplogEntriesParameters, RawSnapshotData,
    };
    use golem_common::model::{Empty, IdempotencyKey};
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
    use test_r::test;

    fn timestamp() -> Timestamp {
        Timestamp::from(0)
    }

    fn agent_metadata(agent_id: &str) -> AgentMetadataView {
        AgentMetadataView {
            component_name: ComponentName("shop:cart".to_string()),
            agent_id: RawAgentId(agent_id.to_string()),
            created_by: golem_common::model::account::AccountId(uuid::Uuid::nil()),
            environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::nil()),
            env: HashMap::new(),
            default_env: HashMap::new(),
            config: Vec::new(),
            default_config: Vec::new(),
            status: AgentStatus::Running,
            component_revision: ComponentRevision::new(1).expect("valid revision"),
            retry_count: 0,
            pending_invocation_count: 0,
            updates: Vec::new(),
            created_at: timestamp(),
            last_error: None,
            component_size: 0,
            total_linear_memory_size: 0,
            exported_resource_instances: HashMap::new(),
            source_language: SourceLanguage::Rust,
            secret_config_paths: std::collections::BTreeSet::new(),
        }
    }

    /// Long agent ids are pre-formatted, so they have to break at their own
    /// structure inside the cell instead of being wrapped mid-token, without
    /// disturbing the table layout.
    #[test]
    fn table_breaks_long_agent_ids_structurally() {
        let agents = vec![
            agent_metadata(
                r#"ShoppingCart(user: "a-fairly-long-user-identifier", items: ["one", "two"])"#,
            ),
            agent_metadata(r#"Counter("short")"#),
        ];

        let table = AgentsMetadataResponseView::format_table_wide(&agents, 120, false, false);

        // The indent only appears if we broke the id ourselves; the table's own
        // wrapping would split it mid-token and would not indent.
        assert!(
            table.contains(r#"  user: "a-fairly-long-user-identifier","#),
            "long id was not broken at its structure:\n{table}"
        );
        assert_rows_aligned(&table);
    }

    /// Agent id cells carry their own coloring, which only lines up if
    /// comfy-table measures cell content with the ANSI escapes stripped. That
    /// needs its `custom_styling` feature, so this fails if the feature is
    /// dropped. The escapes are written out here because the `colored` crate
    /// emits none while colors are globally off, as they are under tests.
    #[test]
    fn table_measures_cells_with_ansi_escapes_stripped() {
        let mut table = ComfyTable::new();
        table
            .set_header(vec!["Agent ID"])
            .add_row(vec![Cell::new("\u{1b}[32mColored(\"id\")\u{1b}[0m")]);

        assert_rows_aligned(&table.to_string());
    }

    fn assert_rows_aligned(table: &str) {
        let widths = table
            .lines()
            .map(|line| strip_ansi_escapes::strip_str(line).chars().count())
            .collect::<Vec<_>>();

        assert!(
            widths.iter().all(|width| *width == widths[0]),
            "misaligned rows, widths {widths:?}:\n{table}"
        );
    }

    fn typed_string_value(value: &str) -> TypedSchemaValue {
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String(value.to_string()),
        )
    }

    fn agent_initialization() -> PublicAgentInvocation {
        PublicAgentInvocation::AgentInitialization(AgentInitializationParameters {
            idempotency_key: IdempotencyKey::new("init-key".to_string()),
            constructor_parameters: typed_string_value("constructor"),
            trace_id: TraceId::generate(),
            trace_states: vec!["trace-state".to_string()],
            invocation_context: vec![],
        })
    }

    fn agent_method_invocation() -> PublicAgentInvocation {
        PublicAgentInvocation::AgentMethodInvocation(AgentMethodInvocationParameters {
            idempotency_key: IdempotencyKey::new("method-key".to_string()),
            method_name: "generated-method".to_string(),
            function_input: typed_string_value("input"),
            trace_id: TraceId::generate(),
            trace_states: vec!["trace-state".to_string()],
            invocation_context: vec![],
        })
    }

    fn save_snapshot() -> PublicAgentInvocation {
        PublicAgentInvocation::SaveSnapshot(Empty {})
    }

    fn load_snapshot() -> PublicAgentInvocation {
        PublicAgentInvocation::LoadSnapshot(LoadSnapshotParameters {
            snapshot: PublicSnapshotData::Raw(RawSnapshotData {
                data: vec![1, 2, 3],
                mime_type: "application/octet-stream".to_string(),
            }),
        })
    }

    fn process_oplog_entries() -> PublicAgentInvocation {
        PublicAgentInvocation::ProcessOplogEntries(ProcessOplogEntriesParameters {
            idempotency_key: IdempotencyKey::new("process-key".to_string()),
        })
    }

    fn manual_update() -> PublicAgentInvocation {
        PublicAgentInvocation::ManualUpdate(ManualUpdateParameters {
            target_revision: ComponentRevision::new(5).unwrap(),
        })
    }

    fn render_for_test(
        kind: AgentInvocationRenderKind,
        invocation: &PublicAgentInvocation,
    ) -> String {
        let rendered =
            render_agent_invocation(kind, "          ", &timestamp(), invocation).join("\n");
        strip_ansi_escapes::strip_str(rendered)
    }

    fn assert_contains_all(rendered: &str, expected: &[&str]) {
        for expected in expected {
            assert!(
                rendered.contains(expected),
                "expected rendered output to contain {expected:?}, got:\n{rendered}"
            );
        }
    }

    fn assert_debug_noise_absent(rendered: &str) {
        for forbidden in [
            "AgentInitializationParameters",
            "AgentMethodInvocationParameters",
            "LoadSnapshotParameters",
            "ProcessOplogEntriesParameters",
            "ManualUpdateParameters",
            "PublicLocalSpanData",
            "trace_states",
            "invocation_context",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "rendered output contains debug-only text {forbidden:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn started_public_agent_invocations_render_without_debug_dump() {
        let cases = [
            (
                agent_initialization(),
                vec![
                    "INVOKE initialize",
                    "idempotency key:   init-key",
                    "input:",
                    "constructor",
                ],
            ),
            (
                agent_method_invocation(),
                vec![
                    "INVOKE generated-method",
                    "idempotency key:   method-key",
                    "input:",
                    "input",
                ],
            ),
            (save_snapshot(), vec!["INVOKE save snapshot"]),
            (
                load_snapshot(),
                vec![
                    "INVOKE load snapshot",
                    "mime type:         application/octet-stream",
                    "data:              (3 bytes)",
                ],
            ),
            (
                process_oplog_entries(),
                vec![
                    "INVOKE process oplog entries",
                    "idempotency key:   process-key",
                ],
            ),
            (
                manual_update(),
                vec!["INVOKE manual update", "target revision:   5"],
            ),
        ];

        for (invocation, expected) in cases {
            let rendered = render_for_test(AgentInvocationRenderKind::Started, &invocation);
            assert_contains_all(&rendered, &expected);
            assert_debug_noise_absent(&rendered);
            assert!(
                !rendered.contains("entries:"),
                "process-oplog-entry count must not be rendered without a public count field:\n{rendered}"
            );
        }
    }

    #[test]
    fn pending_public_agent_invocations_render_without_debug_dump() {
        let cases = [
            (
                agent_initialization(),
                vec![
                    "ENQUEUED AGENT INITIALIZATION",
                    "idempotency key:   init-key",
                    "input:",
                    "constructor",
                ],
            ),
            (
                agent_method_invocation(),
                vec![
                    "ENQUEUED INVOCATION generated-method",
                    "idempotency key:   method-key",
                    "input:",
                    "input",
                ],
            ),
            (save_snapshot(), vec!["ENQUEUED SAVE SNAPSHOT"]),
            (
                load_snapshot(),
                vec![
                    "ENQUEUED LOAD SNAPSHOT",
                    "mime type:         application/octet-stream",
                    "data:              (3 bytes)",
                ],
            ),
            (
                process_oplog_entries(),
                vec![
                    "ENQUEUED PROCESS OPLOG ENTRIES",
                    "idempotency key:   process-key",
                ],
            ),
            (
                manual_update(),
                vec!["ENQUEUED MANUAL UPDATE", "target revision:   5"],
            ),
        ];

        for (invocation, expected) in cases {
            let rendered = render_for_test(AgentInvocationRenderKind::Pending, &invocation);
            assert_contains_all(&rendered, &expected);
            assert_debug_noise_absent(&rendered);
            assert!(
                !rendered.contains("entries:"),
                "process-oplog-entry count must not be rendered without a public count field:\n{rendered}"
            );
        }
    }
}
