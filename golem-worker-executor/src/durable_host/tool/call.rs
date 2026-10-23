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

use super::{
    AcceptedToolCall, Accessor, AccountEmail, AgentEntity, ApplicationName, Arc, AtomicBool,
    ClassifiedHostError, ComponentName, DurableWorkerCtx, EntityCallMode,
    EntityInvocationDescriptor, EntityInvocationDurability, EntityInvocationKeyContext,
    EntityInvocationPlanReference, EnvironmentName, GolemSpecificWasmTrap,
    GuestCallSettlementError, HasSelf, HostFailureKind, HostRequestGolemToolInvocationRejected,
    ModelTypedSchemaValue, NamedToolErrorSchema, NativeToolTask, Ordering, PreparedToolCall,
    Principal, ResolvedOwnerContext, Resource, SchemaType, SerializableToolError,
    SerializableToolRpcError, StoreContextMut, Tool, ToolActivationOutcome, ToolActivationSnapshot,
    ToolAttachmentCounterparty, ToolBindingOwner, ToolCallDispatch, ToolCallPreparation,
    ToolCallStdin, ToolCallTarget, ToolDiscoveryError, ToolInputDecodeFailure,
    ToolInvocationAttempt, ToolInvocationDescriptor, ToolInvocationReplayOutcome,
    ToolInvokeResponse, ToolName, ToolOutputContract, ToolOutputEntry, ToolOwnerPattern,
    ToolRpcEntry, ToolRpcOwnerContext, ToolStdinEntry, TypedSchemaValue, UnderlyingToolEntry,
    WorkerCtx, WorkerExecutorError, admit_tool_response_secret_holds, anyhow,
    authorize_live_permissions_at_serialized_access, classify_tool_discovery_error,
    cleanup_failed_tool_dispatch, close_stdin, decode_tool_terminal, decode_typed_tool_value,
    durability, join_or_park_at_runtime_shutdown, mcp, observe_tool_invoke_plans, oneshot,
    operation, record_tool_rejection_access, reject_output, resolve_tool_command,
    run_guest_call_settled, spawn_tool_execution, tool_target, validate_stream_attachments,
};

impl UnderlyingToolEntry {
    pub(crate) fn new(
        position: crate::durable_host::entity::ResolvedEntityInvocationPosition,
    ) -> Self {
        Self {
            position,
            revoked: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    pub(super) fn next_definition(&self) -> anyhow::Result<&Tool> {
        match self.position.layer() {
            golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
                next_effective_definition,
                ..
            } => Ok(next_effective_definition),
            golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => {
                Err(anyhow!("the terminal tool layer has no underlying tool"))
            }
        }
    }

    pub(super) fn admit(&self) -> anyhow::Result<()> {
        (!self.revoked.load(Ordering::Acquire))
            .then_some(())
            .ok_or_else(|| anyhow!("underlying-tool capability is no longer active"))
    }
}

pub(super) fn caller_tool_owner(
    account: &AccountEmail,
    application: &ApplicationName,
    environment: &EnvironmentName,
    component: &ComponentName,
    tool_name: &ToolName,
) -> ToolOwnerPattern {
    ToolOwnerPattern::Tool {
        account: account.clone(),
        application: application.clone(),
        environment: environment.clone(),
        component: component.clone(),
        tool: tool_name.to_string(),
    }
}

impl ToolCallTarget {
    pub(super) fn tool_name(&self) -> anyhow::Result<ToolName> {
        match self {
            Self::Ambient(rpc) => Ok(rpc.tool_name.clone()),
            Self::Underlying(underlying) => match underlying
                .position
                .plan()
                .layer((underlying.position.plan().len() - 1) as u32)
                .map_err(anyhow::Error::msg)?
                .activation()
                .entity()
            {
                AgentEntity::Tool(name) => Ok(name),
                AgentEntity::ToolMiddleware(_) => {
                    Err(anyhow!("middleware plan has no terminal tool"))
                }
            },
        }
    }
}

pub(super) fn tool_rpc_for_current_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    tool_name: ToolName,
) -> anyhow::Result<ToolRpcEntry> {
    Ok(ToolRpcEntry {
        tool_name,
        owner: ToolRpcOwnerContext {
            owner_id: ctx.state.owned_agent_id.clone(),
        },
    })
}

pub(super) fn tool_rpc_resource<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: &Resource<ToolRpcEntry>,
) -> anyhow::Result<ToolRpcEntry>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| Ok(access.get().table().get(resource)?.clone()))
}

pub(super) fn read_tool_attempt<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    target: ToolCallTarget,
    input: TypedSchemaValue,
) -> anyhow::Result<ToolInvocationAttempt>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| {
        let ctx = access.get();
        if let ToolCallTarget::Ambient(rpc) = &target
            && ctx.state.owned_agent_id != rpc.owner.owner_id
        {
            return Err(anyhow!(
                "tool RPC resource belongs to a different owner runtime"
            ));
        }
        if let ToolCallTarget::Underlying(underlying) = &target {
            underlying.admit()?;
            underlying
                .position
                .next_reference()
                .ok_or_else(|| anyhow!("underlying-tool has no next chain layer"))?;
        }
        let (parent, attempt_ordinal) = next_tool_attempt_ordinal(ctx)?;
        let key_context = EntityInvocationKeyContext::capture(ctx, attempt_ordinal)?;
        Ok(ToolInvocationAttempt {
            target,
            input: decode_typed_tool_value(input, ctx),
            rejection: None,
            key_context,
            parent,
            attempt_ordinal,
        })
    })
}

pub(super) fn next_tool_attempt_ordinal<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<(crate::worker::owner_lane::OwnerInvocationId, u64)> {
    let parent = ctx.owner_invocation_id()?;
    let next = ctx
        .state
        .tool_invocation_attempt_ordinals
        .entry(parent.clone())
        .or_default();
    let ordinal = *next;
    *next = next
        .checked_add(1)
        .ok_or_else(|| anyhow!("tool invocation attempt ordinal overflow"))?;
    Ok((parent, ordinal))
}

pub(super) fn describe_tool_command(
    definition: &Tool,
    input: &ModelTypedSchemaValue,
    attempt_ordinal: u64,
    command_path: Vec<String>,
    has_stdin: bool,
    has_stdout: bool,
    has_stderr: bool,
    call_mode: EntityCallMode,
) -> Result<ToolInvocationDescriptor, SerializableToolRpcError> {
    let command = resolve_tool_command(definition, &command_path, input)
        .map_err(|error| SerializableToolRpcError::RemoteToolError(Box::new(error)))?;
    validate_stream_attachments(
        &command,
        &command_path,
        has_stdin,
        has_stdout,
        has_stderr,
        call_mode,
    )?;
    let declares_stdout = command.stdout_required.is_some();
    let declares_stderr = command.stderr_required.is_some();
    let body = definition.commands.nodes[command.command_index]
        .body
        .as_ref()
        .expect("a resolved command has a body");
    Ok(ToolInvocationDescriptor {
        attempt_ordinal,
        command_path,
        args: command.args,
        has_stdin,
        has_stdout,
        declares_stdout,
        has_stderr,
        declares_stderr,
        output_contract: ToolOutputContract {
            result: body
                .result
                .as_ref()
                .map(|result| golem_common::schema::SchemaGraph {
                    defs: definition.schema.defs.clone(),
                    root: result.type_.clone(),
                }),
            errors: body
                .errors
                .iter()
                .map(|error| NamedToolErrorSchema {
                    name: error.name.clone(),
                    payload: golem_common::schema::SchemaGraph {
                        defs: definition.schema.defs.clone(),
                        root: error
                            .payload
                            .clone()
                            .unwrap_or_else(|| SchemaType::tuple(Vec::new())),
                    },
                })
                .collect(),
        },
    })
}

pub(super) fn rejected_tool_call(
    tool_name: &ToolName,
    attempt_ordinal: u64,
    command_path: &[String],
    input: Option<ModelTypedSchemaValue>,
    input_decode_failure: Option<ToolInputDecodeFailure>,
    has_stdin: bool,
    has_stdout: bool,
    has_stderr: bool,
    call_mode: EntityCallMode,
    error: SerializableToolRpcError,
    stdin: Option<ToolCallStdin>,
) -> ToolCallPreparation {
    ToolCallPreparation::Rejected {
        request: Box::new(HostRequestGolemToolInvocationRejected {
            attempt_ordinal,
            tool_name: tool_name.to_string(),
            command_path: command_path.to_vec(),
            input,
            input_decode_failure,
            has_stdin,
            has_stdout,
            has_stderr,
            call_mode,
            error,
        }),
        stdin,
    }
}

pub(super) async fn resolve_tool_activation<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    rpc: &ToolRpcEntry,
    binding_owner: &ToolBindingOwner,
) -> Result<ToolActivationOutcome, ToolDiscoveryError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    use crate::services::environment_state::{
        get_tool_activation_from_deployment, tool_activation_from_mcp,
    };
    use golem_common::model::mcp_import::McpImportSource;

    crate::durable_host::call_coordinator::process_pending_replay_events_access(
        accessor,
        accessor.getter(),
    )
    .await
    .map_err(ToolDiscoveryError::Retrieval)?;
    let (service, owner) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.state.environment_state_service.clone(),
            ctx.owner_component_metadata().clone(),
        )
    });
    let deployment = service
        .get_live_tool_deployment_state(rpc.owner.owner_id.environment_id, owner.id, owner.revision)
        .await?;
    let fixed =
        get_tool_activation_from_deployment(deployment.as_deref(), binding_owner, &rpc.tool_name)?;
    if !matches!(fixed, ToolActivationOutcome::NotRegistered) {
        return Ok(fixed);
    }
    let Some(deployment) = deployment.filter(|deployment| !deployment.mcp_imports.is_empty())
    else {
        return Ok(fixed);
    };
    let auth = crate::durable_host::call_coordinator::agent_auth_ctx_at_serialized_access(
        accessor,
        accessor.getter(),
    )
    .await?;
    for index in 0..deployment.mcp_imports.len() {
        let source = McpImportSource {
            environment_id: rpc.owner.owner_id.environment_id,
            deployment_revision: deployment.deployment_revision,
            import_index: index.try_into().map_err(|_| {
                ToolDiscoveryError::InconsistentSnapshot {
                    details: "too many MCP imports".into(),
                }
            })?,
            upstream_tool_name: String::new(),
        };
        let observation = service
            .resolve_mcp_import(&source, &auth, false)
            .await
            .map_err(ToolDiscoveryError::Mcp)?;
        if let Some(tool) = observation
            .tools
            .into_iter()
            .find(|tool| tool.definition.name() == Some(rpc.tool_name.as_str()))
        {
            let binding_owner = binding_owner.clone();
            let deployment = deployment.clone();
            return join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
                tool_activation_from_mcp(
                    source,
                    observation.protocol_version,
                    tool,
                    &deployment,
                    &owner,
                    &binding_owner,
                )
                .map(|activation| ToolActivationOutcome::Ready(Box::new(activation)))
            }))
            .await
            .map_err(|error| {
                ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime(error.to_string()))
            })?;
        }
    }
    Ok(ToolActivationOutcome::NotRegistered)
}

pub(super) async fn prepare_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    attempt: ToolInvocationAttempt,
    command_path: Vec<String>,
    stdin: Option<ToolCallStdin>,
    stdout_requested: bool,
    stderr_requested: bool,
    call_mode: EntityCallMode,
    pinned_activation: Option<Arc<ToolActivationSnapshot>>,
    principal: Option<Principal>,
) -> anyhow::Result<ToolCallPreparation>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    crate::durable_host::call_coordinator::process_pending_replay_events_access(
        accessor,
        accessor.getter(),
    )
    .await?;
    let has_stdin = stdin.is_some();
    let ToolInvocationAttempt {
        target,
        input,
        rejection,
        key_context: _,
        parent,
        attempt_ordinal,
    } = attempt;
    let tool_name = target.tool_name()?;
    let input = match input {
        Ok(input) => input,
        Err(error) => {
            return Ok(rejected_tool_call(
                &tool_name,
                attempt_ordinal,
                &command_path,
                None,
                Some(error),
                has_stdin,
                stdout_requested,
                stderr_requested,
                call_mode,
                SerializableToolRpcError::RemoteToolError(Box::new(
                    SerializableToolError::InvalidInput(match error {
                        ToolInputDecodeFailure::InvalidSchemaGraph => {
                            "invalid typed input schema graph".to_string()
                        }
                        ToolInputDecodeFailure::InvalidSchemaValue => {
                            "invalid typed input schema value".to_string()
                        }
                    }),
                )),
                stdin,
            ));
        }
    };

    if let Some(error) = rejection {
        return Ok(rejected_tool_call(
            &tool_name,
            attempt_ordinal,
            &command_path,
            Some(input),
            None,
            has_stdin,
            stdout_requested,
            stderr_requested,
            call_mode,
            error,
            stdin,
        ));
    }

    let (effective_definition, activation, plan) = match &target {
        ToolCallTarget::Ambient(rpc) => {
            let binding_owner = accessor.with(|mut access| {
                let state = access.get();
                let component = state.owner_component_metadata();
                match state.owner_context() {
                    ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                        agent_type_name: agent.agent_type.clone(),
                    },
                    ResolvedOwnerContext::ComponentWorker
                    | ResolvedOwnerContext::ComponentBaseline => {
                        ToolBindingOwner::ComponentBaseline {
                            component_id: component.id,
                        }
                    }
                }
            });
            let activation_snapshot = if let Some(activation) = pinned_activation {
                (*activation).clone()
            } else {
                match resolve_tool_activation(accessor, rpc, &binding_owner).await {
                    Ok(ToolActivationOutcome::Ready(activation)) => *activation,
                    Ok(ToolActivationOutcome::NotBound) => {
                        return Ok(rejected_tool_call(
                            &tool_name,
                            attempt_ordinal,
                            &command_path,
                            Some(input),
                            None,
                            has_stdin,
                            stdout_requested,
                            stderr_requested,
                            call_mode,
                            SerializableToolRpcError::Denied(format!(
                                "tool '{}' is not bound to owner '{binding_owner:?}'",
                                rpc.tool_name
                            )),
                            stdin,
                        ));
                    }
                    Ok(ToolActivationOutcome::NotRegistered) => {
                        return Ok(rejected_tool_call(
                            &tool_name,
                            attempt_ordinal,
                            &command_path,
                            Some(input),
                            None,
                            has_stdin,
                            stdout_requested,
                            stderr_requested,
                            call_mode,
                            SerializableToolRpcError::NotFound(format!(
                                "tool '{}' is not registered",
                                rpc.tool_name
                            )),
                            stdin,
                        ));
                    }
                    Err(ToolDiscoveryError::Mcp(
                        golem_service_base::clients::registry::RegistryServiceError::LimitExceeded(
                            _,
                        ),
                    )) => {
                        return Err(
                            GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted.into()
                        );
                    }
                    Err(ToolDiscoveryError::Mcp(error))
                        if matches!(
                            classify_tool_discovery_error(&ToolDiscoveryError::Mcp(error.clone())),
                            HostFailureKind::Permanent
                        ) =>
                    {
                        return Ok(rejected_tool_call(
                            &tool_name,
                            attempt_ordinal,
                            &command_path,
                            Some(input),
                            None,
                            has_stdin,
                            stdout_requested,
                            stderr_requested,
                            call_mode,
                            mcp::registry_failure(&error),
                            stdin,
                        ));
                    }
                    Err(error) => {
                        return Err(anyhow::Error::new(ClassifiedHostError {
                            kind: classify_tool_discovery_error(&error),
                            message: error.to_string(),
                        }));
                    }
                }
            };
            let plan = activation_snapshot.runtime_plan().map_err(|error| {
                let error = ToolDiscoveryError::InconsistentSnapshot { details: error };
                anyhow::Error::new(ClassifiedHostError {
                    kind: classify_tool_discovery_error(&error),
                    message: error.to_string(),
                })
            })?;
            let activation = Arc::new(
                plan.layer(0)
                    .expect("runtime plan is non-empty")
                    .activation()
                    .clone(),
            );
            (
                activation_snapshot.effective_definition().clone(),
                activation,
                EntityInvocationPlanReference::Root { plan },
            )
        }
        ToolCallTarget::Underlying(underlying) => {
            let next_reference = underlying
                .position
                .next_reference()
                .ok_or_else(|| anyhow!("underlying-tool has no next chain layer"))?;
            let definition = underlying.next_definition()?.clone();
            let activation = Arc::new(
                underlying
                    .position
                    .plan()
                    .layer(underlying.position.position() + 1)
                    .map_err(anyhow::Error::msg)?
                    .activation()
                    .clone(),
            );
            (definition, activation, next_reference)
        }
    };

    let descriptor = match describe_tool_command(
        &effective_definition,
        &input,
        attempt_ordinal,
        command_path.clone(),
        has_stdin,
        stdout_requested,
        stderr_requested,
        call_mode,
    ) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            return Ok(rejected_tool_call(
                &tool_name,
                attempt_ordinal,
                &command_path,
                Some(input),
                None,
                has_stdin,
                stdout_requested,
                stderr_requested,
                call_mode,
                error,
                stdin,
            ));
        }
    };

    // Every live admission is authorized, including middleware `Underlying` calls: the
    // capability only makes the next layer reachable, so the command and arguments that layer
    // receives after boundary projection must also be permitted under the logical tool name.
    let permission_target = accessor.with(|mut access| {
        let component = access.get().owner_component_metadata();
        let owner = caller_tool_owner(
            &component.account_email,
            &component.application_name,
            &component.environment_name,
            &component.component_name,
            &tool_name,
        );
        let command_path = command_path.iter().map(String::as_str).collect::<Vec<_>>();
        tool_target(
            owner,
            &command_path,
            descriptor.args.clone(),
            Some(&effective_definition),
        )
    });
    let permission_target = match permission_target {
        Ok(target) => target,
        Err(error) => {
            return Ok(rejected_tool_call(
                &tool_name,
                attempt_ordinal,
                &command_path,
                Some(input),
                None,
                has_stdin,
                stdout_requested,
                stderr_requested,
                call_mode,
                SerializableToolRpcError::ProtocolError(error.to_string()),
                stdin,
            ));
        }
    };
    let permit = match authorize_live_permissions_at_serialized_access(
        accessor,
        accessor.getter(),
        &[permission_target],
    )
    .await?
    {
        Ok(permit) => permit,
        Err(error) => {
            return Ok(rejected_tool_call(
                &tool_name,
                attempt_ordinal,
                &command_path,
                Some(input),
                None,
                has_stdin,
                stdout_requested,
                stderr_requested,
                call_mode,
                SerializableToolRpcError::Denied(error.to_string()),
                stdin,
            ));
        }
    };

    // Authorization may await, and the middleware handler can return in the meantime. The
    // capability must still be active when the child operation is registered, which is the
    // admission point; a use that loses this race is rejected, never admitted after revocation.
    if let ToolCallTarget::Underlying(underlying) = &target
        && let Err(error) = underlying.admit()
    {
        return Ok(rejected_tool_call(
            &tool_name,
            attempt_ordinal,
            &command_path,
            Some(input),
            None,
            has_stdin,
            stdout_requested,
            stderr_requested,
            call_mode,
            SerializableToolRpcError::Denied(error.to_string()),
            stdin,
        ));
    }

    let descriptor = EntityInvocationDescriptor::Tool(descriptor);
    let operation = accessor.with(|mut access| {
        let ctx = access.get();
        let principal = principal.unwrap_or_else(|| ctx.invocation_principal());
        ctx.owner_execution
            .tool_operations()
            .create(operation::OwnerToolOperationContext {
                parent,
                call_mode,
                activation,
                principal,
                descriptor,
                input,
            })
    });
    if operation.is_rejected() {
        return Err(anyhow!(
            "owner generation was fenced before tool operation registration"
        ));
    }

    Ok(ToolCallPreparation::Ready(PreparedToolCall {
        stdin,
        permit,
        operation,
        plan,
    }))
}

pub(super) async fn dispatch_tool_attempt<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    attempt: ToolInvocationAttempt,
    command_path: Vec<String>,
    stdin: Option<ToolCallStdin>,
    has_stdout: bool,
    has_stderr: bool,
    call_mode: EntityCallMode,
    pinned_activation: Option<Arc<ToolActivationSnapshot>>,
    principal: Option<Principal>,
    attachment_counterparty: ToolAttachmentCounterparty,
) -> anyhow::Result<ToolCallDispatch>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if accessor.with(|mut access| !access.get().state.durable_call_is_live()) {
        match EntityInvocationDurability::replay_tool_access(
            accessor,
            accessor.getter(),
            attempt.parent.clone(),
            &attempt.key_context,
        )
        .await?
        {
            ToolInvocationReplayOutcome::Rejected(response) => {
                return Ok(ToolCallDispatch::Rejected {
                    response: Box::new(decode_tool_terminal(*response)?),
                    stdin,
                });
            }
            ToolInvocationReplayOutcome::Accepted(durability) => {
                let durability = *durability;
                let descriptor = durability.operation().clone();
                let input = attempt
                    .input
                    .expect("accepted replay validated the attempt input");
                let operation = accessor.with(|mut access| {
                    access.get().owner_execution.tool_operations().create(
                        operation::OwnerToolOperationContext {
                            parent: durability.parent().clone(),
                            call_mode: durability.call_mode(),
                            activation: durability.scope().activation().clone(),
                            principal: durability.principal().clone(),
                            descriptor,
                            input,
                        },
                    )
                });
                let operation = operation
                    .accept(durability.scope().invocation_id().clone())
                    .ok_or_else(|| anyhow!("owner generation fenced replayed tool invocation"))?;
                accessor.with(|mut access| {
                    durability::install_cancellation_effect(access.get(), &operation)
                });
                return Ok(ToolCallDispatch::Accepted(Box::new(AcceptedToolCall {
                    durability,
                    operation,
                    stdin,
                    deferred_admission_inserted: false,
                    incapable_lane_ticket: None,
                    attachment_counterparty,
                })));
            }
            ToolInvocationReplayOutcome::ReplayEnded => {}
        }
    }

    let parent = attempt.parent.clone();
    let key_context = attempt.key_context.clone();
    match prepare_tool_call(
        accessor,
        attempt,
        command_path,
        stdin,
        has_stdout,
        has_stderr,
        call_mode,
        pinned_activation,
        principal,
    )
    .await?
    {
        ToolCallPreparation::Rejected { request, stdin } => {
            let response =
                record_tool_rejection_access(accessor, accessor.getter(), parent, *request).await?;
            Ok(ToolCallDispatch::Rejected {
                response: Box::new(decode_tool_terminal(response)?),
                stdin,
            })
        }
        ToolCallPreparation::Ready(prepared) => {
            let context = prepared.operation.context();
            let durability = EntityInvocationDurability::start_live_access(
                accessor,
                accessor.getter(),
                context.parent.clone(),
                &key_context,
                context.activation.entity(),
                context.principal.clone(),
                context.call_mode,
                context.descriptor.clone(),
                prepared.plan.clone(),
                context.input.clone(),
            )
            .await?;
            let operation = prepared
                .operation
                .accept(durability.scope().invocation_id().clone())
                .ok_or_else(|| anyhow!("owner generation fenced tool invocation"))?;
            accessor.with(|mut access| {
                durability::install_cancellation_effect(access.get(), &operation)
            });
            let _permit = prepared.permit;
            Ok(ToolCallDispatch::Accepted(Box::new(AcceptedToolCall {
                durability,
                operation,
                stdin: prepared.stdin,
                deferred_admission_inserted: false,
                incapable_lane_ticket: None,
                attachment_counterparty,
            })))
        }
    }
}

pub(super) async fn dispatch_tool_call<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    target: ToolCallTarget,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<Resource<ToolStdinEntry>>,
    has_stdout: bool,
    has_stderr: bool,
    call_mode: EntityCallMode,
) -> anyhow::Result<ToolCallDispatch>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let attempt = read_tool_attempt(accessor, target, input)?;
    dispatch_tool_attempt(
        accessor,
        attempt,
        command_path,
        stdin.map(ToolCallStdin::Attachment),
        has_stdout,
        has_stderr,
        call_mode,
        None,
        None,
        ToolAttachmentCounterparty::Guest,
    )
    .await
}

/// Counts a guest tool admission as tail work without depending on the guest scheduler to keep
/// its task alive. Supported SDK runtimes retain or cancel pending imports before normal task
/// exit; this guard also defends admission ordering for raw-ABI or custom-scheduler guests.
/// Every guest admission entry point holds the returned activity until that handoff, and drops it
/// before awaiting the execution result, which may wait for the parent's end.
pub(super) fn tool_admission_activity<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
) -> crate::durable_host::tail_work::TailActivity
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    accessor.with(|mut access| access.get().tail_work_tracker().activity())
}

/// Invokes a component-backed tool directly from the owner invocation driver.
///
/// The activation and optional byte stream mappings are pinned at queue admission. Stream
/// readers are reconstructed from the owner's session journal, not from the client connection.
pub(crate) async fn invoke_external_tool<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    activation: Arc<ToolActivationSnapshot>,
    tool_name: ToolName,
    command_path: Vec<String>,
    input: ModelTypedSchemaValue,
    stdin: bool,
    stdout: bool,
    stderr: bool,
    principal: Principal,
) -> Result<anyhow::Result<ToolInvokeResponse>, GuestCallSettlementError> {
    run_guest_call_settled(
        store,
        async move |accessor| -> anyhow::Result<ToolInvokeResponse> {
            let accessor =
                accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(|ctx| ctx.durable_ctx_mut());
            let (result, receiver) = oneshot::channel();
            accessor.spawn(NativeToolTask {
                activation,
                tool_name,
                command_path,
                input,
                stdin,
                stdout,
                stderr,
                principal,
                result,
            });
            receiver
                .await
                .map_err(|_| anyhow!("native tool task ended without a result"))
        },
    )
    .await
}

pub(super) fn register_tool_admission<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    accepted: &mut AcceptedToolCall,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    if accepted.operation.context().activation.filesystem()
        == golem_common::model::entity::FilesystemCapability::Capable
    {
        let parent = accepted.durability.parent().clone();
        let start = accepted.durability.scope().invocation_id().start_index();
        accepted.deferred_admission_inserted = accessor.with(|mut access| {
            access
                .get()
                .owner_execution
                .deferred_tool_admission()
                .insert(parent, start)
        });
    } else {
        let lane = accessor.with(|mut access| access.get().owner_execution.lane());
        let parent = accepted.durability.parent();
        let invocation = accepted.durability.scope().invocation_id().clone();
        let mode = accepted.durability.call_mode();
        accepted.incapable_lane_ticket = Some(lane.register_entity(
            parent.clone(),
            invocation.clone(),
            mode,
            golem_common::model::entity::FilesystemCapability::Incapable,
        )?);
    }
    Ok(())
}

pub(super) async fn invoke_tool_terminal<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    rpc: ToolRpcEntry,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<Resource<ToolStdinEntry>>,
    stdout: Option<Resource<ToolOutputEntry>>,
    stderr: Option<Resource<ToolOutputEntry>>,
) -> anyhow::Result<ToolInvokeResponse>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let stdout_requested = stdout.is_some();
    let stderr_requested = stderr.is_some();
    let stdin_rep = stdin.as_ref().map(Resource::rep);
    let stdout_rep = stdout.as_ref().map(Resource::rep);
    let stderr_rep = stderr.as_ref().map(Resource::rep);
    let admission_activity = tool_admission_activity(accessor);
    let dispatch = dispatch_tool_call(
        accessor,
        ToolCallTarget::Ambient(rpc),
        command_path,
        input,
        stdin,
        stdout_requested,
        stderr_requested,
        EntityCallMode::Synchronous,
    )
    .await;
    let response = match dispatch {
        Err(error) => {
            return Err(cleanup_failed_tool_dispatch(
                accessor, stdin_rep, stdout_rep, stderr_rep, error,
            ));
        }
        Ok(dispatch) => match dispatch {
            ToolCallDispatch::Rejected { response, stdin } => {
                close_stdin(accessor, stdin)?;
                reject_output(accessor, stdout)?;
                reject_output(accessor, stderr)?;
                *response
            }
            ToolCallDispatch::Accepted(accepted) => {
                let execution = spawn_tool_execution(accessor, *accepted, stdout, stderr).await?;
                drop(admission_activity);
                return observe_tool_invoke_plans(
                    accessor,
                    vec![execution.get_plan()],
                    async |mut results| {
                        let response = results.pop().expect("one tool invocation has one result");
                        admit_tool_response_secret_holds(accessor, response).await
                    },
                )
                .await;
            }
        },
    };
    admit_tool_response_secret_holds(accessor, response).await
}
