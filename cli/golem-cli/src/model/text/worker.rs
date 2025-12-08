// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::log::{logln, LogColorize};
use crate::model::deploy::TryUpdateAllWorkersResult;
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::fmt::*;
use crate::model::worker::{
    WorkerMetadata, WorkerMetadataView, WorkerName, WorkersMetadataResponseView,
};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use chrono::DateTime;
use cli_table::{format::Justify, Table};
use colored::Colorize;
use golem_common::model::agent::{BinaryReference, DataValue, ElementValue, TextReference};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::oplog::{
    PluginInstallationDescription, PublicAttributeValue, PublicOplogEntry, PublicUpdateDescription,
    PublicWorkerInvocation, StringAttributeValue,
};
use golem_common::model::worker::UpdateRecord;
use golem_common::model::Timestamp;
use golem_wasm::{print_value_and_type, ValueAndType};
use indoc::indoc;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerCreateView {
    pub component_name: ComponentName,
    pub worker_name: Option<WorkerName>,
}

impl MessageWithFields for WorkerCreateView {
    fn message(&self) -> String {
        if let Some(worker_name) = &self.worker_name {
            format!(
                "Created new agent {}",
                format_message_highlight(&worker_name)
            )
        } else {
            // TODO: review: do we really want to hide the worker name? it is provided now
            //       in "worker new"
            format!(
                "Created new agent with a {}",
                format_message_highlight("random generated name")
            )
        }
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Component name", &self.component_name, format_id)
            .fmt_field_option("Agent name", &self.worker_name, format_worker_name);

        fields.build()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerGetView {
    pub metadata: WorkerMetadataView,
    pub precise: bool,
}

impl WorkerGetView {
    pub fn from_metadata(metadata: WorkerMetadata, precise: bool) -> Self {
        Self {
            metadata: WorkerMetadataView::from(metadata),
            precise,
        }
    }

    pub fn from_metadata_view(metadata: WorkerMetadataView) -> Self {
        Self {
            metadata,
            precise: false,
        }
    }
}

impl MessageWithFields for WorkerGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for agent {}",
            format_message_highlight(&self.metadata.worker_name)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        let mut update_history = String::new();
        for update in &self.metadata.updates {
            match update {
                UpdateRecord::PendingUpdate(update) => {
                    let _ = writeln!(
                        update_history,
                        "{}",
                        format!(
                            "{}: Pending update to {}",
                            update.timestamp, update.target_version
                        )
                        .bright_black()
                    );
                }
                UpdateRecord::SuccessfulUpdate(update) => {
                    let _ = writeln!(
                        update_history,
                        "{}",
                        format!(
                            "{}: Successful update to {}",
                            update.timestamp, update.target_version
                        )
                        .green()
                        .bold()
                    );
                }
                UpdateRecord::FailedUpdate(update) => {
                    let _ = writeln!(
                        update_history,
                        "{}",
                        format!(
                            "{}: Failed update to {}{}",
                            update.timestamp,
                            update.target_version,
                            update
                                .details
                                .as_ref()
                                .map(|details| format!(": {details}"))
                                .unwrap_or_default()
                        )
                        .yellow()
                    );
                }
            }
        }

        fields
            .fmt_field("Component name", &self.metadata.component_name, format_id)
            .fmt_field(
                "Component revision",
                &self.metadata.component_revision,
                format_id,
            )
            .fmt_field("Agent name", &self.metadata.worker_name, format_worker_name)
            .field("Created at", &self.metadata.created_at)
            .fmt_field(
                "Component size",
                &self.metadata.component_size,
                format_binary_size,
            )
            .fmt_field(
                "Total linear memory size",
                &self.metadata.total_linear_memory_size,
                format_binary_size,
            )
            .fmt_field_optional(
                "Environment variables",
                &self.metadata.env,
                !self.metadata.env.is_empty(),
                |env| {
                    env.iter()
                        .map(|(k, v)| format!("{}={}", k, v.bold()))
                        .join(";")
                },
            )
            .fmt_field_optional("Status", &self.metadata.status, self.precise, format_status)
            .fmt_field_optional(
                "Retry count",
                &self.metadata.retry_count,
                self.precise,
                format_retry_count,
            )
            .fmt_field_optional(
                "Pending invocation count",
                &self.metadata.pending_invocation_count,
                self.metadata.pending_invocation_count > 0,
                |n| n.to_string(),
            )
            .fmt_field_optional(
                "Last error",
                &self.metadata.last_error,
                self.metadata.last_error.is_some() && self.precise,
                |err| format_stack(err.as_ref().unwrap()),
            )
            .fmt_field_optional(
                "WARNING",
                "The presented agent metadata may not be up-to-date",
                !self.precise,
                format_warn,
            );

        fields.build()
    }
}

#[derive(Table)]
struct WorkerMetadataTableView {
    #[table(title = "Component name")]
    pub component_name: ComponentName,
    #[table(title = "Agent name")]
    pub worker_name: String,
    #[table(title = "Component\nrevision", justify = "Justify::Right")]
    pub component_revision: ComponentRevision,
    #[table(title = "Status", justify = "Justify::Right")]
    pub status: String,
    #[table(title = "Created at")]
    pub created_at: Timestamp,
}

impl From<&WorkerMetadataView> for WorkerMetadataTableView {
    fn from(value: &WorkerMetadataView) -> Self {
        Self {
            component_name: value.component_name.clone(),
            // TODO: pretty print, once we have "metadata-less" agent-type parsing
            worker_name: textwrap::wrap(&value.worker_name.0, 30).join("\n"),
            status: format_status(&value.status),
            component_revision: value.component_revision,
            created_at: value.created_at,
        }
    }
}

impl TextView for WorkersMetadataResponseView {
    fn log(&self) {
        log_table::<_, WorkerMetadataTableView>(&self.workers);

        if !self.cursors.is_empty() {
            logln("");
        }
        for (component_name, cursor) in &self.cursors {
            logln(format!(
                "Cursor for more results for component {}: {}",
                component_name.log_color_highlight(),
                cursor.log_color_highlight()
            ));
        }
    }
}

impl TextView for TryUpdateAllWorkersResult {
    fn log(&self) {
        // NOP
    }
}

impl TextView for InvokeResultView {
    fn log(&self) {
        fn log_results_format(format: &str) {
            logln(format!(
                "Invocation results in {} format:",
                format_message_highlight(format),
            ))
        }

        if self.result_wave.is_none() && self.result_json.is_none() {
            return;
        }

        if let Some(wave_values) = &self.result_wave {
            if wave_values.is_empty() {
                logln("Empty result.")
            } else {
                log_results_format("WAVE");
                for wave in wave_values {
                    logln(format!("  - {wave}"));
                }
            }
        } else if let Some(json) = &self.result_json {
            logln(format_warn(indoc!(
                "
                Failed to convert invocation result to WAVE format.
                At the moment WAVE does not support Handle (aka Resource) data type.
                "
            )));
            log_results_format("JSON");
            logln(serde_json::to_string_pretty(json).unwrap());
        }
    }
}

impl TextView for Vec<(u64, PublicOplogEntry)> {
    fn log(&self) {
        for (idx, entry) in self {
            logln(format!("{}: ", format_main_id(&format!("#{idx:0>5}"))));
            entry.log()
        }
    }
}

impl TextView for PublicOplogEntry {
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
            PublicOplogEntry::ImportedFunctionInvoked(params) => {
                logln(format!(
                    "{} {}",
                    format_message_highlight("CALL"),
                    format_id(&params.function_name),
                ));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}input:             {}",
                    value_to_string(&params.request)
                ));
                logln(format!(
                    "{pad}result:            {}",
                    value_to_string(&params.response)
                ));
            }
            PublicOplogEntry::ExportedFunctionInvoked(params) => {
                logln(format!(
                    "{} {}",
                    format_message_highlight("INVOKE"),
                    format_id(&params.function_name),
                ));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}idempotency key:   {}",
                    format_id(&params.idempotency_key),
                ));
                logln(format!("{pad}input:"));
                for param in &params.request {
                    logln(format!("{pad}  - {}", value_to_string(param)));
                }
            }
            PublicOplogEntry::ExportedFunctionCompleted(params) => {
                logln(format_message_highlight("INVOKE COMPLETED"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}consumed fuel:     {}",
                    format_id(&params.consumed_fuel),
                ));
                logln(format!(
                    "{pad}result:            {}",
                    params
                        .response
                        .as_ref()
                        .map(value_to_string)
                        .unwrap_or_else(|| "()".to_string())
                ));
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
            PublicOplogEntry::ChangeRetryPolicy(params) => {
                logln(format_message_highlight("CHANGE RETRY POLICY"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}max attempts:      {}",
                    format_id(&params.new_policy.max_attempts),
                ));
                logln(format!(
                    "{pad}min delay:         {} ms",
                    format_id(&params.new_policy.min_delay.as_millis()),
                ));
                logln(format!(
                    "{pad}max delay:         {} ms",
                    format_id(&params.new_policy.max_delay.as_millis()),
                ));
                logln(format!(
                    "{pad}multiplier:        {}",
                    format_id(&params.new_policy.multiplier),
                ));
                logln(format!(
                    "{pad}max jitter factor: {}",
                    format_id(
                        &params
                            .new_policy
                            .max_jitter_factor
                            .map(|x| x.to_string())
                            .unwrap_or("-".to_string()),
                    ),
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
            PublicOplogEntry::BeginRemoteWrite(params) => {
                logln(format_message_highlight("BEGIN REMOTE WRITE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
            }
            PublicOplogEntry::EndRemoteWrite(params) => {
                logln(format_message_highlight("END REMOTE WRITE"));
                logln(format!(
                    "{pad}at:                {}",
                    format_id(&params.timestamp)
                ));
                logln(format!(
                    "{pad}begin index:       {}",
                    format_id(&params.begin_index)
                ));
            }
            PublicOplogEntry::PendingWorkerInvocation(params) => match &params.invocation {
                PublicWorkerInvocation::ExportedFunction(inner_params) => {
                    logln(format!(
                        "{} {}",
                        format_message_highlight("ENQUEUED INVOCATION"),
                        format_id(&inner_params.full_function_name),
                    ));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}idempotency key:   {}",
                        format_id(&inner_params.idempotency_key),
                    ));
                    if let Some(input) = &inner_params.function_input {
                        logln(format!("{pad}input:"));
                        for param in input {
                            logln(format!("{pad}  - {}", value_to_string(param)));
                        }
                    }
                }
                PublicWorkerInvocation::ManualUpdate(inner_params) => {
                    logln(format_message_highlight("ENQUEUED MANUAL UPDATE"));
                    logln(format!(
                        "{pad}at:                {}",
                        format_id(&params.timestamp)
                    ));
                    logln(format!(
                        "{pad}target revision:   {}",
                        format_id(&inner_params.target_revision),
                    ));
                }
            },
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
                    format_id(&format!("{:?}", &params.persistence_level))
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

fn value_to_string(value: &ValueAndType) -> String {
    print_value_and_type(value).expect("Failed to convert value to string")
}

// TODO: pretty print
fn format_worker_name(worker_name: &WorkerName) -> String {
    textwrap::wrap(&worker_name.to_string(), 80).join("\n")
}

#[allow(dead_code)]
fn log_data_value(pad: &str, value: &DataValue) {
    match value {
        DataValue::Tuple(values) => {
            logln(format!("{pad}  tuple:"));
            for value in &values.elements {
                log_element_value(&format!("{pad}    "), value);
            }
        }
        DataValue::Multimodal(values) => {
            logln(format!("{pad}  multi-modal:"));
            for value in &values.elements {
                log_element_value(&format!("{pad}    "), &value.value);
            }
        }
    }
}

#[allow(dead_code)]
fn log_element_value(pad: &str, value: &ElementValue) {
    match value {
        ElementValue::ComponentModel(value) => {
            logln(format!("{pad}- {}", value_to_string(value)));
        }
        ElementValue::UnstructuredText(value) => match value {
            TextReference::Url(url) => {
                logln(format!("{pad}- URL: {}", format_id(&url.value)));
            }
            TextReference::Inline(inline) => {
                logln(format!("{pad}- Inline: {}", format_id(&inline.data)));
                if let Some(text_type) = &inline.text_type {
                    logln(format!(
                        "{pad}  Language code: {}",
                        format_id(&text_type.language_code)
                    ));
                }
            }
        },
        ElementValue::UnstructuredBinary(value) => match value {
            BinaryReference::Url(url) => {
                logln(format!("{pad}- URL: {}", format_id(&url.value)));
            }
            BinaryReference::Inline(inline) => {
                logln(format!(
                    "{pad}- Inline: {} bytes",
                    format_id(&inline.data.len().to_string())
                ));
                logln(format!(
                    "{pad}  MIME type: {}",
                    format_id(&inline.binary_type.mime_type)
                ));
            }
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerFilesView {
    pub nodes: Vec<FileNodeView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNodeView {
    pub name: String,
    pub last_modified: String, // Human-readable timestamp
    pub kind: String,
    pub permissions: String,
    pub size: u64,
}

#[derive(Table)]
pub struct WorkerFileNodeTableView {
    #[table(title = "Name")]
    pub name: String,
    #[table(title = "Kind")]
    pub kind: String,
    #[table(title = "Permissions")]
    pub permissions: String,
    #[table(title = "Size", justify = "Justify::Right")]
    pub size: u64,
    #[table(title = "Last Modified", justify = "Justify::Right")]
    pub last_modified: String,
}

impl From<&FileNodeView> for WorkerFileNodeTableView {
    fn from(value: &FileNodeView) -> Self {
        Self {
            name: value.name.clone(),
            kind: value.kind.clone(),
            permissions: value.permissions.clone(),
            size: value.size,
            last_modified: value.last_modified.clone(),
        }
    }
}

impl TextView for WorkerFilesView {
    fn log(&self) {
        if self.nodes.is_empty() {
            logln("No files found.");
        } else {
            log_table::<_, WorkerFileNodeTableView>(&self.nodes);
        }
    }
}

// Helper function to convert Unix timestamp to human-readable format
pub fn format_timestamp(timestamp: u64) -> String {
    if let Some(datetime) = DateTime::from_timestamp(timestamp as i64, 0) {
        datetime.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        format!("{timestamp}") // Fallback to raw timestamp if conversion fails
    }
}
