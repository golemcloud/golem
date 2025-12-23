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

use golem_common::model::agent::{BinaryReference, DataValue, ElementValue, TextReference};
use golem_common::model::oplog::{
    PluginInstallationDescription, PublicAttributeValue, PublicOplogEntry, PublicUpdateDescription,
    PublicWorkerInvocation, StringAttributeValue,
};
use golem_wasm::{print_value_and_type, ValueAndType};
use std::fmt::Write;

// backported from golem-cli to help debugging worker executor issues
pub fn debug_render_oplog_entry(entry: &PublicOplogEntry) -> String {
    let mut result = String::new();
    let pad = "          ";

    match entry {
        PublicOplogEntry::Create(params) => {
            let _ = writeln!(result, "CREATE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}component revision: {}",
                &params.component_revision,
            );
            let _ = writeln!(result, "{pad}env:");
            for (k, v) in &params.env {
                let _ = writeln!(result, "{pad}  - {}: {}", k, &v);
            }
            if let Some(parent) = params.parent.as_ref() {
                let _ = writeln!(result, "{pad}parent:            {parent}");
            }
            let _ = writeln!(result, "{pad}initial active plugins:");
            for plugin in &params.initial_active_plugins {
                let _ = writeln!(
                    result,
                    "{pad}  - plugin priority: {}",
                    &plugin.plugin_priority
                );
                let inner_pad = format!("{pad}    ");
                log_plugin_description(&mut result, &inner_pad, plugin);
            }
        }
        PublicOplogEntry::ImportedFunctionInvoked(params) => {
            let _ = writeln!(result, "CALL {}", &params.function_name,);
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}input:             {}",
                value_to_string(&params.request)
            );
            let _ = writeln!(
                result,
                "{pad}result:            {}",
                value_to_string(&params.response)
            );
        }
        PublicOplogEntry::ExportedFunctionInvoked(params) => {
            let _ = writeln!(result, "INVOKE {}", &params.function_name,);
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}idempotency key:   {}",
                &params.idempotency_key,
            );
            let _ = writeln!(result, "{pad}input:");
            for param in &params.request {
                let _ = writeln!(result, "{pad}  - {}", value_to_string(param));
            }
        }
        PublicOplogEntry::ExportedFunctionCompleted(params) => {
            let _ = writeln!(result, "INVOKE COMPLETED");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}consumed fuel:     {}", &params.consumed_fuel,);
            let _ = writeln!(
                result,
                "{pad}result:            {}",
                params
                    .response
                    .as_ref()
                    .map(value_to_string)
                    .unwrap_or("()".to_string())
            );
        }
        PublicOplogEntry::Suspend(params) => {
            let _ = writeln!(result, "SUSPEND");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::Error(params) => {
            let _ = writeln!(result, "ERROR");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}error:             {}", &params.error);
            let _ = writeln!(result, "{pad}retry from:        {}", &params.retry_from);
        }
        PublicOplogEntry::NoOp(params) => {
            let _ = writeln!(result, "NOP");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::Jump(params) => {
            let _ = writeln!(result, "JUMP");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}from:              {}", &params.jump.start);
            let _ = writeln!(result, "{pad}to:                {}", &params.jump.end);
        }
        PublicOplogEntry::Interrupted(params) => {
            let _ = writeln!(result, "INTERRUPTED");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::Exited(params) => {
            let _ = writeln!(result, "EXITED");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::ChangeRetryPolicy(params) => {
            let _ = writeln!(result, "CHANGE RETRY POLICY");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}max attempts:      {}",
                &params.new_policy.max_attempts,
            );
            let _ = writeln!(
                result,
                "{pad}min delay:         {} ms",
                &params.new_policy.min_delay.as_millis(),
            );
            let _ = writeln!(
                result,
                "{pad}max delay:         {} ms",
                &params.new_policy.max_delay.as_millis(),
            );
            let _ = writeln!(
                result,
                "{pad}multiplier:        {}",
                &params.new_policy.multiplier,
            );
            let _ = writeln!(
                result,
                "{pad}max jitter factor: {}",
                &params
                    .new_policy
                    .max_jitter_factor
                    .map(|x| x.to_string())
                    .unwrap_or("-".to_string()),
            );
        }
        PublicOplogEntry::BeginAtomicRegion(params) => {
            let _ = writeln!(result, "BEGIN ATOMIC REGION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::EndAtomicRegion(params) => {
            let _ = writeln!(result, "END ATOMIC REGION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}begin index:       {}", &params.begin_index);
        }
        PublicOplogEntry::BeginRemoteWrite(params) => {
            let _ = writeln!(result, "BEGIN REMOTE WRITE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::EndRemoteWrite(params) => {
            let _ = writeln!(result, "END REMOTE WRITE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}begin index:       {}", &params.begin_index);
        }
        PublicOplogEntry::PendingWorkerInvocation(params) => match &params.invocation {
            PublicWorkerInvocation::ExportedFunction(inner_params) => {
                let _ = writeln!(
                    result,
                    "ENQUEUED INVOCATION {}",
                    &inner_params.full_function_name,
                );
                let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
                let _ = writeln!(
                    result,
                    "{pad}idempotency key:   {}",
                    &inner_params.idempotency_key,
                );
                if let Some(input) = &inner_params.function_input {
                    let _ = writeln!(result, "{pad}input:");
                    for param in input {
                        let _ = writeln!(result, "{pad}  - {}", value_to_string(param));
                    }
                }
            }
            PublicWorkerInvocation::ManualUpdate(inner_params) => {
                let _ = writeln!(result, "ENQUEUED MANUAL UPDATE");
                let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
                let _ = writeln!(
                    result,
                    "{pad}target revision: {}",
                    &inner_params.target_revision,
                );
            }
        },
        PublicOplogEntry::PendingUpdate(params) => {
            let _ = writeln!(result, "ENQUEUED UPDATE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}target revision:   {}",
                &params.target_revision,
            );
            match &params.description {
                PublicUpdateDescription::Automatic(_) => {
                    let _ = writeln!(result, "{pad}type:              automatic");
                }
                PublicUpdateDescription::SnapshotBased(_inner_params) => {
                    let _ = writeln!(result, "{pad}type:              snapshot based");
                }
            }
        }
        PublicOplogEntry::SuccessfulUpdate(params) => {
            let _ = writeln!(result, "SUCCESSFUL UPDATE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}target revision:   {}",
                &params.target_revision,
            );
            let _ = writeln!(result, "{pad}new active plugins:");
            for plugin in &params.new_active_plugins {
                let _ = writeln!(
                    result,
                    "{pad}  - plugin priority: {}",
                    &plugin.plugin_priority,
                );
                let inner_pad = format!("{pad}    ");
                log_plugin_description(&mut result, &inner_pad, plugin);
            }
        }
        PublicOplogEntry::FailedUpdate(params) => {
            let _ = writeln!(result, "FAILED UPDATE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}target revision:   {}",
                &params.target_revision,
            );
            if let Some(details) = &params.details {
                let _ = writeln!(result, "{pad}error:             {details}");
            }
        }
        PublicOplogEntry::GrowMemory(params) => {
            let _ = writeln!(result, "GROW MEMORY");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}increase:          {}", &params.delta,);
        }
        PublicOplogEntry::CreateResource(params) => {
            let _ = writeln!(result, "CREATE RESOURCE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}resource id:       {}", &params.id);
        }
        PublicOplogEntry::DropResource(params) => {
            let _ = writeln!(result, "DROP RESOURCE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}resource id:       {}", &params.id);
        }
        PublicOplogEntry::Log(params) => {
            let _ = writeln!(result, "LOG");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}level:             {:?}", params.level,);
            let _ = writeln!(result, "{pad}message:           {}", params.message);
        }
        PublicOplogEntry::Restart(params) => {
            let _ = writeln!(result, "RESTART");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
        }
        PublicOplogEntry::ActivatePlugin(params) => {
            let _ = writeln!(result, "ACTIVATE PLUGIN");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}plugin priority:   {}",
                &params.plugin.plugin_priority,
            );
            log_plugin_description(&mut result, pad, &params.plugin);
        }
        PublicOplogEntry::DeactivatePlugin(params) => {
            let _ = writeln!(result, "DEACTIVATE PLUGIN");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}plugin priority:   {}",
                &params.plugin.plugin_priority,
            );
            log_plugin_description(&mut result, pad, &params.plugin);
        }
        PublicOplogEntry::Revert(params) => {
            let _ = writeln!(result, "REVERT");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}to oplog index:    {}",
                &params.dropped_region.start.previous(),
            );
        }
        PublicOplogEntry::CancelPendingInvocation(params) => {
            let _ = writeln!(result, "CANCEL INVOCATION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}idempotency key:   {}",
                &params.idempotency_key,
            );
        }
        PublicOplogEntry::StartSpan(params) => {
            let _ = writeln!(result, "START SPAN");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}span id:           {}", &params.span_id);
            if let Some(parent_id) = &params.parent_id {
                let _ = writeln!(result, "{pad}parent span:       {}", &parent_id,);
            }
            if let Some(linked_id) = &params.linked_context {
                let _ = writeln!(result, "{pad}linked span:       {}", &linked_id,);
            }
            let _ = writeln!(result, "{pad}attributes:");
            for attr in &params.attributes {
                let _ = writeln!(
                    result,
                    "{pad}  - {}: {}",
                    attr.key,
                    match &attr.value {
                        PublicAttributeValue::String(StringAttributeValue { value }) => value,
                    }
                );
            }
        }
        PublicOplogEntry::FinishSpan(params) => {
            let _ = writeln!(result, "FINISH SPAN");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}span id:           {}", &params.span_id);
        }
        PublicOplogEntry::SetSpanAttribute(params) => {
            let _ = writeln!(result, "SET SPAN ATTRIBUTE");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}span id:           {}", &params.span_id);
            let _ = writeln!(result, "{pad}key:               {}", &params.key);
            let _ = writeln!(
                result,
                "{pad}value:             {}",
                match &params.value {
                    PublicAttributeValue::String(StringAttributeValue { value }) => value,
                }
            );
        }
        PublicOplogEntry::ChangePersistenceLevel(params) => {
            let _ = writeln!(result, "CHANGE PERSISTENCE LEVEL");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(
                result,
                "{pad}level:             {:?}",
                &params.persistence_level,
            );
        }
        PublicOplogEntry::BeginRemoteTransaction(params) => {
            let _ = writeln!(result, "BEGIN REMOTE TRANSACTION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}transaction id:    {}", &params.transaction_id);
        }
        PublicOplogEntry::PreCommitRemoteTransaction(params) => {
            let _ = writeln!(result, "PRE COMMIT REMOTE TRANSACTION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}begin index:       {}", &params.begin_index);
        }
        PublicOplogEntry::PreRollbackRemoteTransaction(params) => {
            let _ = writeln!(result, "PRE ROLLBACK REMOTE TRANSACTION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}begin index:       {}", &params.begin_index);
        }
        PublicOplogEntry::CommittedRemoteTransaction(params) => {
            let _ = writeln!(result, "COMMITED REMOTE TRANSACTION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}begin index:       {}", &params.begin_index);
        }
        PublicOplogEntry::RolledBackRemoteTransaction(params) => {
            let _ = writeln!(result, "ROLLED BACK REMOTE TRANSACTION");
            let _ = writeln!(result, "{pad}at:                {}", &params.timestamp);
            let _ = writeln!(result, "{pad}begin index:       {}", &params.begin_index);
        }
    }

    result
}

fn log_plugin_description(output: &mut String, pad: &str, value: &PluginInstallationDescription) {
    let _ = writeln!(output, "{pad}plugin name:       {}", &value.plugin_name);
    let _ = writeln!(output, "{pad}plugin version:    {}", &value.plugin_version,);
    let _ = writeln!(
        output,
        "{pad}plugin parameters:    {}",
        &value.plugin_version,
    );
    for (k, v) in &value.parameters {
        let _ = writeln!(output, "{pad}  - {}: {}", k, &v);
    }
}

fn value_to_string(value: &ValueAndType) -> String {
    print_value_and_type(value).unwrap_or_else(|_| format!("{value:?}"))
}

#[allow(dead_code)]
fn log_data_value(output: &mut String, pad: &str, value: &DataValue) {
    match value {
        DataValue::Tuple(values) => {
            let _ = writeln!(output, "{pad}  tuple:");
            for value in &values.elements {
                log_element_value(output, &format!("{pad}    "), value);
            }
        }
        DataValue::Multimodal(values) => {
            let _ = writeln!(output, "{pad}  multi-modal:");
            for value in &values.elements {
                log_element_value(output, &format!("{pad}    "), &value.value);
            }
        }
    }
}

#[allow(dead_code)]
fn log_element_value(output: &mut String, pad: &str, value: &ElementValue) {
    match value {
        ElementValue::ComponentModel(value) => {
            let _ = writeln!(output, "{pad}- {}", value_to_string(value));
        }
        ElementValue::UnstructuredText(value) => match value {
            TextReference::Url(url) => {
                let _ = writeln!(output, "{pad}- URL: {}", url.value);
            }
            TextReference::Inline(inline) => {
                let _ = writeln!(output, "{pad}- Inline: {}", inline.data);
                if let Some(text_type) = &inline.text_type {
                    let _ = writeln!(output, "{pad}  Language code: {}", text_type.language_code);
                }
            }
        },
        ElementValue::UnstructuredBinary(value) => match value {
            BinaryReference::Url(url) => {
                let _ = writeln!(output, "{pad}- URL: {}", url.value);
            }
            BinaryReference::Inline(inline) => {
                let _ = writeln!(output, "{pad}- Inline: {} bytes", inline.data.len());
                let _ = writeln!(output, "{pad}  MIME type: {}", inline.binary_type.mime_type);
            }
        },
    }
}
