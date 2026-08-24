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

//! Host implementation of `golem:tool/host@0.1.0`.
//!
//! The interface is wired into the linker for every agent component. Tool
//! discovery is implemented as a durable environment-state read. Tool
//! invocation authorization is enforced here before the invocation backend,
//! which is implemented separately on top of sidecar instances.

use crate::durable_host::authorization::targets::tool_target;
use crate::durable_host::concurrent::{
    CallHandle, CallReplayOutcome, Cancellable, NotCancellable,
    authorize_live_permissions_at_serialized_access,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::secrets::secret_hold_targets_for_value;
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, InternalRetryResult, LiveAuthorizationPermit,
};
use crate::preview2::golem::tool::host::{
    Host, HostFutureInvokeResult, HostFutureInvokeResultWithStore, HostToolRpc,
    HostToolRpcWithStore, InvocationResult, RegisteredTool as WitRegisteredTool, RpcError,
    TypedSchemaValue,
};
use crate::services::environment_state::ToolDiscoveryError;
use crate::workerctx::WorkerCtx;
use anyhow::{Context, anyhow};
use golem_common::model::card::owner::ToolOwnerPattern;
use golem_common::model::oplog::host_functions::{
    GolemToolGetAllTools, GolemToolGetTool, GolemToolRpcAsyncInvokeAndAwait, GolemToolRpcInvoke,
    GolemToolRpcInvokeAndAwait,
};
use golem_common::model::oplog::payload::types::{
    SerializableToolError, SerializableToolInvocationResult, SerializableToolRpcError,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemToolGetTool, HostRequestGolemToolInvoke,
    HostRequestNoInput, HostResponseGolemToolInvokeResult, HostResponseGolemToolTool,
    HostResponseGolemToolTools, HostResponseGolemToolUnitOrFailure,
};
use golem_common::model::tool::{RegisteredTool, ToolName, ToolSource};
use golem_common::schema::render::cli_text::value_to_cli_text_unredacted;
use golem_common::schema::tool::DiscoveredTool;
use golem_common::schema::tool::canonical::CanonicalSurfaceRef;
use golem_common::schema::tool::wit::wire::{
    Host as HostToolCommon, HostUnderlyingTool, HostUnderlyingToolWithStore, Tool as WitTool,
    ToolError, UnderlyingTool,
};
use golem_common::schema::tool::{FlagShape, OptionShape, OptionSpec, Repetition, Tool};
use golem_common::schema::wit::{decode_graph, decode_value_with, encode_graph, encode_value_with};
use golem_common::schema::{
    SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue as ModelTypedSchemaValue,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::marker::PhantomData;
use std::sync::Arc;
use wasmtime::component::{Accessor, HasData, HasSelf, Linker, Resource, StreamReader};

const NOT_IMPLEMENTED: &str =
    "golem:tool/host tool invocation requires the sidecar invocation backend";
const UNDERLYING_TOOL_NOT_BOUND: &str =
    "golem:tool/common underlying-tool is not bound to a middleware chain";

struct ToolCommonHost<Ctx: WorkerCtx>(PhantomData<fn() -> Ctx>);

impl<Ctx: WorkerCtx> HasData for ToolCommonHost<Ctx> {
    type Data<'a> = &'a mut DurableWorkerCtx<Ctx>;
}

pub fn add_common_to_linker<Ctx: WorkerCtx>(
    linker: &mut Linker<Ctx>,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<()> {
    golem_common::schema::tool::wit::wire::add_to_linker::<_, ToolCommonHost<Ctx>>(linker, get)
}

/// Host-side resource table entry backing the `golem:tool/host.tool-rpc`
/// resource.
#[derive(Clone)]
pub struct ToolRpcEntry {
    tool_name: ToolName,
}

/// Host-side resource table entry backing the
/// `golem:tool/host.future-invoke-result` resource.
pub struct FutureInvokeResultEntry {
    state: FutureToolInvokeState,
}

type ToolInvokeResponse = Result<SerializableToolInvocationResult, SerializableToolRpcError>;

async fn admit_tool_response_secret_holds<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> Result<ToolInvokeResponse, WorkerExecutorError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let targets = accessor.with(|mut access| {
        let ctx = access.get();
        let value = match &response {
            Ok(result) => result.result.as_ref(),
            Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_ref() {
                SerializableToolError::CustomError(value) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        };
        match value {
            Some(value) => secret_hold_targets_for_value(ctx, value.value()),
            None => Ok(Vec::new()),
        }
    })?;
    if targets.is_empty() {
        return Ok(response);
    }
    match authorize_live_permissions_at_serialized_access(accessor, accessor.getter(), &targets)
        .await?
    {
        Ok(_) => Ok(response),
        Err(_) => Ok(Err(SerializableToolRpcError::Denied(
            "permission denied".to_string(),
        ))),
    }
}

enum FutureToolInvokeState {
    Ready(Box<Option<ToolInvokeResponse>>),
    Consumed,
    Cancelled,
}

fn render_tool_value(
    tool: &Tool,
    type_: &SchemaType,
    value: &SchemaValue,
) -> Result<String, String> {
    value_to_cli_text_unredacted(&tool.schema, type_, value).map_err(|error| error.to_string())
}

fn option_args(
    tool: &Tool,
    option: &OptionSpec,
    value: &SchemaValue,
) -> Result<Vec<String>, String> {
    let prefix = format!("--{}=", option.long);
    match &option.shape {
        OptionShape::Scalar(type_) | OptionShape::OptionalScalar(type_) => Ok(vec![format!(
            "{prefix}{}",
            render_tool_value(tool, type_, value)?
        )]),
        OptionShape::RepeatableList(shape) => {
            let SchemaValue::List { elements } = value else {
                return Err(format!(
                    "canonical tool option '{}' must contain a list",
                    option.long
                ));
            };
            let rendered = elements
                .iter()
                .map(|value| render_tool_value(tool, &shape.item_type, value))
                .collect::<Result<Vec<_>, _>>()?;
            match shape.repetition {
                Repetition::Repeated => Ok(rendered
                    .into_iter()
                    .map(|value| format!("{prefix}{value}"))
                    .collect()),
                Repetition::Delimited(separator) | Repetition::Either(separator) => Ok((!rendered
                    .is_empty())
                .then(|| format!("{prefix}{}", rendered.join(&separator.to_string())))
                .into_iter()
                .collect()),
            }
        }
        OptionShape::RepeatableMap(shape) => {
            let SchemaType::Map {
                key: key_type,
                value: value_type,
                ..
            } = tool
                .schema
                .resolve_ref(&shape.map_type)
                .map_err(|error| error.to_string())?
            else {
                return Err(format!(
                    "canonical tool option '{}' must declare a map type",
                    option.long
                ));
            };
            let SchemaValue::Map { entries } = value else {
                return Err(format!(
                    "canonical tool option '{}' must contain a map",
                    option.long
                ));
            };
            let rendered = entries
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}={}",
                        render_tool_value(tool, key_type, key)?,
                        render_tool_value(tool, value_type, value)?
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            match shape.repetition {
                Repetition::Repeated => Ok(rendered
                    .into_iter()
                    .map(|value| format!("{prefix}{value}"))
                    .collect()),
                Repetition::Delimited(separator) | Repetition::Either(separator) => Ok((!rendered
                    .is_empty())
                .then(|| format!("{prefix}{}", rendered.join(&separator.to_string())))
                .into_iter()
                .collect()),
            }
        }
    }
}

fn flag_args(
    flag: &golem_common::schema::tool::FlagSpec,
    value: &SchemaValue,
) -> Result<Vec<String>, String> {
    match (flag.shape, value) {
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(value)) if *value == shape.default => {
            Ok(Vec::new())
        }
        (FlagShape::BoolFlag(_), SchemaValue::Bool(true)) => Ok(vec![format!("--{}", flag.long)]),
        (FlagShape::BoolFlag(shape), SchemaValue::Bool(false)) if shape.negatable => {
            Ok(vec![format!("--no-{}", flag.long)])
        }
        (FlagShape::BoolFlag(_), SchemaValue::Bool(false)) => Err(format!(
            "canonical tool flag '{}' cannot represent false",
            flag.long
        )),
        (FlagShape::CountFlag(max), SchemaValue::U32(count)) => {
            if max.is_some_and(|max| *count > max) {
                return Err(format!(
                    "canonical tool flag '{}' count {} exceeds its maximum",
                    flag.long, count
                ));
            }
            Ok((0..*count).map(|_| format!("--{}", flag.long)).collect())
        }
        (FlagShape::BoolFlag(_), _) => Err(format!(
            "canonical tool flag '{}' must contain a boolean",
            flag.long
        )),
        (FlagShape::CountFlag(_), _) => Err(format!(
            "canonical tool flag '{}' must contain a u32 count",
            flag.long
        )),
    }
}

fn canonical_tool_args(
    tool: &Tool,
    command_path: &[String],
    input: &ModelTypedSchemaValue,
) -> Result<Vec<String>, String> {
    let command_index = tool
        .command_index_by_path(command_path)
        .ok_or_else(|| format!("invalid tool command path: {}", command_path.join(" ")))?;
    let values = tool
        .decode_canonical_input_record(command_index, input.value().clone())
        .map_err(|error| error.to_string())?;
    let surfaces = tool.canonical_input_surfaces(command_index);
    let body = tool.commands.nodes[command_index]
        .body
        .as_ref()
        .expect("command_index_by_path only resolves commands with bodies");
    let mut args = Vec::new();

    for (surface, field) in surfaces.into_iter().zip(values) {
        match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => args.extend(option_args(
                tool,
                &tool.commands.nodes[node].globals.options[index],
                &field.value,
            )?),
            CanonicalSurfaceRef::GlobalFlag { node, index } => args.extend(flag_args(
                &tool.commands.nodes[node].globals.flags[index],
                &field.value,
            )?),
            CanonicalSurfaceRef::BodyPositional { index } => args.push(render_tool_value(
                tool,
                &body.positionals.fixed[index].type_,
                &field.value,
            )?),
            CanonicalSurfaceRef::BodyTail => {
                let tail = body
                    .positionals
                    .tail
                    .as_ref()
                    .expect("BodyTail resolves an existing tail positional");
                let SchemaValue::List { elements } = &field.value else {
                    return Err(format!(
                        "canonical tool tail positional '{}' must contain a list",
                        tail.name
                    ));
                };
                if !elements.is_empty()
                    && let Some(separator) = &tail.separator
                {
                    args.push(separator.clone());
                }
                args.extend(
                    elements
                        .iter()
                        .map(|value| render_tool_value(tool, &tail.item_type, value))
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            CanonicalSurfaceRef::BodyOption { index } => {
                args.extend(option_args(tool, &body.options[index], &field.value)?)
            }
            CanonicalSurfaceRef::BodyFlag { index } => {
                args.extend(flag_args(&body.flags[index], &field.value)?)
            }
        }
    }

    Ok(args)
}

fn decode_typed_tool_value<Ctx: WorkerCtx>(
    value: TypedSchemaValue,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<ModelTypedSchemaValue, String> {
    let graph = decode_graph(&value.graph).map_err(|error| error.to_string());
    let decoded = decode_value_with(value.value, ctx).map_err(|error| error.to_string());
    Ok(ModelTypedSchemaValue::new(graph?, decoded?))
}

fn encode_typed_tool_value<Ctx: WorkerCtx>(
    value: &ModelTypedSchemaValue,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<TypedSchemaValue, String> {
    Ok(TypedSchemaValue {
        graph: encode_graph(value.graph()).map_err(|error| error.to_string())?,
        value: encode_value_with(value.value(), ctx).map_err(|error| error.to_string())?,
    })
}

fn project_tool_error<Ctx: WorkerCtx>(
    error: SerializableToolError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> RpcError {
    let error = match error {
        SerializableToolError::InvalidToolName(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidToolName(value)
        }
        SerializableToolError::InvalidCommandPath(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidCommandPath(value)
        }
        SerializableToolError::InvalidInput(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidInput(value)
        }
        SerializableToolError::ConstraintViolation(value) => {
            crate::preview2::golem::tool::host::ToolError::ConstraintViolation(value)
        }
        SerializableToolError::InvalidResult(value) => {
            crate::preview2::golem::tool::host::ToolError::InvalidResult(value)
        }
        SerializableToolError::CustomError(value) => match encode_typed_tool_value(&value, ctx) {
            Ok(value) => crate::preview2::golem::tool::host::ToolError::CustomError(value),
            Err(error) => return RpcError::ProtocolError(error),
        },
    };
    RpcError::RemoteToolError(error)
}

fn project_tool_rpc_error<Ctx: WorkerCtx>(
    error: SerializableToolRpcError,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> RpcError {
    match error {
        SerializableToolRpcError::ProtocolError(value) => RpcError::ProtocolError(value),
        SerializableToolRpcError::Denied(value) => RpcError::Denied(value),
        SerializableToolRpcError::NotFound(value) => RpcError::NotFound(value),
        SerializableToolRpcError::RemoteInternalError(value) => {
            RpcError::RemoteInternalError(value)
        }
        SerializableToolRpcError::RemoteToolError(error) => project_tool_error(*error, ctx),
    }
}

fn project_tool_response_value<Ctx: WorkerCtx>(
    response: ToolInvokeResponse,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(Option<TypedSchemaValue>, Option<Vec<u8>>), RpcError> {
    response
        .map_err(|error| project_tool_rpc_error(error, ctx))
        .and_then(|response| {
            let result = response
                .result
                .as_ref()
                .map(|value| encode_typed_tool_value(value, ctx))
                .transpose()
                .map_err(RpcError::ProtocolError)?;
            Ok((result, response.stdout))
        })
}

fn project_tool_response<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: ToolInvokeResponse,
) -> Result<InvocationResult, RpcError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let (result, stdout) = project_tool_response_value(response, access.get())?;
        let stdout = stdout
            .map(|bytes| StreamReader::new(&mut access, bytes))
            .transpose()
            .map_err(|error| RpcError::RemoteInternalError(error.to_string()))?;
        Ok(InvocationResult { result, stdout })
    })
}

fn project_tool_unit<Ctx: WorkerCtx>(
    response: Result<(), SerializableToolRpcError>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(), RpcError> {
    response.map_err(|error| project_tool_rpc_error(error, ctx))
}

fn tool_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    rpc: &ToolRpcEntry,
    registered_tool: &RegisteredTool,
) -> ToolOwnerPattern {
    let component = ctx.component_metadata();
    let ToolSource::Component { component_name, .. } = &registered_tool.source;
    ToolOwnerPattern::Tool {
        account: registered_tool.owner_account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component_name.clone(),
        tool: rpc.tool_name.to_string(),
    }
}

fn empty_tool_input() -> ModelTypedSchemaValue {
    ModelTypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
        SchemaValue::Tuple {
            elements: Vec::new(),
        },
    )
}

fn invocation_request(
    rpc: &ToolRpcEntry,
    command_path: &[String],
    args: Vec<String>,
    input: ModelTypedSchemaValue,
    has_stdin: bool,
) -> HostRequestGolemToolInvoke {
    HostRequestGolemToolInvoke {
        tool_name: rpc.tool_name.to_string(),
        command_path: command_path.to_vec(),
        args,
        input,
        has_stdin,
    }
}

struct PreparedToolCall {
    stdin: Option<StreamReader<u8>>,
    request: HostRequestGolemToolInvoke,
    permit: LiveAuthorizationPermit,
}

enum ToolCallPreparation {
    Ready(PreparedToolCall),
    Rejected {
        request: Box<HostRequestGolemToolInvoke>,
        response: ToolInvokeResponse,
        stdin: Option<StreamReader<u8>>,
    },
}

async fn prepare_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: &Resource<ToolRpcEntry>,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<StreamReader<u8>>,
) -> anyhow::Result<ToolCallPreparation>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let has_stdin = stdin.is_some();
    let (rpc, input, environment_state_service, environment_id, agent_type) =
        accessor.with(|mut access| {
            let ctx = access.get();
            let rpc = ctx.table().get(resource)?.clone();
            let input = decode_typed_tool_value(input, ctx);
            let agent_type = ctx
                .parsed_agent_id()
                .map(|agent_id| agent_id.agent_type)
                .ok_or_else(|| anyhow!("tool invocation requires an agent caller"))?;
            Ok::<_, anyhow::Error>((
                rpc,
                input,
                ctx.state.environment_state_service.clone(),
                ctx.state.owned_agent_id.environment_id,
                agent_type,
            ))
        })?;

    let input = match input {
        Ok(input) => input,
        Err(error) => {
            return Ok(ToolCallPreparation::Rejected {
                request: Box::new(invocation_request(
                    &rpc,
                    &command_path,
                    Vec::new(),
                    empty_tool_input(),
                    has_stdin,
                )),
                response: Err(SerializableToolRpcError::ProtocolError(format!(
                    "invalid tool input: {error}"
                ))),
                stdin,
            });
        }
    };

    let registered_tool = environment_state_service
        .get_registered_tool(environment_id, &rpc.tool_name)
        .await?
        .ok_or_else(|| {
            SerializableToolRpcError::NotFound(format!(
                "tool '{}' is not registered",
                rpc.tool_name
            ))
        });
    let registered_tool = match registered_tool {
        Ok(registered_tool) => registered_tool,
        Err(error) => {
            return Ok(ToolCallPreparation::Rejected {
                request: Box::new(invocation_request(
                    &rpc,
                    &command_path,
                    Vec::new(),
                    input,
                    has_stdin,
                )),
                response: Err(error),
                stdin,
            });
        }
    };
    let binding = environment_state_service
        .get_agent_tool_binding(environment_id, &agent_type, &rpc.tool_name)
        .await?
        .ok_or_else(|| {
            SerializableToolRpcError::Denied(format!(
                "tool '{}' is not bound to agent type '{agent_type}'",
                rpc.tool_name
            ))
        });
    let binding = match binding {
        Ok(binding) => binding,
        Err(error) => {
            return Ok(ToolCallPreparation::Rejected {
                request: Box::new(invocation_request(
                    &rpc,
                    &command_path,
                    Vec::new(),
                    input,
                    has_stdin,
                )),
                response: Err(error),
                stdin,
            });
        }
    };
    if registered_tool.deployment_revision != binding.deployment_revision {
        return Ok(ToolCallPreparation::Rejected {
            request: Box::new(invocation_request(
                &rpc,
                &command_path,
                Vec::new(),
                input,
                has_stdin,
            )),
            response: Err(SerializableToolRpcError::RemoteInternalError(format!(
                "tool '{}' changed while resolving its binding",
                rpc.tool_name
            ))),
            stdin,
        });
    }

    let args = match canonical_tool_args(&registered_tool.definition, &command_path, &input) {
        Ok(args) => args,
        Err(error) => {
            return Ok(ToolCallPreparation::Rejected {
                request: Box::new(invocation_request(
                    &rpc,
                    &command_path,
                    Vec::new(),
                    input,
                    has_stdin,
                )),
                response: Err(SerializableToolRpcError::ProtocolError(error)),
                stdin,
            });
        }
    };
    let request = invocation_request(&rpc, &command_path, args.clone(), input.clone(), has_stdin);

    let target = accessor.with(|mut access| {
        let owner = tool_owner(access.get(), &rpc, &registered_tool);
        let command_path = command_path.iter().map(String::as_str).collect::<Vec<_>>();
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        tool_target(owner, &command_path, &args)
    });
    let target = match target {
        Ok(target) => target,
        Err(error) => {
            return Ok(ToolCallPreparation::Rejected {
                request: Box::new(request),
                response: Err(SerializableToolRpcError::ProtocolError(error.to_string())),
                stdin,
            });
        }
    };
    let permit = match authorize_live_permissions_at_serialized_access(
        accessor,
        accessor.getter(),
        &[target],
    )
    .await?
    {
        Ok(permit) => permit,
        Err(error) => {
            return Ok(ToolCallPreparation::Rejected {
                request: Box::new(request),
                response: Err(SerializableToolRpcError::Denied(error.to_string())),
                stdin,
            });
        }
    };

    Ok(ToolCallPreparation::Ready(PreparedToolCall {
        stdin,
        request,
        permit,
    }))
}

async fn close_stdin<U, D>(
    accessor: &Accessor<U, D>,
    mut stdin: Option<StreamReader<u8>>,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    D: wasmtime::component::HasData + ?Sized,
{
    if let Some(stdin) = &mut stdin {
        accessor.with(|mut access| stdin.close(&mut access))?;
    }
    Ok(())
}

impl TryFrom<&DiscoveredTool> for WitRegisteredTool {
    type Error = anyhow::Error;

    fn try_from(value: &DiscoveredTool) -> Result<Self, Self::Error> {
        let name = value.definition.name().unwrap_or("<unnamed tool>");
        let definition = WitTool::try_from(&value.definition).with_context(|| {
            format!(
                "failed to encode discovered tool '{name}' implemented by component {}",
                value.implemented_by
            )
        })?;

        Ok(Self {
            definition,
            implemented_by: value.implemented_by.into(),
        })
    }
}

fn classify_tool_discovery_error(error: &ToolDiscoveryError) -> HostFailureKind {
    match error {
        ToolDiscoveryError::Retrieval(_) => HostFailureKind::Transient,
        ToolDiscoveryError::AgentContextRequired
        | ToolDiscoveryError::InconsistentSnapshot { .. } => HostFailureKind::Permanent,
    }
}

fn terminal_tool_discovery_error(message: String) -> anyhow::Error {
    anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message,
    })
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) async fn get_all_tools_model(&mut self) -> anyhow::Result<Vec<Arc<DiscoveredTool>>> {
        let agent_type = self.parsed_agent_id().map(|agent_id| agent_id.agent_type);
        let environment_id = self.state.owned_agent_id.environment_id;
        let component_id = self.state.owned_agent_id.agent_id.component_id;
        let component_revision = self.state.component_metadata.revision;

        let mut handle = CallHandle::<GolemToolGetAllTools, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = if let Some(agent_type) = &agent_type {
                loop {
                    let result = self
                        .state
                        .environment_state_service
                        .get_accessible_tools(
                            environment_id,
                            component_id,
                            component_revision,
                            agent_type,
                        )
                        .await;
                    match handle
                        .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                        .await?
                    {
                        InternalRetryResult::Persist => break result,
                        InternalRetryResult::RetryInternally => continue,
                    }
                }
            } else {
                Err(ToolDiscoveryError::AgentContextRequired)
            };

            handle
                .complete(
                    self,
                    HostResponseGolemToolTools {
                        result: result.map_err(|error| error.to_string()),
                    },
                )
                .await?
        };

        response.result.map_err(|error| {
            let agent_type = agent_type
                .as_ref()
                .map_or("<missing>", |agent_type| agent_type.0.as_str());
            terminal_tool_discovery_error(format!(
                "failed to discover tools for agent type '{}' in environment '{environment_id}': {error}",
                agent_type
            ))
        })
    }

    pub(crate) async fn get_tool_model(
        &mut self,
        tool_name: String,
    ) -> anyhow::Result<Option<Arc<DiscoveredTool>>> {
        let agent_type = self.parsed_agent_id().map(|agent_id| agent_id.agent_type);
        let valid_tool_name = ToolName::try_from(tool_name.as_str()).ok();
        let environment_id = self.state.owned_agent_id.environment_id;
        let component_id = self.state.owned_agent_id.agent_id.component_id;
        let component_revision = self.state.component_metadata.revision;

        let mut handle = CallHandle::<GolemToolGetTool, NotCancellable>::start(
            self,
            HostRequestGolemToolGetTool {
                name: tool_name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = if let Some(agent_type) = &agent_type {
                if let Some(valid_tool_name) = &valid_tool_name {
                    loop {
                        let result = self
                            .state
                            .environment_state_service
                            .get_accessible_tool(
                                environment_id,
                                component_id,
                                component_revision,
                                agent_type,
                                valid_tool_name,
                            )
                            .await;
                        match handle
                            .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                            .await?
                        {
                            InternalRetryResult::Persist => break result,
                            InternalRetryResult::RetryInternally => continue,
                        }
                    }
                } else {
                    Ok(None)
                }
            } else {
                Err(ToolDiscoveryError::AgentContextRequired)
            };

            handle
                .complete(
                    self,
                    HostResponseGolemToolTool {
                        result: result.map_err(|error| error.to_string()),
                    },
                )
                .await?
        };

        response.result.map_err(|error| {
            let agent_type = agent_type
                .as_ref()
                .map_or("<missing>", |agent_type| agent_type.0.as_str());
            terminal_tool_discovery_error(format!(
                "failed to discover tool '{}' for agent type '{}' in environment '{environment_id}': {error}",
                tool_name, agent_type
            ))
        })
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_tools(&mut self) -> anyhow::Result<Vec<WitRegisteredTool>> {
        self.get_all_tools_model()
            .await?
            .iter()
            .map(|tool| WitRegisteredTool::try_from(tool.as_ref()))
            .collect()
    }

    async fn get_tool(&mut self, name: String) -> anyhow::Result<Option<WitRegisteredTool>> {
        self.get_tool_model(name)
            .await?
            .as_ref()
            .map(|tool| WitRegisteredTool::try_from(tool.as_ref()))
            .transpose()
    }
}

impl<Ctx: WorkerCtx> HostUnderlyingTool for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<UnderlyingTool>) -> anyhow::Result<()> {
        let _ = self.table().delete(rep);
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostToolCommon for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> HostUnderlyingToolWithStore<U> for ToolCommonHost<Ctx> {
    async fn invoke(
        _accessor: &Accessor<U, Self>,
        _self_: Resource<UnderlyingTool>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Result<InvocationResult, ToolError>> {
        Err(anyhow!(UNDERLYING_TOOL_NOT_BOUND))
    }
}

impl<Ctx: WorkerCtx> HostToolRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, tool_name: String) -> anyhow::Result<Resource<ToolRpcEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "new");
        let tool_name = ToolName::try_from(tool_name).map_err(|error| anyhow!(error))?;
        Ok(self.table().push(ToolRpcEntry { tool_name })?)
    }

    async fn drop(&mut self, rep: Resource<ToolRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::tool-rpc", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolRpcWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn invoke(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Result<(), RpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke");
        });
        let replaying = accessor.with(|mut access| !access.get().state.is_live());
        let handle = if replaying {
            let handle = CallHandle::<GolemToolRpcInvoke, Cancellable>::start_access(
                accessor,
                accessor.getter(),
                HostRequestGolemToolInvoke {
                    tool_name: String::new(),
                    command_path: Vec::new(),
                    args: Vec::new(),
                    input: empty_tool_input(),
                    has_stdin: false,
                },
                DurableFunctionType::ReadRemote,
            )
            .await?;
            match handle.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => {
                    close_stdin(accessor, stdin).await?;
                    return Ok(accessor
                        .with(|mut access| project_tool_unit(response.result, access.get())));
                }
                CallReplayOutcome::Incomplete(live) => Some(live),
            }
        } else {
            None
        };

        if let Some(handle) = handle {
            close_stdin(accessor, stdin).await?;
            let response = handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    HostResponseGolemToolUnitOrFailure {
                        result: Err(SerializableToolRpcError::RemoteInternalError(
                            NOT_IMPLEMENTED.to_string(),
                        )),
                    },
                )
                .await?;
            return Ok(accessor.with(|mut access| project_tool_unit(response.result, access.get())));
        }

        let preparation = prepare_tool_call(accessor, &self_, command_path, input, stdin).await?;
        let prepared = match preparation {
            ToolCallPreparation::Rejected {
                request,
                response,
                stdin,
            } => {
                close_stdin(accessor, stdin).await?;
                let handle = CallHandle::<GolemToolRpcInvoke, Cancellable>::start_access(
                    accessor,
                    accessor.getter(),
                    *request,
                    DurableFunctionType::ReadRemote,
                )
                .await?;
                let response = handle
                    .complete_access(
                        accessor,
                        accessor.getter(),
                        HostResponseGolemToolUnitOrFailure {
                            result: response.map(|_| ()),
                        },
                    )
                    .await?;
                return Ok(
                    accessor.with(|mut access| project_tool_unit(response.result, access.get()))
                );
            }
            ToolCallPreparation::Ready(prepared) => prepared,
        };

        let handle = CallHandle::<GolemToolRpcInvoke, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            prepared.request.clone(),
            DurableFunctionType::ReadRemote,
        )
        .await?;
        close_stdin(accessor, prepared.stdin).await?;
        let _permit = prepared.permit;
        let result = Err(SerializableToolRpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        ));
        let response = handle
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseGolemToolUnitOrFailure { result },
            )
            .await?;
        Ok(accessor.with(|mut access| project_tool_unit(response.result, access.get())))
    }

    async fn invoke_and_await(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke-and-await");
        });
        let replaying = accessor.with(|mut access| !access.get().state.is_live());
        let handle = if replaying {
            let mut handle = CallHandle::<GolemToolRpcInvokeAndAwait, Cancellable>::start_access(
                accessor,
                accessor.getter(),
                HostRequestGolemToolInvoke {
                    tool_name: String::new(),
                    command_path: Vec::new(),
                    args: Vec::new(),
                    input: empty_tool_input(),
                    has_stdin: false,
                },
                DurableFunctionType::ReadRemote,
            )
            .await?;
            match handle.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => {
                    close_stdin(accessor, stdin).await?;
                    return Ok(project_tool_response(accessor, response.result));
                }
                CallReplayOutcome::Incomplete(live) => {
                    handle = live;
                    Some(handle)
                }
            }
        } else {
            None
        };

        if let Some(handle) = handle {
            close_stdin(accessor, stdin).await?;
            let result = admit_tool_response_secret_holds(
                accessor,
                Err(SerializableToolRpcError::RemoteInternalError(
                    NOT_IMPLEMENTED.to_string(),
                )),
            )
            .await?;
            let response = handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    HostResponseGolemToolInvokeResult { result },
                )
                .await?;
            return Ok(project_tool_response(accessor, response.result));
        }

        let preparation = prepare_tool_call(accessor, &self_, command_path, input, stdin).await?;
        let prepared = match preparation {
            ToolCallPreparation::Rejected {
                request,
                response,
                stdin,
            } => {
                close_stdin(accessor, stdin).await?;
                let handle = CallHandle::<GolemToolRpcInvokeAndAwait, Cancellable>::start_access(
                    accessor,
                    accessor.getter(),
                    *request,
                    DurableFunctionType::ReadRemote,
                )
                .await?;
                let response = admit_tool_response_secret_holds(accessor, response).await?;
                let response = handle
                    .complete_access(
                        accessor,
                        accessor.getter(),
                        HostResponseGolemToolInvokeResult { result: response },
                    )
                    .await?;
                return Ok(project_tool_response(accessor, response.result));
            }
            ToolCallPreparation::Ready(prepared) => prepared,
        };

        let handle = CallHandle::<GolemToolRpcInvokeAndAwait, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            prepared.request.clone(),
            DurableFunctionType::ReadRemote,
        )
        .await?;
        close_stdin(accessor, prepared.stdin).await?;
        let _permit = prepared.permit;
        let result = Err(SerializableToolRpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        ));
        let result = admit_tool_response_secret_holds(accessor, result).await?;
        let response = handle
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseGolemToolInvokeResult { result },
            )
            .await?;
        Ok(project_tool_response(accessor, response.result))
    }

    async fn async_invoke_and_await(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Resource<FutureInvokeResultEntry>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "async-invoke-and-await");
        });
        let replaying = accessor.with(|mut access| !access.get().state.is_live());
        let handle = if replaying {
            let handle = CallHandle::<GolemToolRpcAsyncInvokeAndAwait, Cancellable>::start_access(
                accessor,
                accessor.getter(),
                HostRequestGolemToolInvoke {
                    tool_name: String::new(),
                    command_path: Vec::new(),
                    args: Vec::new(),
                    input: empty_tool_input(),
                    has_stdin: false,
                },
                DurableFunctionType::ReadRemote,
            )
            .await?;
            match handle.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => {
                    close_stdin(accessor, stdin).await?;
                    return accessor.with(|mut access| {
                        Ok(access.get().table().push(FutureInvokeResultEntry {
                            state: FutureToolInvokeState::Ready(Box::new(Some(response.result))),
                        })?)
                    });
                }
                CallReplayOutcome::Incomplete(live) => Some(live),
            }
        } else {
            None
        };

        if let Some(handle) = handle {
            close_stdin(accessor, stdin).await?;
            let result = admit_tool_response_secret_holds(
                accessor,
                Err(SerializableToolRpcError::RemoteInternalError(
                    NOT_IMPLEMENTED.to_string(),
                )),
            )
            .await?;
            let response = handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    HostResponseGolemToolInvokeResult { result },
                )
                .await?;
            return accessor.with(|mut access| {
                Ok(access.get().table().push(FutureInvokeResultEntry {
                    state: FutureToolInvokeState::Ready(Box::new(Some(response.result))),
                })?)
            });
        }

        let preparation = prepare_tool_call(accessor, &self_, command_path, input, stdin).await?;
        let prepared = match preparation {
            ToolCallPreparation::Rejected {
                request,
                response,
                stdin,
            } => {
                close_stdin(accessor, stdin).await?;
                let handle =
                    CallHandle::<GolemToolRpcAsyncInvokeAndAwait, Cancellable>::start_access(
                        accessor,
                        accessor.getter(),
                        *request,
                        DurableFunctionType::ReadRemote,
                    )
                    .await?;
                let response = admit_tool_response_secret_holds(accessor, response).await?;
                let response = handle
                    .complete_access(
                        accessor,
                        accessor.getter(),
                        HostResponseGolemToolInvokeResult { result: response },
                    )
                    .await?;
                return accessor.with(|mut access| {
                    Ok(access.get().table().push(FutureInvokeResultEntry {
                        state: FutureToolInvokeState::Ready(Box::new(Some(response.result))),
                    })?)
                });
            }
            ToolCallPreparation::Ready(prepared) => prepared,
        };
        let handle = CallHandle::<GolemToolRpcAsyncInvokeAndAwait, Cancellable>::start_access(
            accessor,
            accessor.getter(),
            prepared.request.clone(),
            DurableFunctionType::ReadRemote,
        )
        .await?;
        close_stdin(accessor, prepared.stdin).await?;
        let _permit = prepared.permit;
        let result = admit_tool_response_secret_holds(
            accessor,
            Err(SerializableToolRpcError::RemoteInternalError(
                NOT_IMPLEMENTED.to_string(),
            )),
        )
        .await?;
        let response = handle
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseGolemToolInvokeResult { result },
            )
            .await?;
        let state = FutureToolInvokeState::Ready(Box::new(Some(response.result)));
        accessor.with(|mut access| {
            Ok(access
                .get()
                .table()
                .push(FutureInvokeResultEntry { state })?)
        })
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureInvokeResultEntry>,
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::future-invoke-result", "get");
        });
        let response = accessor.with(|mut access| -> anyhow::Result<Result<_, RpcError>> {
            let entry = access.get().table().get_mut(&self_)?;
            let old = std::mem::replace(&mut entry.state, FutureToolInvokeState::Consumed);
            Ok(match old {
                FutureToolInvokeState::Ready(mut response) => match response.take() {
                    Some(response) => Ok(response),
                    None => Err(RpcError::ProtocolError(
                        "tool invocation future has already been consumed".to_string(),
                    )),
                },
                FutureToolInvokeState::Consumed => Err(RpcError::ProtocolError(
                    "tool invocation future has already been consumed".to_string(),
                )),
                FutureToolInvokeState::Cancelled => {
                    entry.state = FutureToolInvokeState::Cancelled;
                    Err(RpcError::ProtocolError(
                        "tool invocation future was cancelled".to_string(),
                    ))
                }
            })
        })?;
        Ok(response.and_then(|response| project_tool_response(accessor, response)))
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, self_: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "cancel");
        self.table().get_mut(&self_)?.state = FutureToolInvokeState::Cancelled;
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{WitRegisteredTool, classify_tool_discovery_error, terminal_tool_discovery_error};
    use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
    use crate::services::environment_state::ToolDiscoveryError;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::tool::{RegisteredTool, ToolProvisionConfig, ToolSource};
    use golem_common::schema::tool::{
        CommandBody, CommandNode, CommandTree, DiscoveredTool, Doc, Globals, Positional,
        Positionals, Tool,
    };
    use golem_common::schema::{MetadataEnvelope, SchemaGraph, SchemaType, SchemaTypeDef, TypeId};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use test_r::test;

    fn registered_tool() -> (RegisteredTool, ComponentId) {
        let component_id = ComponentId::new();
        let definition = Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "search".to_string(),
                    aliases: vec!["find".to_string()],
                    doc: Doc {
                        summary: "Search documents".to_string(),
                        description: "Searches indexed documents".to_string(),
                        examples: Vec::new(),
                    },
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: Some(CommandBody {
                        positionals: Positionals {
                            fixed: vec![Positional {
                                name: "query".to_string(),
                                doc: Doc::default(),
                                value_name: Some("QUERY".to_string()),
                                type_: SchemaType::string().with_metadata(MetadataEnvelope {
                                    doc: Some("Text to search for".to_string()),
                                    aliases: vec!["term".to_string()],
                                    ..Default::default()
                                }),
                                default: None,
                                required: true,
                                accepts_stdio: false,
                            }],
                            tail: None,
                        },
                        options: Vec::new(),
                        flags: Vec::new(),
                        constraints: Vec::new(),
                        stdin: None,
                        stdout: None,
                        result: None,
                        errors: Vec::new(),
                        annotations: None,
                    }),
                }],
            },
            schema: SchemaGraph::empty(),
        };

        (
            RegisteredTool {
                deployment_revision: DeploymentRevision::try_from(1_u64).unwrap(),
                definition,
                provision: ToolProvisionConfig::default(),
                source: ToolSource::Component {
                    component_id,
                    component_revision: ComponentRevision::try_from(7_u64).unwrap(),
                    component_name: ComponentName("search-tools".to_string()),
                },
                owner_account_id: AccountId::new(),
                owner_account_email: AccountEmail::new("owner@example.com"),
                metadata_version: "0.1.0".to_string(),
            },
            component_id,
        )
    }

    #[test]
    fn registered_component_tool_converts_to_discovery_wit_record() {
        let (registered, component_id) = registered_tool();
        let expected_definition = registered.definition.clone();

        let discovered = DiscoveredTool::from(registered);
        assert_eq!(discovered.definition, expected_definition);
        assert_eq!(discovered.implemented_by, component_id);

        let wit = WitRegisteredTool::try_from(&discovered).unwrap();
        assert_eq!(
            Tool::try_from(&wit.definition).unwrap(),
            expected_definition
        );
        assert_eq!(ComponentId::from(wit.implemented_by), component_id);
    }

    #[test]
    fn tool_discovery_error_classification_preserves_integrity_semantics() {
        let retrieval = ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime("offline"));
        let agent_context_required = ToolDiscoveryError::AgentContextRequired;
        let inconsistent = ToolDiscoveryError::InconsistentSnapshot {
            details: "dangling binding".to_string(),
        };

        assert_eq!(
            classify_tool_discovery_error(&retrieval),
            HostFailureKind::Transient
        );
        assert_eq!(
            classify_tool_discovery_error(&agent_context_required),
            HostFailureKind::Permanent
        );
        assert_eq!(
            classify_tool_discovery_error(&inconsistent),
            HostFailureKind::Permanent
        );
    }

    #[test]
    fn terminal_tool_discovery_errors_remain_permanent() {
        let error = terminal_tool_discovery_error("discovery failed".to_string());
        let classified = error.downcast_ref::<ClassifiedHostError>().unwrap();

        assert_eq!(classified.kind, HostFailureKind::Permanent);
        assert_eq!(classified.message, "discovery failed");
    }

    #[test]
    fn discovered_tool_wit_encoding_error_has_tool_and_source_context() {
        let (registered, component_id) = registered_tool();
        let mut discovered = DiscoveredTool::from(registered);
        let duplicate_id = TypeId::new("duplicate");
        let duplicate_definition = SchemaTypeDef {
            id: duplicate_id,
            name: Some("Duplicate".to_string()),
            body: SchemaType::string(),
        };
        discovered.definition.schema.defs =
            vec![duplicate_definition.clone(), duplicate_definition];

        let error = match WitRegisteredTool::try_from(&discovered) {
            Ok(_) => panic!("duplicate schema definitions must fail WIT encoding"),
            Err(error) => error,
        };
        let message = format!("{error:#}");

        assert!(message.contains("discovered tool 'search'"));
        assert!(message.contains(&component_id.to_string()));
        assert!(message.contains("duplicate type id: duplicate"));
    }
}
