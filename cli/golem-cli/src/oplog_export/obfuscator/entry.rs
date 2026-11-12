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

//! The exhaustive entry match. Every public entry case and every field has an explicit rule;
//! a field without a kept rule is a token (default deny).

use super::host::Direction;
use super::value::{Region, Walker, flag, kept_case, number, record, text};
use super::{ObfuscationContext, agent_reference};
use crate::oplog_export::model::{
    CaseKind, OmittedReason, ReportEntry, ReportField, ReportNode, ReportOmission,
    SchemaLessRpcValueRef, TimestampNode, TupleNode,
};
use golem_common::model::card::{
    AccountCardHolder, AgentCardHolder, ApplicationCardHolder, PublicCardHolder,
};
use golem_common::model::oplog::public_oplog_entry::*;
use golem_common::model::oplog::{
    AgentInvocationOutputParameters, DurableInvocationId, ExternalToolResultParameters,
    FallibleResultParameters, LoadSnapshotParameters, LogTraceContext, ManualUpdateParameters,
    MultipartPartData, OmittedOplogEntry, ProcessOplogEntriesParameters,
    ProcessOplogEntriesResultParameters, PublicAgentInvocation, PublicAgentInvocationResult,
    PublicEntityInvocation, PublicEntityInvocationContext, PublicEntityInvocationOperation,
    PublicExternalToolResult, PublicFailedSnapshotAssistedUpdateDetails, PublicOplogEntry,
    PublicOplogEntryAttribution, PublicOplogEntryWithIndex, PublicQueuedCardEvent,
    PublicQueuedCardEventCard, PublicQueuedCardEventTransfer,
    PublicQueuedCardEventTransferReceived, PublicRetryPolicyState,
    PublicSnapshotAssistedUpdateDetails, PublicSnapshotData, PublicSpanAttributes,
    PublicSpanFinished, PublicSpanStarted, PublicToolInvocationOperation,
    PublicTypedAgentConfigEntry, PublicUpdateDescription, SaveSnapshotResultParameters,
};
use golem_common::model::{AgentId, OplogIndex, Timestamp};
use golem_common::schema::{IntoTypedSchemaValue, SchemaValue, TypedSchemaValue};
use serde::Serialize;

/// One rendered entry with what the coverage document needs from it.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedEntry {
    pub entry: ReportEntry,
    /// `true` for an `End` or `Cancelled` with a value whose `Start` function is unknown.
    pub provenance_unknown: bool,
    pub schema_less_rpc_values: Vec<SchemaLessRpcValueRef>,
}

/// Renders one public oplog entry. `start_function` is the host function name of the `Start`
/// at the `start_index` of an `End` or `Cancelled`, or `None` when it is unknown; other entry
/// kinds ignore it.
pub fn render_entry(
    ctx: &ObfuscationContext,
    entry: &PublicOplogEntryWithIndex,
    start_function: Option<&str>,
) -> RenderedEntry {
    let PublicOplogEntryWithIndex {
        oplog_index,
        attribution,
        entry,
    } = entry;
    let mut walker = Walker::new(ctx);
    let attribution = walker.attribution(attribution);
    let (body, provenance_unknown) = walker.body(entry, start_function);
    let index = oplog_index.as_u64();
    RenderedEntry {
        entry: ReportEntry {
            index,
            timestamp: entry.timestamp(),
            kind: entry.entry_type().to_string(),
            attribution,
            body: Some(body),
            omitted: None,
        },
        provenance_unknown,
        schema_less_rpc_values: walker
            .rpc_values
            .into_iter()
            .map(|path| SchemaLessRpcValueRef {
                index,
                path: path.to_string(),
            })
            .collect(),
    }
}

/// Renders an entry that the CLI did not read in full.
pub fn render_omitted(
    ctx: &ObfuscationContext,
    entry: &OmittedOplogEntry,
    reason: OmittedReason,
) -> ReportEntry {
    let OmittedOplogEntry {
        oplog_index,
        attribution,
        timestamp,
        entry_type,
        size,
        component_revision: _,
    } = entry;
    let mut walker = Walker::new(ctx);
    ReportEntry {
        index: oplog_index.as_u64(),
        timestamp: *timestamp,
        kind: entry_type.clone(),
        attribution: walker.attribution(attribution),
        body: None,
        omitted: Some(ReportOmission {
            reason,
            size: *size,
        }),
    }
}

/// The fields of one rendered record, in order. An absent optional field is left out.
struct Fields(Vec<ReportField>);

impl Fields {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn add(mut self, name: &str, value: ReportNode) -> Self {
        self.0.push(ReportField {
            name: Some(name.to_string()),
            value,
        });
        self
    }

    fn opt(self, name: &str, value: Option<ReportNode>) -> Self {
        match value {
            Some(value) => self.add(name, value),
            None => self,
        }
    }

    fn build(self) -> ReportNode {
        record(None, self.0)
    }
}

fn idx(index: &OplogIndex) -> ReportNode {
    number(index.as_u64())
}

fn timestamp(value: &Timestamp) -> ReportNode {
    ReportNode::Timestamp(TimestampNode { value: *value })
}

fn tuple(elements: Vec<ReportNode>) -> ReportNode {
    ReportNode::Tuple(TupleNode { elements })
}

/// The serde name of a Golem enum value: its string form, or the `type` tag of a tagged
/// enum.
fn serde_name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        Ok(serde_json::Value::Object(map)) => map
            .get("type")
            .and_then(|tag| tag.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn name<T: Serialize>(value: &T) -> ReportNode {
    text(serde_name(value))
}

/// Kept Golem metadata of a nested structure (retry policy state): names, numbers and flags
/// as they are. Only used for structures that hold no customer data.
fn kept_json(value: &serde_json::Value) -> ReportNode {
    match value {
        serde_json::Value::Null => tuple(Vec::new()),
        serde_json::Value::Bool(value) => flag(*value),
        serde_json::Value::Number(n) => match n.as_u64() {
            Some(n) => number(n),
            None => text(n.to_string()),
        },
        serde_json::Value::String(s) => text(s.clone()),
        serde_json::Value::Array(elements) => tuple(elements.iter().map(kept_json).collect()),
        serde_json::Value::Object(map) => {
            let fields: Vec<ReportField> = map
                .iter()
                .filter(|(key, _)| key.as_str() != "type")
                .map(|(key, value)| ReportField {
                    name: Some(key.clone()),
                    value: kept_json(value),
                })
                .collect();
            match map.get("type").and_then(|tag| tag.as_str()) {
                Some(case) => kept_case(
                    CaseKind::Variant,
                    None,
                    case,
                    None,
                    (!fields.is_empty()).then(|| record(None, fields)),
                ),
                None => record(None, fields),
            }
        }
    }
}

/// A kept Golem enum or structure: a tagged enum is a case node with its serde `type` tag.
fn kept<T: Serialize>(value: &T) -> ReportNode {
    kept_json(&serde_json::to_value(value).unwrap_or_default())
}

/// The serialised form of a Golem value as one text: its string form, or its JSON text.
fn serde_text<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn snapshot_size(data: &PublicSnapshotData) -> (u64, Option<String>) {
    fn part_size(data: &MultipartPartData) -> u64 {
        match data {
            MultipartPartData::Raw(raw) => raw.data.len() as u64,
            MultipartPartData::Json(json) => json.data.to_string().len() as u64,
        }
    }
    match data {
        PublicSnapshotData::Raw(raw) => (raw.data.len() as u64, Some(raw.mime_type.clone())),
        PublicSnapshotData::Json(json) => (json.data.to_string().len() as u64, None),
        PublicSnapshotData::Multipart(multipart) => (
            multipart
                .parts
                .iter()
                .map(|part| part_size(&part.data))
                .sum(),
            Some(multipart.mime_type.clone()),
        ),
    }
}

/// Snapshot data is removed; its case, size and MIME type stay.
fn removed_snapshot(data: &PublicSnapshotData) -> ReportNode {
    let (size, mime_type) = snapshot_size(data);
    kept_case(
        CaseKind::Variant,
        None,
        &serde_name(data),
        None,
        Some(
            Fields::new()
                .add("size", number(size))
                .opt("mimeType", mime_type.map(text))
                .build(),
        ),
    )
}

fn snapshot_assisted(details: &PublicSnapshotAssistedUpdateDetails) -> ReportNode {
    let PublicSnapshotAssistedUpdateDetails {
        pending_update_index,
        source_component_revision,
        source_revision_start_index,
        snapshot_index,
    } = details;
    Fields::new()
        .add("pendingUpdateIndex", idx(pending_update_index))
        .add(
            "sourceComponentRevision",
            number(source_component_revision.get()),
        )
        .add("sourceRevisionStartIndex", idx(source_revision_start_index))
        .add("snapshotIndex", idx(snapshot_index))
        .build()
}

fn failed_snapshot_assisted(details: &PublicFailedSnapshotAssistedUpdateDetails) -> ReportNode {
    let PublicFailedSnapshotAssistedUpdateDetails {
        pending_update_index,
        source_component_revision,
        source_revision_start_index,
        snapshot_index,
    } = details;
    Fields::new()
        .add("pendingUpdateIndex", idx(pending_update_index))
        .add(
            "sourceComponentRevision",
            number(source_component_revision.get()),
        )
        .add("sourceRevisionStartIndex", idx(source_revision_start_index))
        .add("snapshotIndex", idx(snapshot_index))
        .build()
}

fn retry_state(state: &PublicRetryPolicyState) -> ReportNode {
    kept(state)
}

impl Walker<'_, '_> {
    /// A token over the serialised form of a Golem value: over its text when it serialises
    /// to a string, otherwise over its JSON text.
    fn tok<T: Serialize>(&self, value: &T) -> ReportNode {
        self.str_leaf(&serde_text(value), Region::Customer)
    }

    /// A collection of Golem values as one `list` token; with `--keep-shape` its length and an
    /// element token per value.
    fn tok_list<'v, T: Serialize + 'v>(
        &mut self,
        values: impl IntoIterator<Item = &'v T>,
    ) -> ReportNode {
        let elements = values
            .into_iter()
            .map(|value| SchemaValue::String(serde_text(value)))
            .collect();
        self.schemaless(&SchemaValue::List { elements }, Region::Customer)
    }

    fn int(&mut self, value: i64) -> ReportNode {
        self.schemaless(&SchemaValue::S64(value), Region::Customer)
    }

    fn uint(&mut self, value: u64) -> ReportNode {
        self.schemaless(&SchemaValue::U64(value), Region::Customer)
    }

    /// A time chosen by the guest, such as when a scheduled invocation may start: a datetime
    /// token that `--keep-scalars` shows.
    fn datetime(&mut self, value: &Timestamp) -> ReportNode {
        match chrono::DateTime::from_timestamp_millis(value.to_millis() as i64) {
            Some(datetime) => {
                self.schemaless(&SchemaValue::Datetime { value: datetime }, Region::Customer)
            }
            None => self.tok(value),
        }
    }

    fn boolean(&mut self, value: bool) -> ReportNode {
        self.schemaless(&SchemaValue::Bool(value), Region::Customer)
    }

    fn err_text(&self, value: &str) -> ReportNode {
        self.str_leaf(value, Region::Error)
    }

    fn agent(&self, agent_id: &AgentId) -> ReportNode {
        ReportNode::Agent(agent_reference(self.ctx, agent_id))
    }

    fn host(
        &mut self,
        function: Option<&str>,
        direction: Direction,
        value: &TypedSchemaValue,
    ) -> ReportNode {
        self.host_payload(function, direction, value).0
    }

    fn attribution(&mut self, attribution: &PublicOplogEntryAttribution) -> Option<ReportNode> {
        match attribution {
            PublicOplogEntryAttribution::Agent(_) => None,
            PublicOplogEntryAttribution::Entity(PublicEntityInvocationContext {
                invocation,
                ancestors,
            }) => {
                let payload = Fields::new()
                    .add("invocation", self.entity_invocation(invocation))
                    .add(
                        "ancestors",
                        tuple(
                            ancestors
                                .iter()
                                .map(|ancestor| self.entity_invocation(ancestor))
                                .collect(),
                        ),
                    )
                    .build();
                Some(kept_case(
                    CaseKind::Variant,
                    None,
                    "Entity",
                    None,
                    Some(payload),
                ))
            }
        }
    }

    fn entity_invocation(&mut self, invocation: &PublicEntityInvocation) -> ReportNode {
        let PublicEntityInvocation {
            entity,
            start_index,
            call_mode,
            operation,
        } = invocation;
        let operation = operation.as_ref().map(|operation| match operation {
            PublicEntityInvocationOperation::Tool(PublicToolInvocationOperation {
                command_path,
                has_stdin,
                has_stdout,
                declares_stdout,
                has_stderr,
                declares_stderr,
            }) => kept_case(
                CaseKind::Variant,
                None,
                "Tool",
                None,
                Some(
                    Fields::new()
                        .add("commandPath", self.tok(command_path))
                        .add("hasStdin", self.boolean(*has_stdin))
                        .add("hasStdout", self.boolean(*has_stdout))
                        .add("declaresStdout", self.boolean(*declares_stdout))
                        .add("hasStderr", self.boolean(*has_stderr))
                        .add("declaresStderr", self.boolean(*declares_stderr))
                        .build(),
                ),
            ),
        });
        Fields::new()
            .add(
                "entity",
                Fields::new()
                    .add("kind", name(&entity.kind))
                    .add("name", self.str_leaf(&entity.name, Region::Customer))
                    .build(),
            )
            .add("startIndex", idx(start_index))
            .add("callMode", name(call_mode))
            .opt("operation", operation)
            .build()
    }

    fn span_started(&self, span: &PublicSpanStarted) -> ReportNode {
        let PublicSpanStarted {
            span_id,
            trace_id,
            trace_states,
            parent_span_id,
            links,
            started_at,
            attributes,
            kind,
        } = span;
        Fields::new()
            .add("spanId", self.tok(span_id))
            .add("traceId", self.tok(trace_id))
            .add("traceStates", self.tok(trace_states))
            .opt(
                "parentSpanId",
                parent_span_id.as_ref().map(|id| self.tok(id)),
            )
            .add("links", self.tok(links))
            .add("startedAt", timestamp(started_at))
            .add("attributes", self.tok(attributes))
            .add("kind", name(kind))
            .build()
    }

    fn span_finished(&self, span: &PublicSpanFinished) -> ReportNode {
        let PublicSpanFinished {
            span_id,
            finished_at,
            outcome,
        } = span;
        Fields::new()
            .add("spanId", self.tok(span_id))
            .add("finishedAt", timestamp(finished_at))
            .add("outcome", name(outcome))
            .build()
    }

    fn span_attributes(&self, span: &PublicSpanAttributes) -> ReportNode {
        let PublicSpanAttributes {
            span_id,
            attributes,
        } = span;
        Fields::new()
            .add("spanId", self.tok(span_id))
            .add("attributes", self.tok(attributes))
            .build()
    }

    fn card_holder(&self, holder: &PublicCardHolder) -> ReportNode {
        let payload = match holder {
            PublicCardHolder::Account(AccountCardHolder { account_id }) => {
                Fields::new().add("accountId", self.tok(account_id)).build()
            }
            PublicCardHolder::Application(ApplicationCardHolder { application_id }) => {
                Fields::new()
                    .add("applicationId", self.tok(application_id))
                    .build()
            }
            PublicCardHolder::Agent(AgentCardHolder { agent_id }) => {
                Fields::new().add("agentId", self.agent(agent_id)).build()
            }
        };
        kept_case(
            CaseKind::Variant,
            None,
            &serde_name(holder),
            None,
            Some(payload),
        )
    }

    fn invocation(&mut self, invocation: &PublicAgentInvocation) -> ReportNode {
        let payload = match invocation {
            PublicAgentInvocation::AgentInitialization(params) => {
                let golem_common::model::oplog::AgentInitializationParameters {
                    idempotency_key,
                    constructor_parameters,
                    trace_id,
                    trace_states,
                    invocation_context,
                } = params;
                Some(
                    Fields::new()
                        .add("idempotencyKey", self.tok(idempotency_key))
                        .add(
                            "constructorParameters",
                            self.customer(constructor_parameters),
                        )
                        .add("traceId", self.tok(trace_id))
                        .add("traceStates", self.tok(trace_states))
                        .add("invocationContext", self.tok(invocation_context))
                        .build(),
                )
            }
            PublicAgentInvocation::AgentMethodInvocation(params) => {
                let golem_common::model::oplog::AgentMethodInvocationParameters {
                    idempotency_key,
                    method_name,
                    function_input,
                    trace_id,
                    trace_states,
                    invocation_context,
                } = params;
                Some(
                    Fields::new()
                        .add("idempotencyKey", self.tok(idempotency_key))
                        .add("methodName", text(method_name.clone()))
                        .add("functionInput", self.customer(function_input))
                        .add("traceId", self.tok(trace_id))
                        .add("traceStates", self.tok(trace_states))
                        .add("invocationContext", self.tok(invocation_context))
                        .build(),
                )
            }
            PublicAgentInvocation::ExternalTool(params) => {
                let golem_common::model::oplog::ExternalToolInvocationParameters {
                    idempotency_key,
                    tool_name,
                    command_path,
                    input,
                    trace_id,
                    trace_states,
                    invocation_context,
                } = params;
                Some(
                    Fields::new()
                        .add("idempotencyKey", self.tok(idempotency_key))
                        .add("toolName", self.str_leaf(tool_name, Region::Customer))
                        .add("commandPath", self.tok(command_path))
                        .add("input", self.customer(input))
                        .add("traceId", self.tok(trace_id))
                        .add("traceStates", self.tok(trace_states))
                        .add("invocationContext", self.tok(invocation_context))
                        .build(),
                )
            }
            PublicAgentInvocation::SaveSnapshot(golem_common::model::Empty {}) => None,
            PublicAgentInvocation::LoadSnapshot(LoadSnapshotParameters { snapshot }) => Some(
                Fields::new()
                    .add("snapshot", removed_snapshot(snapshot))
                    .build(),
            ),
            PublicAgentInvocation::ProcessOplogEntries(ProcessOplogEntriesParameters {
                idempotency_key,
            }) => Some(
                Fields::new()
                    .add("idempotencyKey", self.tok(idempotency_key))
                    .build(),
            ),
            PublicAgentInvocation::ManualUpdate(ManualUpdateParameters { target_revision }) => {
                Some(
                    Fields::new()
                        .add("targetRevision", number(target_revision.get()))
                        .build(),
                )
            }
        };
        kept_case(
            CaseKind::Variant,
            None,
            &serde_name(invocation),
            None,
            payload,
        )
    }

    fn invocation_result(&mut self, result: &PublicAgentInvocationResult) -> ReportNode {
        let payload = match result {
            PublicAgentInvocationResult::AgentInitialization(AgentInvocationOutputParameters {
                output,
            })
            | PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
                output,
            }) => Some(Fields::new().add("output", self.customer(output)).build()),
            PublicAgentInvocationResult::ExternalTool(ExternalToolResultParameters { result }) => {
                let result = match result {
                    PublicExternalToolResult::Success(success) => {
                        let node = match success.into_typed_schema_value() {
                            Ok(typed) => self.customer(&typed),
                            Err(_) => self.tok(success),
                        };
                        kept_case(CaseKind::Variant, None, "Success", None, Some(node))
                    }
                    PublicExternalToolResult::Failure(failure) => {
                        let node = match failure.into_typed_schema_value() {
                            Ok(typed) => self.typed(
                                typed.graph(),
                                typed.root_type(),
                                typed.value(),
                                Region::Error,
                            ),
                            Err(_) => self.tok(failure),
                        };
                        kept_case(CaseKind::Variant, None, "Failure", None, Some(node))
                    }
                };
                Some(Fields::new().add("result", result).build())
            }
            PublicAgentInvocationResult::ManualUpdate(golem_common::model::Empty {}) => None,
            PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters { error }) => Some(
                Fields::new()
                    .opt("error", error.as_deref().map(|e| self.err_text(e)))
                    .build(),
            ),
            PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
                snapshot,
            }) => Some(
                Fields::new()
                    .add("snapshot", removed_snapshot(snapshot))
                    .build(),
            ),
            PublicAgentInvocationResult::ProcessOplogEntries(
                ProcessOplogEntriesResultParameters { error },
            ) => Some(
                Fields::new()
                    .opt("error", error.as_deref().map(|e| self.err_text(e)))
                    .build(),
            ),
        };
        kept_case(CaseKind::Variant, None, &serde_name(result), None, payload)
    }

    fn update_description(&self, description: &PublicUpdateDescription) -> ReportNode {
        let payload = match description {
            PublicUpdateDescription::Automatic(golem_common::model::Empty {}) => None,
            PublicUpdateDescription::SnapshotAssistedAutomatic(
                golem_common::model::oplog::SnapshotAssistedAutomaticUpdateParameters {},
            ) => None,
            PublicUpdateDescription::SnapshotBased(params) => {
                let golem_common::model::oplog::SnapshotBasedUpdateParameters {
                    payload,
                    mime_type,
                    filesystem_snapshot,
                } = params;
                Some(
                    Fields::new()
                        .add("size", number(payload.len() as u64))
                        .add("mimeType", text(mime_type.clone()))
                        .opt(
                            "filesystemSnapshot",
                            filesystem_snapshot.as_ref().map(|s| self.tok(s)),
                        )
                        .build(),
                )
            }
        };
        kept_case(
            CaseKind::Variant,
            None,
            &serde_name(description),
            None,
            payload,
        )
    }

    fn queued_card_event(&self, event: &PublicQueuedCardEvent) -> ReportNode {
        let payload = match event {
            PublicQueuedCardEvent::Install(PublicQueuedCardEventCard { card_id })
            | PublicQueuedCardEvent::Revoke(PublicQueuedCardEventCard { card_id }) => {
                Fields::new().add("cardId", self.tok(card_id)).build()
            }
            PublicQueuedCardEvent::TransferStarted(PublicQueuedCardEventTransfer {
                transfer_id,
                card_id,
                target_holder,
            }) => Fields::new()
                .add("transferId", self.tok(transfer_id))
                .add("cardId", self.tok(card_id))
                .add("targetHolder", self.card_holder(target_holder))
                .build(),
            PublicQueuedCardEvent::TransferReceived(PublicQueuedCardEventTransferReceived {
                transfer_id,
                card_id,
            }) => Fields::new()
                .add("transferId", self.tok(transfer_id))
                .add("cardId", self.tok(card_id))
                .build(),
        };
        kept_case(
            CaseKind::Variant,
            None,
            &serde_name(event),
            None,
            Some(payload),
        )
    }

    fn trace_context(&self, context: &LogTraceContext) -> ReportNode {
        let LogTraceContext { trace_id, span_id } = context;
        Fields::new()
            .add("traceId", self.tok(trace_id))
            .add("spanId", self.tok(span_id))
            .build()
    }

    /// The body of an entry and whether it is an `End` or `Cancelled` with a value whose
    /// `Start` function is unknown.
    fn body(
        &mut self,
        entry: &PublicOplogEntry,
        start_function: Option<&str>,
    ) -> (ReportNode, bool) {
        let mut provenance_unknown = false;
        let body = match entry {
            PublicOplogEntry::Create(CreateParams {
                timestamp: _,
                agent_id,
                owner_kind,
                agent_mode,
                component_revision,
                env,
                created_by,
                environment_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                local_agent_config,
                original_phantom_id,
                instance_id,
            }) => Fields::new()
                .add("agentId", self.agent(agent_id))
                .add("ownerKind", name(owner_kind))
                .add("agentMode", name(agent_mode))
                .add("componentRevision", number(component_revision.get()))
                .add(
                    "env",
                    record(
                        None,
                        env.iter()
                            .map(|(key, value)| ReportField {
                                name: Some(key.clone()),
                                value: self.str_leaf(value, Region::Customer),
                            })
                            .collect(),
                    ),
                )
                .add("createdBy", self.tok(created_by))
                .add("environmentId", self.tok(environment_id))
                .opt("parent", parent.as_ref().map(|parent| self.agent(parent)))
                .add("componentSize", number(*component_size))
                .add(
                    "initialTotalLinearMemorySize",
                    number(*initial_total_linear_memory_size),
                )
                .add(
                    "initialActivePlugins",
                    self.tok_list(initial_active_plugins),
                )
                .add(
                    "localAgentConfig",
                    tuple(
                        local_agent_config
                            .iter()
                            .map(|PublicTypedAgentConfigEntry { path, value }| {
                                Fields::new()
                                    .add(
                                        "path",
                                        tuple(path.iter().map(|p| text(p.clone())).collect()),
                                    )
                                    .add("value", self.customer(value))
                                    .build()
                            })
                            .collect(),
                    ),
                )
                .opt(
                    "originalPhantomId",
                    original_phantom_id.as_ref().map(|id| self.tok(id)),
                )
                .add("instanceId", self.tok(instance_id))
                .build(),
            PublicOplogEntry::Start(StartParams {
                timestamp: _,
                parent_start_index,
                function_name,
                invocation_id,
                observational_owner,
                request,
                durable_function_type,
                span_started,
                denial,
            }) => Fields::new()
                .opt("parentStartIndex", parent_start_index.as_ref().map(idx))
                .add("functionName", text(function_name.clone()))
                .opt(
                    "invocationId",
                    invocation_id
                        .as_ref()
                        .map(|DurableInvocationId { anchor, ordinal }| {
                            Fields::new()
                                .add("anchor", idx(anchor))
                                .add("ordinal", number(*ordinal))
                                .build()
                        }),
                )
                .opt("observationalOwner", observational_owner.as_ref().map(idx))
                .opt(
                    "request",
                    request
                        .as_ref()
                        .map(|r| self.host(Some(function_name), Direction::Request, r)),
                )
                .add("durableFunctionType", kept(durable_function_type))
                .opt(
                    "spanStarted",
                    span_started.as_ref().map(|s| self.span_started(s)),
                )
                .opt("denial", denial.as_deref().map(|d| self.err_text(d)))
                .build(),
            PublicOplogEntry::End(EndParams {
                timestamp: _,
                start_index,
                response,
                forced_commit,
                span_finished,
                span_attributes,
            }) => {
                provenance_unknown = start_function.is_none() && response.is_some();
                Fields::new()
                    .add("startIndex", idx(start_index))
                    .opt(
                        "response",
                        response
                            .as_ref()
                            .map(|r| self.host(start_function, Direction::Response, r)),
                    )
                    .add("forcedCommit", flag(*forced_commit))
                    .opt(
                        "spanFinished",
                        span_finished.as_ref().map(|s| self.span_finished(s)),
                    )
                    .opt(
                        "spanAttributes",
                        span_attributes.as_ref().map(|s| self.span_attributes(s)),
                    )
                    .build()
            }
            PublicOplogEntry::Cancelled(CancelledParams {
                timestamp: _,
                start_index,
                partial,
                span_finished,
            }) => {
                provenance_unknown = start_function.is_none() && partial.is_some();
                Fields::new()
                    .add("startIndex", idx(start_index))
                    .opt(
                        "partial",
                        partial
                            .as_ref()
                            .map(|p| self.host(start_function, Direction::Response, p)),
                    )
                    .opt(
                        "spanFinished",
                        span_finished.as_ref().map(|s| self.span_finished(s)),
                    )
                    .build()
            }
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                timestamp: _,
                invocation,
                wallet_pin,
            }) => Fields::new()
                .add("invocation", self.invocation(invocation))
                .add("walletPin", self.tok(wallet_pin))
                .build(),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: _,
                result,
                method_name,
                consumed_fuel,
                component_revision,
            }) => Fields::new()
                .add("result", self.invocation_result(result))
                .opt("methodName", method_name.as_ref().map(|m| text(m.clone())))
                .add("consumedFuel", self.int(*consumed_fuel))
                .add("componentRevision", number(component_revision.get()))
                .build(),
            PublicOplogEntry::Suspend(SuspendParams { timestamp: _ })
            | PublicOplogEntry::RecoverySucceeded(RecoverySucceededParams { timestamp: _ })
            | PublicOplogEntry::Interrupted(InterruptedParams { timestamp: _ })
            | PublicOplogEntry::Exited(ExitedParams { timestamp: _ })
            | PublicOplogEntry::Restart(RestartParams { timestamp: _ })
            | PublicOplogEntry::Resumed(ResumedParams { timestamp: _ }) => Fields::new().build(),
            PublicOplogEntry::Error(ErrorParams {
                timestamp: _,
                kind,
                error,
                retry_from,
                retry_policy_state,
            }) => Fields::new()
                .add("kind", name(kind))
                .add("error", self.agent_error_message(error))
                .add("retryFrom", idx(retry_from))
                .opt(
                    "retryPolicyState",
                    retry_policy_state.as_ref().map(retry_state),
                )
                .build(),
            PublicOplogEntry::Jump(JumpParams { timestamp: _, jump })
            | PublicOplogEntry::Revert(RevertParams {
                timestamp: _,
                dropped_region: jump,
            }) => {
                let field = if matches!(entry, PublicOplogEntry::Jump(_)) {
                    "jump"
                } else {
                    "droppedRegion"
                };
                Fields::new()
                    .add(
                        field,
                        Fields::new()
                            .add("start", idx(&jump.start))
                            .add("end", idx(&jump.end))
                            .build(),
                    )
                    .build()
            }
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp: _,
                invocation,
                not_before,
            }) => Fields::new()
                .add("invocation", self.invocation(invocation))
                .opt("notBefore", not_before.as_ref().map(|t| self.datetime(t)))
                .build(),
            PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                timestamp: _,
                target_revision,
                description,
                update_attempt_index,
            }) => Fields::new()
                .add("targetRevision", number(target_revision.get()))
                .add("description", self.update_description(description))
                .add("updateAttemptIndex", idx(update_attempt_index))
                .build(),
            PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                timestamp: _,
                target_revision,
                new_component_size,
                new_active_plugins,
                snapshot_assisted_details,
            }) => Fields::new()
                .add("targetRevision", number(target_revision.get()))
                .add("newComponentSize", number(*new_component_size))
                .add("newActivePlugins", self.tok_list(new_active_plugins))
                .opt(
                    "snapshotAssistedDetails",
                    snapshot_assisted_details.as_ref().map(snapshot_assisted),
                )
                .build(),
            PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp: _,
                target_revision,
                details,
                snapshot_assisted_details,
                update_attempt_index,
            }) => Fields::new()
                .add("targetRevision", number(target_revision.get()))
                .opt(
                    "details",
                    details.as_deref().map(|d| self.failed_update_details(d)),
                )
                .opt(
                    "snapshotAssistedDetails",
                    snapshot_assisted_details
                        .as_ref()
                        .map(failed_snapshot_assisted),
                )
                .opt("updateAttemptIndex", update_attempt_index.as_ref().map(idx))
                .build(),
            PublicOplogEntry::GrowMemory(GrowMemoryParams {
                timestamp: _,
                delta,
            }) => Fields::new().add("delta", number(*delta)).build(),
            PublicOplogEntry::CreateResource(CreateResourceParams {
                timestamp: _,
                id,
                name: resource_name,
                owner,
            })
            | PublicOplogEntry::DropResource(DropResourceParams {
                timestamp: _,
                id,
                name: resource_name,
                owner,
            }) => Fields::new()
                .add(
                    "id",
                    self.schemaless(&SchemaValue::U64(id.0), Region::Customer),
                )
                .add("name", text(resource_name.clone()))
                .add("owner", text(owner.clone()))
                .build(),
            PublicOplogEntry::Log(LogParams {
                timestamp: _,
                level,
                context,
                message,
                trace_context,
            }) => Fields::new()
                .add("level", name(level))
                .add("context", self.str_leaf(context, Region::Customer))
                .add("message", self.str_leaf(message, Region::Customer))
                .opt(
                    "traceContext",
                    trace_context.as_ref().map(|c| self.trace_context(c)),
                )
                .build(),
            PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
                timestamp: _,
                plugin,
            })
            | PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
                timestamp: _,
                plugin,
            }) => Fields::new().add("plugin", self.tok(plugin)).build(),
            PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
                timestamp: _,
                idempotency_key,
            }) => Fields::new()
                .add("idempotencyKey", self.tok(idempotency_key))
                .build(),
            PublicOplogEntry::BeginRemoteTransaction(BeginRemoteTransactionParams {
                timestamp: _,
                transaction_id,
            }) => Fields::new()
                .add("transactionId", self.tok(transaction_id))
                .build(),
            PublicOplogEntry::PreCommitRemoteTransaction(PreCommitRemoteTransactionParams {
                timestamp: _,
                begin_index,
            })
            | PublicOplogEntry::PreRollbackRemoteTransaction(
                PreRollbackRemoteTransactionParams {
                    timestamp: _,
                    begin_index,
                },
            )
            | PublicOplogEntry::CommittedRemoteTransaction(CommittedRemoteTransactionParams {
                timestamp: _,
                begin_index,
            })
            | PublicOplogEntry::RolledBackRemoteTransaction(RolledBackRemoteTransactionParams {
                timestamp: _,
                begin_index,
            }) => Fields::new().add("beginIndex", idx(begin_index)).build(),
            PublicOplogEntry::Snapshot(SnapshotParams {
                timestamp: _,
                data,
                filesystem_snapshot,
            }) => Fields::new()
                .add("data", removed_snapshot(data))
                .opt(
                    "filesystemSnapshot",
                    filesystem_snapshot.as_ref().map(|s| self.tok(s)),
                )
                .build(),
            PublicOplogEntry::OplogProcessorCheckpoint(OplogProcessorCheckpointParams {
                timestamp: _,
                plugin,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
            }) => Fields::new()
                .add("plugin", self.tok(plugin))
                .add("targetAgentId", self.agent(target_agent_id))
                .add("confirmedUpTo", idx(confirmed_up_to))
                .add("sendingUpTo", idx(sending_up_to))
                .add("lastBatchStart", idx(last_batch_start))
                .build(),
            PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
                timestamp: _,
                policy,
            }) => {
                let golem_common::model::oplog::PublicNamedRetryPolicy {
                    name: policy_name,
                    priority,
                    predicate,
                    policy,
                } = policy;
                Fields::new()
                    .add(
                        "policy",
                        Fields::new()
                            .add("name", self.str_leaf(policy_name, Region::Customer))
                            .add(
                                "priority",
                                self.schemaless(&SchemaValue::U32(*priority), Region::Customer),
                            )
                            .add("predicate", self.tok(predicate))
                            .add("policy", self.tok(policy))
                            .build(),
                    )
                    .build()
            }
            PublicOplogEntry::RemoveRetryPolicy(RemoveRetryPolicyParams {
                timestamp: _,
                name: policy_name,
            }) => Fields::new()
                .add("name", self.str_leaf(policy_name, Region::Customer))
                .build(),
            PublicOplogEntry::CardEventQueued(CardEventQueuedParams {
                timestamp: _,
                event,
            }) => Fields::new()
                .add("event", self.queued_card_event(event))
                .build(),
            PublicOplogEntry::CardInstalled(CardInstalledParams {
                timestamp: _,
                queued_event_index,
                card_id,
                wallet_generation,
            }) => Fields::new()
                .opt("queuedEventIndex", queued_event_index.as_ref().map(idx))
                .add("cardId", self.tok(card_id))
                .add("walletGeneration", self.uint(*wallet_generation))
                .build(),
            PublicOplogEntry::CardInstallFailed(CardInstallFailedParams {
                timestamp: _,
                queued_event_index,
                card_id,
                reason,
            }) => Fields::new()
                .add("queuedEventIndex", idx(queued_event_index))
                .add("cardId", self.tok(card_id))
                .add("reason", name(reason))
                .build(),
            PublicOplogEntry::CardRevoked(CardRevokedParams {
                timestamp: _,
                queued_event_index,
                card_id,
                wallet_generation,
            }) => Fields::new()
                .add("queuedEventIndex", idx(queued_event_index))
                .add("cardId", self.tok(card_id))
                .add("walletGeneration", self.uint(*wallet_generation))
                .build(),
            PublicOplogEntry::CardExpired(CardExpiredParams {
                timestamp: _,
                card_id,
                wallet_generation,
            }) => Fields::new()
                .add("cardId", self.tok(card_id))
                .add("walletGeneration", self.uint(*wallet_generation))
                .build(),
            PublicOplogEntry::CardDerived(CardDerivedParams {
                timestamp: _,
                card_id,
                parent_ids,
                wallet_generation,
            }) => Fields::new()
                .add("cardId", self.tok(card_id))
                .add("parentIds", self.tok_list(parent_ids))
                .add("walletGeneration", self.uint(*wallet_generation))
                .build(),
            PublicOplogEntry::CardTransferStarted(CardTransferStartedParams {
                timestamp: _,
                transfer_id,
                card_id,
                target_holder,
                source_wallet_generation,
            }) => Fields::new()
                .add("transferId", self.tok(transfer_id))
                .add("cardId", self.tok(card_id))
                .add("targetHolder", self.card_holder(target_holder))
                .add(
                    "sourceWalletGeneration",
                    self.uint(*source_wallet_generation),
                )
                .build(),
            PublicOplogEntry::CardTransferred(CardTransferredParams {
                timestamp: _,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                target_wallet_generation,
            }) => Fields::new()
                .add("transferId", self.tok(transfer_id))
                .add("sourceCardId", self.tok(source_card_id))
                .add("installedCardId", self.tok(installed_card_id))
                .add("targetHolder", self.card_holder(target_holder))
                .add(
                    "targetWalletGeneration",
                    self.uint(*target_wallet_generation),
                )
                .build(),
            PublicOplogEntry::CardRevokedCascade(CardRevokedCascadeParams {
                timestamp: _,
                revoked_card_ids,
                local_wallet_generation,
            }) => Fields::new()
                .add("revokedCardIds", self.tok_list(revoked_card_ids))
                .add("localWalletGeneration", self.uint(*local_wallet_generation))
                .build(),
            PublicOplogEntry::CardTransferConfirmed(CardTransferConfirmedParams {
                timestamp: _,
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
            }) => Fields::new()
                .add("transferId", self.tok(transfer_id))
                .add("sourceCardId", self.tok(source_card_id))
                .add("installedCardId", self.tok(installed_card_id))
                .add("targetHolder", self.card_holder(target_holder))
                .build(),
            PublicOplogEntry::HostStreamFrame(HostStreamFrameParams {
                timestamp: _,
                parent_start_index,
                kind,
                payload,
            }) => Fields::new()
                .add("parentStartIndex", idx(parent_start_index))
                .add("kind", name(kind))
                .add("payload", self.customer(payload))
                .build(),
            PublicOplogEntry::ToolAttachmentRead(ToolAttachmentReadParams {
                timestamp: _,
                observation,
                payload,
            }) => Fields::new()
                .add("observation", self.customer(observation))
                .opt(
                    "payload",
                    payload.as_ref().map(|bytes| {
                        self.schemaless(
                            &golem_common::schema::conversion::binary_to_value(bytes.clone(), None),
                            Region::Customer,
                        )
                    }),
                )
                .build(),
            PublicOplogEntry::ToolAttachmentControl(ToolAttachmentControlParams {
                timestamp: _,
                attachment,
                transition,
            }) => Fields::new()
                .add("attachment", self.customer(attachment))
                .add("transition", self.customer(transition))
                .build(),
            PublicOplogEntry::StreamRegistered(StreamRegisteredParams {
                timestamp: _,
                record,
            })
            | PublicOplogEntry::StreamItems(StreamItemsParams {
                timestamp: _,
                record,
            })
            | PublicOplogEntry::StreamEnd(StreamEndParams {
                timestamp: _,
                record,
            })
            | PublicOplogEntry::StreamCancel(StreamCancelParams {
                timestamp: _,
                record,
            })
            | PublicOplogEntry::StreamSession(StreamSessionParams {
                timestamp: _,
                record,
            }) => Fields::new().add("record", self.customer(record)).build(),
            PublicOplogEntry::CompletionDiscarded(CompletionDiscardedParams {
                timestamp: _,
                start_index,
            })
            | PublicOplogEntry::CompletionDelivered(CompletionDeliveredParams {
                timestamp: _,
                start_index,
            }) => Fields::new().add("startIndex", idx(start_index)).build(),
            PublicOplogEntry::SnapshotConfirmed(SnapshotConfirmedParams {
                timestamp: _,
                filesystem_snapshot,
            }) => Fields::new()
                .add("filesystemSnapshot", self.tok(filesystem_snapshot))
                .build(),
            PublicOplogEntry::SpanClosed(SpanClosedParams {
                timestamp: _,
                resource_start_index,
                span_finished,
            }) => Fields::new()
                .opt("resourceStartIndex", resource_start_index.as_ref().map(idx))
                .add("spanFinished", self.span_finished(span_finished))
                .build(),
        };
        (body, provenance_unknown)
    }
}
