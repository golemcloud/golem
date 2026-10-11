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

use crate::log::logln;
use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::*;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use golem_common::model::Timestamp;
use golem_common::model::oplog::{
    MultipartPartData, OplogCursor, PluginInstallationDescription, PublicAgentInvocation,
    PublicAgentInvocationResult, PublicAttributeValue, PublicEntityCallMode,
    PublicEntityInvocation, PublicEntityInvocationOperation, PublicOplogEntry,
    PublicOplogEntryAttribution, PublicSnapshotData, PublicUpdateDescription, StringAttributeValue,
};
use golem_common::schema::TypedSchemaValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use crate::agent_id_display::SourceLanguage;
#[cfg(test)]
use crate::model::agent::{
    AgentGetView, AgentMetadataView, AgentsMetadataResponseView, RawAgentId,
};
#[cfg(test)]
use golem_common::model::AgentStatus;
#[cfg(test)]
use golem_common::model::component::ComponentName;
#[cfg(test)]
use golem_common::model::oplog::OplogErrorKind;
#[cfg(test)]
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOplogEntryView {
    pub index: u64,
    pub attribution: PublicOplogEntryAttribution,
    pub entry: PublicOplogEntry,
}

#[derive(Clone, Debug)]
pub struct AgentOplogSearchPage {
    pub entries: Vec<AgentOplogEntryView>,
    pub next: Option<OplogCursor>,
    pub scanned_through_index: Option<u64>,
    pub last_index: u64,
    pub scan_limit_reached: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OplogEntryGroupKey {
    OplogIndex(u64),
    Span(String),
}

impl OplogEntryGroupKey {
    pub fn start_index(&self) -> Option<u64> {
        match self {
            Self::OplogIndex(index) => Some(*index),
            Self::Span(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OplogEntryPresentation {
    pub index: u64,
    pub timestamp: Timestamp,
    pub kind: String,
    pub group: Option<OplogEntryGroupKey>,
    pub detail: Value,
    pub attribution: Option<PublicOplogEntryAttribution>,
    pub entry: Option<PublicOplogEntry>,
    pub search_text: OnceLock<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogEntryCategory {
    Invocation,
    HostCall,
    Log,
    Failure,
    Trace,
    State,
    Lifecycle,
    Other,
}

impl OplogEntryCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Invocation => "invoke",
            Self::HostCall => "host",
            Self::Log => "log",
            Self::Failure => "failure",
            Self::Trace => "trace",
            Self::State => "state",
            Self::Lifecycle => "lifecycle",
            Self::Other => "other",
        }
    }
}

impl OplogEntryPresentation {
    pub fn structural_parent_group(&self) -> Option<OplogEntryGroupKey> {
        if let Some(PublicOplogEntry::Start(params)) = self.entry.as_ref()
            && let Some(parent) = params.parent_start_index
        {
            return Some(OplogEntryGroupKey::OplogIndex(parent.as_u64()));
        }
        if let Some(PublicOplogEntryAttribution::Entity(context)) = self.attribution.as_ref() {
            return Some(OplogEntryGroupKey::OplogIndex(
                context.invocation.start_index.as_u64(),
            ));
        }
        None
    }

    pub fn category(&self) -> OplogEntryCategory {
        use PublicOplogEntry as Entry;

        match self.entry.as_ref() {
            Some(
                Entry::AgentInvocationStarted(_)
                | Entry::AgentInvocationFinished(_)
                | Entry::PendingAgentInvocation(_)
                | Entry::CancelPendingInvocation(_),
            ) => OplogEntryCategory::Invocation,
            Some(
                Entry::Start(_)
                | Entry::End(_)
                | Entry::Cancelled(_)
                | Entry::CompletionDiscarded(_)
                | Entry::CompletionDelivered(_)
                | Entry::HostStreamFrame(_),
            ) => OplogEntryCategory::HostCall,
            Some(Entry::Log(_)) => OplogEntryCategory::Log,
            Some(Entry::Error(_) | Entry::FailedUpdate(_)) => OplogEntryCategory::Failure,
            Some(Entry::StartSpan(_) | Entry::FinishSpan(_) | Entry::SetSpanAttribute(_)) => {
                OplogEntryCategory::Trace
            }
            Some(
                Entry::BeginAtomicRegion(_)
                | Entry::EndAtomicRegion(_)
                | Entry::BeginRemoteTransaction(_)
                | Entry::PreCommitRemoteTransaction(_)
                | Entry::PreRollbackRemoteTransaction(_)
                | Entry::CommittedRemoteTransaction(_)
                | Entry::RolledBackRemoteTransaction(_)
                | Entry::Snapshot(_)
                | Entry::CreateResource(_)
                | Entry::DropResource(_)
                | Entry::StreamRegistered(_)
                | Entry::StreamItems(_)
                | Entry::StreamEnd(_)
                | Entry::StreamCancel(_)
                | Entry::StreamSession(_),
            ) => OplogEntryCategory::State,
            Some(
                Entry::Create(_)
                | Entry::Suspend(_)
                | Entry::RecoverySucceeded(_)
                | Entry::Jump(_)
                | Entry::Interrupted(_)
                | Entry::Exited(_)
                | Entry::PendingUpdate(_)
                | Entry::SuccessfulUpdate(_)
                | Entry::Restart(_)
                | Entry::Resumed(_)
                | Entry::Revert(_),
            ) => OplogEntryCategory::Lifecycle,
            Some(_) => OplogEntryCategory::Other,
            None => synthetic_category(&self.kind),
        }
    }

    pub fn summary(&self) -> String {
        use PublicOplogEntry as Entry;

        match self.entry.as_ref() {
            Some(Entry::Create(params)) => format!("created {}", params.agent_id),
            Some(Entry::Start(params)) => params.function_name.clone(),
            Some(Entry::End(params)) => format!("completed host call #{}", params.start_index),
            Some(Entry::Cancelled(params)) => {
                format!("cancelled host call #{}", params.start_index)
            }
            Some(Entry::AgentInvocationStarted(params)) => invocation_summary(&params.invocation),
            Some(Entry::AgentInvocationFinished(params)) => params
                .method_name
                .as_ref()
                .map(|method| format!("completed {method}"))
                .unwrap_or_else(|| "invocation completed".to_string()),
            Some(Entry::PendingAgentInvocation(params)) => {
                format!("queued {}", invocation_summary(&params.invocation))
            }
            Some(Entry::Error(params)) => params.error.clone(),
            Some(Entry::Log(params)) => {
                if params.context.is_empty() {
                    params.message.clone()
                } else {
                    format!("{} · {}", params.context, params.message)
                }
            }
            Some(Entry::PendingUpdate(params)) => {
                format!("revision {} queued", params.target_revision)
            }
            Some(Entry::SuccessfulUpdate(params)) => {
                format!("revision {} active", params.target_revision)
            }
            Some(Entry::FailedUpdate(params)) => params
                .details
                .clone()
                .unwrap_or_else(|| format!("revision {} failed", params.target_revision)),
            Some(Entry::StartSpan(params)) => format!("span {} started", params.span_id),
            Some(Entry::FinishSpan(params)) => format!("span {} finished", params.span_id),
            Some(Entry::SetSpanAttribute(params)) => {
                format!("span {} · {}", params.span_id, params.key)
            }
            Some(Entry::HostStreamFrame(params)) => {
                format!("{:?} frame for #{}", params.kind, params.parent_start_index)
            }
            Some(Entry::CompletionDelivered(params)) => {
                format!("completion delivered for #{}", params.start_index)
            }
            Some(Entry::CompletionDiscarded(params)) => {
                format!("completion discarded for #{}", params.start_index)
            }
            Some(Entry::BeginRemoteTransaction(params)) => {
                format!("transaction {}", params.transaction_id)
            }
            Some(Entry::PreCommitRemoteTransaction(params)) => {
                format!("prepare commit for #{}", params.begin_index)
            }
            Some(Entry::PreRollbackRemoteTransaction(params)) => {
                format!("prepare rollback for #{}", params.begin_index)
            }
            Some(Entry::CommittedRemoteTransaction(params)) => {
                format!("committed transaction #{}", params.begin_index)
            }
            Some(Entry::RolledBackRemoteTransaction(params)) => {
                format!("rolled back transaction #{}", params.begin_index)
            }
            Some(Entry::Jump(params)) => {
                format!("jump #{}..#{}", params.jump.start, params.jump.end)
            }
            Some(Entry::Revert(params)) => format!(
                "reverted through #{}",
                params.dropped_region.start.previous()
            ),
            Some(_) | None => summary_from_json(&self.detail).unwrap_or_default(),
        }
    }

    pub fn tags(&self) -> Vec<String> {
        use PublicOplogEntry as Entry;

        let mut tags = Vec::new();
        match self.entry.as_ref() {
            Some(Entry::Start(params)) => {
                tags.push(format!("{:?}", params.durable_function_type));
                if let Some(parent) = params.parent_start_index {
                    tags.push(format!("parent #{parent}"));
                }
            }
            Some(Entry::AgentInvocationFinished(params)) => {
                tags.push(format!("fuel {}", params.consumed_fuel));
                tags.push(format!("revision {}", params.component_revision));
            }
            Some(Entry::Log(params)) => tags.push(format!("{:?}", params.level)),
            Some(Entry::Error(params)) => tags.push(format!("{:?}", params.kind)),
            Some(Entry::End(params)) if params.forced_commit => {
                tags.push("forced commit".to_string())
            }
            _ => {}
        }
        if matches!(
            self.attribution,
            Some(PublicOplogEntryAttribution::Entity(_))
        ) {
            tags.push("entity".to_string());
        }
        tags
    }

    pub fn matches_loaded_filter(&self, filter: &str) -> bool {
        let filter = filter.trim().to_lowercase();
        if filter.is_empty() {
            return true;
        }
        self.search_text
            .get_or_init(|| {
                let detail = serde_json::to_string(&self.detail).unwrap_or_default();
                let attribution = self
                    .attribution
                    .as_ref()
                    .and_then(|value| serde_json::to_string(value).ok())
                    .unwrap_or_default();
                format!(
                    "{} {} {} {} {} {}",
                    self.index,
                    self.kind,
                    self.category().label(),
                    self.summary(),
                    detail,
                    attribution
                )
                .to_lowercase()
            })
            .contains(&filter)
    }
}

impl From<AgentOplogEntryView> for OplogEntryPresentation {
    fn from(value: AgentOplogEntryView) -> Self {
        let group = match &value.entry {
            PublicOplogEntry::Start(_)
            | PublicOplogEntry::BeginAtomicRegion(_)
            | PublicOplogEntry::BeginRemoteTransaction(_) => {
                Some(OplogEntryGroupKey::OplogIndex(value.index))
            }
            PublicOplogEntry::End(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.start_index.as_u64()))
            }
            PublicOplogEntry::Cancelled(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.start_index.as_u64()))
            }
            PublicOplogEntry::CompletionDiscarded(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.start_index.as_u64()))
            }
            PublicOplogEntry::CompletionDelivered(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.start_index.as_u64()))
            }
            PublicOplogEntry::HostStreamFrame(params) => Some(OplogEntryGroupKey::OplogIndex(
                params.parent_start_index.as_u64(),
            )),
            PublicOplogEntry::EndAtomicRegion(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.begin_index.as_u64()))
            }
            PublicOplogEntry::PreCommitRemoteTransaction(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.begin_index.as_u64()))
            }
            PublicOplogEntry::PreRollbackRemoteTransaction(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.begin_index.as_u64()))
            }
            PublicOplogEntry::CommittedRemoteTransaction(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.begin_index.as_u64()))
            }
            PublicOplogEntry::RolledBackRemoteTransaction(params) => {
                Some(OplogEntryGroupKey::OplogIndex(params.begin_index.as_u64()))
            }
            PublicOplogEntry::StartSpan(params) => {
                Some(OplogEntryGroupKey::Span(params.span_id.to_string()))
            }
            PublicOplogEntry::FinishSpan(params) => {
                Some(OplogEntryGroupKey::Span(params.span_id.to_string()))
            }
            PublicOplogEntry::SetSpanAttribute(params) => {
                Some(OplogEntryGroupKey::Span(params.span_id.to_string()))
            }
            _ => None,
        };
        let timestamp = value.entry.timestamp();
        let kind = value.entry.kind().to_string();
        let detail = serde_json::to_value(&value.entry).unwrap_or(Value::Null);
        Self {
            index: value.index,
            timestamp,
            kind,
            group,
            detail,
            attribution: Some(value.attribution),
            entry: Some(value.entry),
            search_text: OnceLock::new(),
        }
    }
}

fn invocation_summary(invocation: &PublicAgentInvocation) -> String {
    match invocation {
        PublicAgentInvocation::AgentInitialization(_) => "initialize".to_string(),
        PublicAgentInvocation::AgentMethodInvocation(params) => params.method_name.clone(),
        PublicAgentInvocation::SaveSnapshot(_) => "save snapshot".to_string(),
        PublicAgentInvocation::LoadSnapshot(_) => "load snapshot".to_string(),
        PublicAgentInvocation::ProcessOplogEntries(_) => "process oplog entries".to_string(),
        PublicAgentInvocation::ManualUpdate(params) => {
            format!("update to revision {}", params.target_revision)
        }
    }
}

fn synthetic_category(kind: &str) -> OplogEntryCategory {
    match kind {
        "AgentInvocationStarted" | "AgentInvocationFinished" | "PendingAgentInvocation" => {
            OplogEntryCategory::Invocation
        }
        "Start" | "End" | "Cancelled" | "HostStreamFrame" => OplogEntryCategory::HostCall,
        "Log" => OplogEntryCategory::Log,
        "Error" | "FailedUpdate" => OplogEntryCategory::Failure,
        "StartSpan" | "FinishSpan" | "SetSpanAttribute" => OplogEntryCategory::Trace,
        "Create" | "Restart" | "Revert" | "Jump" => OplogEntryCategory::Lifecycle,
        _ => OplogEntryCategory::Other,
    }
}

fn summary_from_json(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "functionName",
        "methodName",
        "message",
        "error",
        "context",
        "name",
    ] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            return Some(value.to_string());
        }
    }
    None
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
        for line in render_oplog_attribution_lines(&self.attribution) {
            logln(line);
        }
        self.entry.log()
    }
}

fn render_oplog_attribution_lines(attribution: &PublicOplogEntryAttribution) -> Vec<String> {
    let pad = "          ";
    match attribution {
        PublicOplogEntryAttribution::Agent(_) => vec![format!("{pad}owner:             agent")],
        PublicOplogEntryAttribution::Entity(context) => {
            let mut lines = vec![
                format!("{pad}owner:             entity"),
                format!("{pad}entity chain:"),
            ];
            for ancestor in &context.ancestors {
                lines.push(format_entity_invocation(pad, ancestor, false));
            }
            lines.push(format_entity_invocation(pad, &context.invocation, true));
            if let Some(PublicEntityInvocationOperation::Tool(tool)) = &context.invocation.operation
            {
                lines.push(format!(
                    "{pad}tool command:      {}",
                    tool.command_path.join(" ")
                ));
                lines.push(format!("{pad}stdin requested:   {}", tool.has_stdin));
                lines.push(format!("{pad}stdout requested:  {}", tool.has_stdout));
                lines.push(format!("{pad}stdout declared:   {}", tool.declares_stdout));
                lines.push(format!(
                    "{pad}stdout recording:  none (live attachment only)"
                ));
            }
            lines
        }
    }
}

fn format_entity_invocation(
    pad: &str,
    invocation: &PublicEntityInvocation,
    current: bool,
) -> String {
    let kind = match invocation.entity.kind {
        golem_common::model::oplog::PublicAgentEntityKind::Tool => "tool",
        golem_common::model::oplog::PublicAgentEntityKind::ToolMiddleware => "tool middleware",
    };
    let call_mode = match invocation.call_mode {
        PublicEntityCallMode::Synchronous => "synchronous",
        PublicEntityCallMode::Asynchronous => "asynchronous",
        PublicEntityCallMode::FireAndForget => "fire and forget",
    };
    let current = if current { " [current]" } else { "" };
    format!(
        "{pad}  - {kind} {} (start #{}, {call_mode}){current}",
        invocation.entity.name, invocation.start_index
    )
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
                if let Some(owner) = params.observational_owner {
                    logln(format!("{pad}observational owner: {}", owner));
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
                if let Some(wallet_pin) = &params.wallet_pin {
                    let wallet_id_hash = wallet_pin
                        .wallet_token
                        .wallet_id_hash
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>();
                    logln(format!("{pad}wallet id hash:    {wallet_id_hash}"));
                    logln(format!(
                        "{pad}wallet generation: {}",
                        wallet_pin.wallet_token.generation
                    ));
                    if let Some(scope_card_id) = wallet_pin.scope_card_id {
                        logln(format!("{pad}scope card:        {scope_card_id}"));
                    }
                }
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
                        log_typed_schema_value(pad, &output.output);
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
                logln(format!("{pad}kind:              {:?}", params.kind));
                logln(format!("{pad}retry from:        {}", params.retry_from));
                logln(format!(
                    "{pad}error:             {}",
                    format_error(&params.error)
                ));
            }
            PublicOplogEntry::RecoverySucceeded(params) => {
                logln(format_message_highlight("RECOVERY SUCCEEDED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
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
            PublicOplogEntry::Resumed(params) => {
                logln(format_message_highlight("RESUMED"));
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
                if let Some(generation) = params.wallet_generation {
                    logln(format!("{pad}wallet generation: {generation}"));
                }
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
            PublicOplogEntry::StreamRegistered(params) => {
                logln(format_message_highlight("STREAM REGISTERED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}record:            {}",
                    typed_schema_value_to_string(&params.record)
                ));
            }
            PublicOplogEntry::StreamItems(params) => {
                logln(format_message_highlight("STREAM ITEMS"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}record:            {}",
                    typed_schema_value_to_string(&params.record)
                ));
            }
            PublicOplogEntry::StreamEnd(params) => {
                logln(format_message_highlight("STREAM END"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}record:            {}",
                    typed_schema_value_to_string(&params.record)
                ));
            }
            PublicOplogEntry::StreamCancel(params) => {
                logln(format_message_highlight("STREAM CANCEL"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}record:            {}",
                    typed_schema_value_to_string(&params.record)
                ));
            }
            PublicOplogEntry::StreamSession(params) => {
                logln(format_message_highlight("STREAM SESSION"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}record:            {}",
                    typed_schema_value_to_string(&params.record)
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
                if let Some(generation) = params.wallet_generation {
                    logln(format!("{pad}wallet generation: {generation}"));
                }
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
                if let Some(generation) = params.wallet_generation {
                    logln(format!("{pad}wallet generation: {generation}"));
                }
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
            PublicOplogEntry::CardDerived(params) => {
                logln(format_message_highlight("CARD DERIVED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.card_id)
                ));
                logln(format!(
                    "{pad}parent ids:        {}",
                    format_id(&format!("{:?}", params.parent_ids))
                ));
                if let Some(generation) = params.wallet_generation {
                    logln(format!("{pad}wallet generation: {generation}"));
                }
            }
            PublicOplogEntry::CardTransferStarted(params) => {
                logln(format_message_highlight("CARD TRANSFER STARTED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}transfer id:       {}",
                    format_id(&params.transfer_id)
                ));
                logln(format!(
                    "{pad}card id:           {}",
                    format_id(&params.card_id)
                ));
                logln(format!(
                    "{pad}target holder:     {}",
                    format_id(&format!("{:?}", params.target_holder))
                ));
                if let Some(generation) = params.source_wallet_generation {
                    logln(format!("{pad}source generation: {generation}"));
                }
            }
            PublicOplogEntry::CardTransferred(params) => {
                logln(format_message_highlight("CARD TRANSFER ADMITTED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}transfer id:       {}",
                    format_id(&params.transfer_id)
                ));
                if let Some(source_card_id) = params.source_card_id {
                    logln(format!(
                        "{pad}source card id:    {}",
                        format_id(&source_card_id)
                    ));
                }
                logln(format!(
                    "{pad}installed card id: {}",
                    format_id(&params.installed_card_id)
                ));
                logln(format!(
                    "{pad}target holder:     {}",
                    format_id(&format!("{:?}", params.target_holder))
                ));
                if let Some(generation) = params.target_wallet_generation {
                    logln(format!("{pad}target generation: {generation}"));
                }
            }
            PublicOplogEntry::CardRevokedCascade(params) => {
                logln(format_message_highlight("CARD REVOKED CASCADE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}revoked card ids:  {}",
                    format_id(&format!("{:?}", params.revoked_card_ids))
                ));
                if let Some(generation) = params.local_wallet_generation {
                    logln(format!("{pad}local generation:  {generation}"));
                }
            }
            PublicOplogEntry::CardTransferConfirmed(params) => {
                logln(format_message_highlight("CARD TRANSFER CONFIRMED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}transfer id:       {}",
                    format_id(&params.transfer_id)
                ));
                logln(format!(
                    "{pad}source card id:    {}",
                    format_id(&params.source_card_id)
                ));
                logln(format!(
                    "{pad}installed card id: {}",
                    format_id(&params.installed_card_id)
                ));
                logln(format!(
                    "{pad}target holder:     {}",
                    format_id(&format!("{:?}", params.target_holder))
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
            ));
        }
        PublicAgentInvocation::AgentMethodInvocation(params) => {
            lines.push(format!(
                "{pad}idempotency key:   {}",
                format_id(&params.idempotency_key)
            ));
            lines.push(format!("{pad}input:"));
            lines.push(render_typed_schema_value_line(pad, &params.function_input));
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
    golem_common::schema::render::value_to_cli_text_with_secret_metadata(
        value.graph(),
        value.root_type(),
        value.value(),
    )
    .unwrap_or_else(|err| format!("<rendering error: {err}>"))
}

fn log_typed_schema_value(pad: &str, value: &TypedSchemaValue) {
    logln(render_typed_schema_value_line(pad, value));
}

fn render_typed_schema_value_line(pad: &str, value: &TypedSchemaValue) -> String {
    let rendered = typed_schema_value_to_string(value);
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
    use golem_common::model::oplog::payload::types::{SecretRevealAudit, SerializableDateTime};
    use golem_common::model::oplog::public_oplog_entry::{
        BeginAtomicRegionParams, EndAtomicRegionParams, EndParams, StartParams,
    };
    use golem_common::model::oplog::{
        AgentInitializationParameters, AgentMethodInvocationParameters, HostRequestSecretReveal,
        HostResponseSecretRevealed, LoadSnapshotParameters, ManualUpdateParameters,
        ProcessOplogEntriesParameters, PublicAgentEntity, PublicAgentEntityKind,
        PublicDurableFunctionType, PublicEntityInvocationContext, PublicToolInvocationOperation,
        RawSnapshotData,
    };
    use golem_common::model::{Empty, IdempotencyKey};
    use golem_common::schema::{
        IntoTypedSchemaValue, NamedFieldType, PermissionCardSpec, PermissionCardValuePayload,
        QuotaTokenSpec, QuotaTokenValuePayload, SchemaGraph, SchemaType, SchemaValue, SecretSpec,
        SecretValuePayload,
    };
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
            last_error_kind: None,
            component_size: 0,
            total_linear_memory_size: 0,
            exported_resource_instances: HashMap::new(),
            source_language: SourceLanguage::Rust,
            secret_config_paths: std::collections::BTreeSet::new(),
        }
    }

    #[test]
    fn oplog_presentation_uses_typed_kind_timestamp_and_relationships() {
        let start = OplogEntryPresentation::from(AgentOplogEntryView {
            index: 17,
            attribution: PublicOplogEntryAttribution::agent(),
            entry: PublicOplogEntry::Start(StartParams {
                timestamp: Timestamp::from(1000),
                parent_start_index: None,
                function_name: "golem:test/run".to_string(),
                invocation_id: None,
                observational_owner: None,
                request: None,
                durable_function_type: PublicDurableFunctionType::WriteLocal(Empty {}),
            }),
        });
        let end = OplogEntryPresentation::from(AgentOplogEntryView {
            index: 19,
            attribution: PublicOplogEntryAttribution::agent(),
            entry: PublicOplogEntry::End(EndParams {
                timestamp: Timestamp::from(2000),
                start_index: golem_common::model::OplogIndex::from_u64(17),
                response: None,
                forced_commit: false,
            }),
        });
        let atomic_start = OplogEntryPresentation::from(AgentOplogEntryView {
            index: 23,
            attribution: PublicOplogEntryAttribution::agent(),
            entry: PublicOplogEntry::BeginAtomicRegion(BeginAtomicRegionParams {
                timestamp: Timestamp::from(3000),
            }),
        });
        let atomic_end = OplogEntryPresentation::from(AgentOplogEntryView {
            index: 29,
            attribution: PublicOplogEntryAttribution::agent(),
            entry: PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
                timestamp: Timestamp::from(4000),
                begin_index: golem_common::model::OplogIndex::from_u64(23),
            }),
        });

        assert_eq!(start.kind, "Start");
        assert_eq!(start.timestamp, Timestamp::from(1000));
        assert_eq!(start.group, Some(OplogEntryGroupKey::OplogIndex(17)));
        assert_eq!(start.category(), OplogEntryCategory::HostCall);
        assert_eq!(start.summary(), "golem:test/run");
        assert!(matches!(start.entry, Some(PublicOplogEntry::Start(_))));
        assert!(matches!(
            start.attribution,
            Some(PublicOplogEntryAttribution::Agent(_))
        ));
        assert_eq!(end.kind, "End");
        assert_eq!(end.timestamp, Timestamp::from(2000));
        assert_eq!(end.group, Some(OplogEntryGroupKey::OplogIndex(17)));
        assert_eq!(atomic_start.group, Some(OplogEntryGroupKey::OplogIndex(23)));
        assert_eq!(atomic_end.group, Some(OplogEntryGroupKey::OplogIndex(23)));
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

    #[test]
    fn recovery_failure_is_visible_in_agent_get_and_list() {
        let mut agent = agent_metadata(r#"Counter("unavailable")"#);
        agent.status = AgentStatus::Failed;
        agent.last_error_kind = Some(OplogErrorKind::Recovery);
        agent.last_error = Some("snapshot payload is unavailable".to_string());

        let table = AgentsMetadataResponseView::format_table_wide(
            std::slice::from_ref(&agent),
            120,
            false,
            false,
        );
        assert!(table.contains("Unavailable"), "{table}");

        let fields = AgentGetView::from_metadata(agent, true).fields();
        assert!(fields.contains(&("Failure category".to_string(), "Recovery".to_string())));
        assert!(fields.iter().any(|(name, value)| {
            name == "Last error" && value.contains("snapshot payload is unavailable")
        }));
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
    fn oplog_attribution_renders_agent_and_nested_entity_ownership() {
        let agent =
            render_oplog_attribution_lines(&PublicOplogEntryAttribution::agent()).join("\n");
        assert_eq!(agent, "          owner:             agent");

        let ancestor = PublicEntityInvocation {
            entity: PublicAgentEntity {
                kind: PublicAgentEntityKind::ToolMiddleware,
                name: "authorize".to_string(),
            },
            start_index: golem_common::model::OplogIndex::from_u64(7),
            call_mode: PublicEntityCallMode::Synchronous,
            operation: None,
        };
        let invocation = PublicEntityInvocation {
            entity: PublicAgentEntity {
                kind: PublicAgentEntityKind::Tool,
                name: "filesystem".to_string(),
            },
            start_index: golem_common::model::OplogIndex::from_u64(11),
            call_mode: PublicEntityCallMode::Asynchronous,
            operation: Some(PublicEntityInvocationOperation::Tool(
                PublicToolInvocationOperation {
                    command_path: vec!["files".to_string(), "lookup".to_string()],
                    has_stdin: true,
                    has_stdout: false,
                    declares_stdout: true,
                },
            )),
        };

        let rendered = render_oplog_attribution_lines(&PublicOplogEntryAttribution::entity(
            PublicEntityInvocationContext {
                invocation,
                ancestors: vec![ancestor],
            },
        ))
        .join("\n");

        assert_contains_all(
            &rendered,
            &[
                "owner:             entity",
                "entity chain:",
                "tool middleware authorize (start #7, synchronous)",
                "tool filesystem (start #11, asynchronous) [current]",
                "tool command:      files lookup",
                "stdin requested:   true",
                "stdout requested:  false",
                "stdout declared:   true",
                "stdout recording:  none (live attachment only)",
            ],
        );
        assert!(
            rendered.find("authorize").unwrap() < rendered.find("filesystem").unwrap(),
            "ancestor must render before the current invocation:\n{rendered}"
        );
    }

    #[test]
    fn oplog_typed_values_show_safe_secret_metadata() {
        let secret_id = uuid::Uuid::from_u128(1);
        let permission_card_id = uuid::Uuid::from_u128(2);
        let schema = SchemaType::record(vec![
            NamedFieldType {
                name: "credential".to_string(),
                body: SchemaType::secret(SecretSpec::default()),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "quota".to_string(),
                body: SchemaType::quota_token(QuotaTokenSpec::default()),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "permission".to_string(),
                body: SchemaType::permission_card(PermissionCardSpec { polymorphic: true }),
                metadata: Default::default(),
            },
        ]);
        let value = TypedSchemaValue::new(
            SchemaGraph::anonymous(schema),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::Secret(SecretValuePayload {
                        secret_id,
                        config_key: Some(vec!["database".to_string(), "password".to_string()]),
                        version: 7,
                        resolved_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                        category: Some("api-key".to_string()),
                    }),
                    SchemaValue::QuotaToken(QuotaTokenValuePayload {
                        environment_id: uuid::Uuid::nil().into(),
                        resource_name: "private-quota-resource".to_string(),
                        expected_use: 1,
                        last_credit: 0,
                        last_credit_at: chrono::DateTime::from_timestamp(1_700_000_001, 0).unwrap(),
                    }),
                    SchemaValue::PermissionCard(PermissionCardValuePayload {
                        card_id: permission_card_id,
                        parent_ids: vec![],
                        expires_at: None,
                        polymorphic: true,
                    }),
                ],
            },
        );

        let mut invocation = agent_method_invocation();
        let PublicAgentInvocation::AgentMethodInvocation(parameters) = &mut invocation else {
            unreachable!()
        };
        parameters.function_input = value.clone();

        for rendered in [
            typed_schema_value_to_string(&value),
            render_for_test(AgentInvocationRenderKind::Started, &invocation),
        ] {
            assert_contains_all(
                &rendered,
                &[
                    "<secret metadata:",
                    &secret_id.to_string(),
                    "database",
                    "password",
                    "\"version\":7",
                    "\"resolvedAt\":\"2023-11-14T22:13:20+00:00\"",
                    "\"category\":\"api-key\"",
                    "<redacted: quota-token>",
                    "<redacted: permission-card>",
                ],
            );
            assert!(!rendered.contains("<redacted: secret>"));
            assert!(!rendered.contains("secretValue"));
            assert!(!rendered.contains("private-quota-resource"));
            assert!(!rendered.contains(&permission_card_id.to_string()));
        }
    }

    #[test]
    fn oplog_secret_reveal_payloads_show_audit_metadata_without_secret_values() {
        let secret_id = uuid::Uuid::from_u128(3);
        let request = HostRequestSecretReveal {
            secret_id,
            expected_type: SchemaGraph::anonymous(SchemaType::string()),
        }
        .into_typed_schema_value()
        .expect("secret reveal request must be schema-encodable");
        let response = HostResponseSecretRevealed {
            secret_id,
            pinned_revision: 11,
            resolved_at: SerializableDateTime {
                seconds: 1_700_000_004,
                nanoseconds: 0,
            },
            result: Ok(()),
            audit: SecretRevealAudit {
                calling_agent: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                    agent_id: "secret-reveal-auditor".to_string(),
                },
                config_key: Some(vec!["database".to_string(), "password".to_string()]),
                timestamp: SerializableDateTime {
                    seconds: 1_700_000_005,
                    nanoseconds: 0,
                },
            },
        }
        .into_typed_schema_value()
        .expect("secret reveal response must be schema-encodable");

        let rendered = [
            typed_schema_value_to_string(&request),
            typed_schema_value_to_string(&response),
        ]
        .join("\n");
        assert_contains_all(
            &rendered,
            &[
                "low-bits: 3",
                "secret-reveal-auditor",
                "database",
                "password",
                "1700000004",
                "1700000005",
                "11",
            ],
        );
        assert!(!rendered.contains("secretValue"));
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
