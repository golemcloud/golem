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

use super::*;

async fn try_resolve_agent_id(
    component_service: Arc<dyn ComponentService>,
    agent_id: &AgentId,
) -> Option<ParsedAgentId> {
    if let Ok(component) = component_service
        .get_metadata(agent_id.component_id, None)
        .await
    {
        ParsedAgentId::parse(&agent_id.agent_id, &component.metadata).ok()
    } else {
        None
    }
}

pub(super) async fn enrich_golem_rpc_invoke(
    components: Arc<dyn ComponentService>,
    mut payload: HostRequestGolemRpcInvoke,
) -> HostRequestGolemRpcInvoke {
    let agent_id = try_resolve_agent_id(components, &payload.remote_agent_id).await;
    payload.remote_agent_type = agent_id
        .as_ref()
        .map(|agent_id| agent_id.agent_type.clone());
    payload.remote_agent_parameters = agent_id.map(|agent_id| agent_id.parameters);
    payload
}

pub(super) async fn enrich_golem_rpc_scheduled_invocation(
    components: Arc<dyn ComponentService>,
    mut payload: HostRequestGolemRpcScheduledInvocation,
) -> HostRequestGolemRpcScheduledInvocation {
    let agent_id = try_resolve_agent_id(components, &payload.remote_agent_id).await;
    payload.remote_agent_type = agent_id
        .as_ref()
        .map(|agent_id| agent_id.agent_type.clone());
    payload.remote_agent_parameters = agent_id.map(|agent_id| agent_id.parameters);
    payload
}

fn resolve_agent_type_from_worker_name<'a>(
    metadata: &'a golem_common::model::component_metadata::ComponentMetadata,
    worker_name: &str,
) -> Option<&'a golem_common::schema::agent::AgentTypeSchema> {
    ParsedAgentId::parse_agent_type_name(worker_name)
        .ok()
        .and_then(|type_name| metadata.find_agent_type_by_name_ref(&type_name))
}

/// A schema-native empty value (`()`), used as the best-effort fallback when
/// the driving schema cannot be resolved from the component metadata. Mirrors
/// the previous `DataValue::Tuple(empty)` fallback in the legacy renderer.
fn empty_typed_schema_value() -> TypedSchemaValue {
    TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
        SchemaValue::Tuple {
            elements: Vec::new(),
        },
    )
}

/// Build the self-contained graph of a public typed value: `root` together with
/// the named definitions it reaches in the agent type's definition registry.
/// Agent schemas refer to named types (records, variants, ...) through
/// `SchemaType::Ref`, so a graph without these definitions cannot be rendered.
fn self_contained_graph(registry: &SchemaGraph, root: SchemaType) -> SchemaGraph {
    SchemaGraph {
        defs: reachable_defs(registry, &root),
        root,
    }
}

/// Pair a schema-native invocation **input** value (a parameter record, see
/// [`crate::worker::invocation::lower_invocation`]) with the record schema
/// derived from the agent's declared [`InputSchema`]. `registry` is the agent
/// type's definition registry that the input schema's references resolve in.
///
/// The recorded value is caller-only: auto-injected fields (e.g. the principal)
/// travel out of band and are not part of it, so they are excluded from the
/// derived record schema to keep the schema and value arities aligned.
fn input_value_to_typed_schema_value(
    registry: &SchemaGraph,
    input_schema: &InputSchema,
    value: SchemaValue,
) -> Result<TypedSchemaValue, String> {
    let fields = input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    Ok(TypedSchemaValue::new(
        self_contained_graph(registry, SchemaType::record(fields)),
        value,
    ))
}

/// Pair a schema-native invocation **output** value with the schema derived
/// from the agent method's declared [`OutputSchema`]. `registry` is the agent
/// type's definition registry that the output schema's references resolve in.
/// A `unit` output is represented by the canonical empty tuple (see
/// [`crate::worker::invocation::decode_invoke_output`]).
fn output_value_to_typed_schema_value(
    registry: &SchemaGraph,
    output_schema: &OutputSchema,
    value: SchemaValue,
) -> Result<TypedSchemaValue, String> {
    let root = match output_schema.schema() {
        Some(ty) => ty.clone(),
        None => SchemaType::tuple(Vec::new()),
    };
    Ok(TypedSchemaValue::new(
        self_contained_graph(registry, root),
        value,
    ))
}

pub(super) async fn agent_invocation_to_public(
    components: Arc<dyn ComponentService>,
    owned_agent_id: &OwnedAgentId,
    component_revision: ComponentRevision,
    invocation: AgentInvocation,
) -> Result<PublicAgentInvocation, String> {
    match invocation {
        AgentInvocation::AgentInitialization {
            idempotency_key,
            input,
            invocation_context,
            ..
        } => {
            let metadata = components
                .get_metadata(
                    owned_agent_id.agent_id.component_id,
                    Some(component_revision),
                )
                .await
                .map_err(|err| err.to_string())?;

            let agent_type = resolve_agent_type_from_worker_name(
                &metadata.metadata,
                &owned_agent_id.agent_id.agent_id,
            );

            let constructor_parameters = match agent_type {
                Some(agent_type) => input_value_to_typed_schema_value(
                    &agent_type.schema,
                    &agent_type.constructor.input_schema,
                    input,
                )
                .unwrap_or_else(|_| empty_typed_schema_value()),
                None => empty_typed_schema_value(),
            };

            let span_data = invocation_context.to_oplog_data();

            Ok(PublicAgentInvocation::AgentInitialization(
                AgentInitializationParameters {
                    idempotency_key,
                    constructor_parameters,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::AgentMethod {
            idempotency_key,
            method_name,
            input,
            invocation_context,
            ..
        } => {
            let metadata = components
                .get_metadata(
                    owned_agent_id.agent_id.component_id,
                    Some(component_revision),
                )
                .await
                .map_err(|err| err.to_string())?;

            let agent_type = resolve_agent_type_from_worker_name(
                &metadata.metadata,
                &owned_agent_id.agent_id.agent_id,
            );

            let method = agent_type.and_then(|at| {
                at.methods
                    .iter()
                    .find(|m| m.name == method_name)
                    .map(|m| (at, m))
            });

            let function_input = match method {
                Some((agent_type, method)) => input_value_to_typed_schema_value(
                    &agent_type.schema,
                    &method.input_schema,
                    input,
                )
                .unwrap_or_else(|_| empty_typed_schema_value()),
                None => empty_typed_schema_value(),
            };

            let span_data = invocation_context.to_oplog_data();

            Ok(PublicAgentInvocation::AgentMethodInvocation(
                AgentMethodInvocationParameters {
                    idempotency_key,
                    method_name,
                    function_input,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::ExternalTool {
            idempotency_key,
            tool_name,
            command_path,
            input,
            invocation_context,
            ..
        } => {
            let span_data = invocation_context.to_oplog_data();
            Ok(PublicAgentInvocation::ExternalTool(
                ExternalToolInvocationParameters {
                    idempotency_key,
                    tool_name: tool_name.into_inner(),
                    command_path,
                    input: *input,
                    trace_id: invocation_context.trace_id.clone(),
                    trace_states: invocation_context.trace_states.clone(),
                    invocation_context: encode_span_data(&span_data),
                },
            ))
        }
        AgentInvocation::ManualUpdate { target_revision } => Ok(
            PublicAgentInvocation::ManualUpdate(ManualUpdateParameters { target_revision }),
        ),
        AgentInvocation::SaveSnapshot { .. } => Ok(PublicAgentInvocation::SaveSnapshot(Empty {})),
        AgentInvocation::LoadSnapshot { snapshot, .. } => Ok(PublicAgentInvocation::LoadSnapshot(
            LoadSnapshotParameters {
                snapshot: raw_snapshot_to_public(snapshot),
            },
        )),
        AgentInvocation::ProcessOplogEntries {
            idempotency_key, ..
        } => Ok(PublicAgentInvocation::ProcessOplogEntries(
            ProcessOplogEntriesParameters { idempotency_key },
        )),
    }
}

pub(super) async fn agent_invocation_result_to_public(
    components: Arc<dyn ComponentService>,
    owned_agent_id: &OwnedAgentId,
    component_revision: ComponentRevision,
    method_name: Option<String>,
    result: AgentInvocationResult,
) -> Result<PublicAgentInvocationResult, String> {
    match result {
        AgentInvocationResult::AgentInitialization => {
            let _ = components;
            let _ = owned_agent_id;
            let _ = component_revision;

            Ok(PublicAgentInvocationResult::AgentInitialization(
                AgentInvocationOutputParameters {
                    output: empty_typed_schema_value(),
                },
            ))
        }
        AgentInvocationResult::AgentMethod { output } => {
            // The persisted `method_name` lets us resolve the method's declared
            // output schema and pair it with the schema-native output value.
            let metadata = match &method_name {
                Some(_) => Some(
                    components
                        .get_metadata(
                            owned_agent_id.agent_id.component_id,
                            Some(component_revision),
                        )
                        .await
                        .map_err(|err| err.to_string())?,
                ),
                None => None,
            };

            let method =
                metadata
                    .as_ref()
                    .zip(method_name.as_ref())
                    .and_then(|(metadata, method_name)| {
                        resolve_agent_type_from_worker_name(
                            &metadata.metadata,
                            &owned_agent_id.agent_id.agent_id,
                        )
                        .and_then(|at| {
                            at.methods
                                .iter()
                                .find(|m| m.name == *method_name)
                                .map(|m| (at, m))
                        })
                    });

            let output = match method {
                Some((agent_type, method)) => output_value_to_typed_schema_value(
                    &agent_type.schema,
                    &method.output_schema,
                    output,
                )
                .unwrap_or_else(|_| empty_typed_schema_value()),
                None => empty_typed_schema_value(),
            };

            Ok(PublicAgentInvocationResult::AgentMethod(
                AgentInvocationOutputParameters { output },
            ))
        }
        AgentInvocationResult::ExternalTool { result } => Ok(
            PublicAgentInvocationResult::ExternalTool(ExternalToolResultParameters {
                result: match result {
                    Ok(result) => PublicExternalToolResult::Success(result),
                    Err(error) => PublicExternalToolResult::Failure(error),
                },
            }),
        ),
        AgentInvocationResult::ManualUpdate => {
            Ok(PublicAgentInvocationResult::ManualUpdate(Empty {}))
        }
        AgentInvocationResult::LoadSnapshot { error } => Ok(
            PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters { error }),
        ),
        AgentInvocationResult::SaveSnapshot { snapshot } => Ok(
            PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
                snapshot: raw_snapshot_to_public(snapshot),
            }),
        ),
        AgentInvocationResult::ProcessOplogEntries { error } => {
            Ok(PublicAgentInvocationResult::ProcessOplogEntries(
                ProcessOplogEntriesResultParameters { error },
            ))
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod conversion_tests;
