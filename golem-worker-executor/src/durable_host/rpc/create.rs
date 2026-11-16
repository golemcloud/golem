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

//! `wasm-rpc.create` and `wasm-rpc.agent-id`, the concurrent members of the RPC client.

use super::host::RemoteAgentType;
use super::{
    WasmRpcEntry, WasmRpcEntryPayload, WasmRpcTargetActivation, discard_owned_rpc_config,
    discard_owned_rpc_input, install_embedded_span, restore_embedded_span, rpc_span_finished,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::authorization::targets::agent_target;
use crate::durable_host::call_coordinator::agent_auth_ctx_at_serialized_access;
use crate::durable_host::concurrent::AccessClaimOptions;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::golem::agent::schema_value_tree_to_typed_constructor_parameters;
use crate::preview2::golem::agent::common::AgentError as WitAgentError;
use crate::preview2::golem::agent::host::{CreateOptions, Created, HostWasmRpcWithStore, RpcError};
use crate::services::HasWorker;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::worker::target::{keyed_target, unkeyed_target};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_common::base_model::agent::AgentMode;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentTypeName, ParsedAgentKey, RegisteredAgentType,
};
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::{AgentResourcePattern, AgentVerb};
use golem_common::model::invocation_context::{AttributeValue, SpanId};
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcCreate;
use golem_common::model::oplog::payload::{
    HostRequestGolemRpcCreateAgent, HostResponseGolemRpcCreated,
};
use golem_common::model::oplog::{AttributeMap, DurableFunctionType, SpanKind, SpanStarted};
use golem_common::model::{AgentId, OwnedAgentId};
use golem_common::schema::agent::RegisteredAgentTypeSchema;
use golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue;
use golem_schema::schema::wit::decode_typed_rejecting_quota_with;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use wasmtime::component::{Accessor, HasSelf, Resource};

/// Decodes guest-owned config overrides into the plain JSON that the creation request carries.
/// Every value is drained, so no handle in a later value leaks when an earlier one is rejected.
pub(super) fn decode_rpc_config<Ctx: WorkerCtx>(
    config: Vec<TypedAgentConfigValue>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<Vec<AgentConfigEntryDto>, String> {
    let mut decoded = Vec::with_capacity(config.len());
    let mut error = None;
    for entry in config {
        match decode_typed_rejecting_quota_with(entry.value, ctx) {
            Ok(typed) if error.is_none() => match golem_schema::schema::render::to_json_value(
                typed.graph(),
                typed.root_type(),
                typed.value(),
            ) {
                Ok(encoded) => decoded.push(AgentConfigEntryDto {
                    path: entry.path,
                    value: encoded.into(),
                }),
                Err(err) => error = Some(format!("Failed serializing agent config: {err}")),
            },
            Ok(_) => {}
            Err(err) => {
                error.get_or_insert_with(|| format!("Invalid agent config value: {err}"));
            }
        }
    }
    error.map_or(Ok(decoded), Err)
}

/// The agent a live `create` makes, with what its client needs.
struct LiveCreation {
    agent_id: AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    registered: RegisteredAgentType,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWasmRpcWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn create(
        accessor: &Accessor<U, Self>,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
        options: CreateOptions,
    ) -> anyhow::Result<Result<Created, RpcError>> {
        let CreateOptions {
            keyed,
            agent_config,
            env,
        } = options;
        let span_started = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.check_read_only_allows("golem::rpc::wasm-rpc::create")
                .map_err(wasmtime::Error::from)?;
            Ok::<_, anyhow::Error>(SpanStarted {
                span_id: SpanId::generate(),
                trace_id: ctx.state.invocation_context.trace_id.clone(),
                trace_states: ctx.state.invocation_context.trace_states.clone(),
                parent_span_id: Some(ctx.state.current_span_id.clone()),
                links: Vec::new(),
                started_at: golem_common::model::Timestamp::now_utc(),
                attributes: AttributeMap(BTreeMap::from([
                    (
                        "name".to_string(),
                        AttributeValue::String("rpc-connection".to_string()),
                    ),
                    (
                        "target_agent_type".to_string(),
                        AttributeValue::String(agent_type_name.clone()),
                    ),
                ])),
                kind: SpanKind::Client,
            })
        })?;
        let mut handle = DurableCallSession::<GolemRpcWasmRpcCreate, Cancellable>::start_access_with_options_and_span(
            accessor,
            accessor.getter(),
            DurableFunctionType::WriteRemote,
            AccessClaimOptions::default(),
            HostRequestGolemRpcCreateAgent {
                agent_type_name: AgentTypeName(agent_type_name.clone()),
                keyed,
            },
            span_started.clone(),
        )
        .await?;
        let span = if handle.is_live() {
            accessor.with(|mut access| {
                let ctx = access.get();
                let parent = ctx.state.current_span_id.clone();
                install_embedded_span(ctx, &span_started, &parent)
            })?
        } else {
            let recorded = handle
                .recorded_span_started_access(accessor, accessor.getter())
                .await?
                .ok_or_else(|| {
                    WorkerExecutorError::unexpected_oplog_entry(
                        "RPC create Start carrying span_started",
                        format!(
                            "RPC create Start at {} without span metadata",
                            handle.start_index()
                        ),
                    )
                })?;
            accessor.with(|mut access| restore_embedded_span(access.get(), &recorded))?
        };
        let creation_index = accessor.with(|mut access| {
            (!access.get().state.snapshotting_mode).then(|| handle.start_index())
        });

        if !handle.is_live() {
            match handle.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => {
                    accessor.with(|mut access| {
                        let ctx = access.get();
                        discard_owned_rpc_input(constructor, ctx);
                        discard_owned_rpc_config(agent_config, ctx);
                    });
                    return replayed_creation(accessor, response, span.span_id(), creation_index)
                        .await;
                }
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
        }

        let creation = live_create(
            accessor,
            &mut handle,
            agent_type_name,
            constructor,
            keyed,
            agent_config,
            env,
            span.span_id(),
        )
        .await?;
        let target_environment_id =
            accessor.with(|mut access| access.get().owned_agent_id.environment_id);
        let creation = match creation {
            Ok(creation) => creation,
            Err((error, registered)) => {
                let finished = rpc_span_finished::<()>(span.span_id(), &Err(error.clone()));
                handle
                    .complete_access_with_span(
                        accessor,
                        accessor.getter(),
                        HostResponseGolemRpcCreated {
                            result: Err(error.clone().into()),
                            type_id: registered.as_ref().map(|registered| registered.type_id),
                            agent_type: registered.map(schema_of),
                            target_environment_id,
                        },
                        finished,
                    )
                    .await?;
                accessor.with(|mut access| finish_span_in_memory(access.get(), span.span_id()))?;
                return Ok(Err(error.into()));
            }
        };
        let remote = remote_agent_type(accessor, creation.registered.clone()).await?;
        handle
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseGolemRpcCreated {
                    result: Ok((creation.agent_id, creation.fingerprint)),
                    type_id: Some(creation.registered.type_id),
                    agent_type: Some(schema_of(creation.registered)),
                    target_environment_id,
                },
            )
            .await?;
        let client = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.table().push(WasmRpcEntry {
                payload: Box::new(WasmRpcEntryPayload {
                    remote_agent_id: OwnedAgentId::new(target_environment_id, &creation.agent_id),
                    ephemeral: None,
                    span_id: span.span_id().clone(),
                    creation_index,
                    target_activation: WasmRpcTargetActivation::Activated {
                        demand: Box::new(crate::services::rpc::ReplayedDemand::new(
                            creation.fingerprint,
                        )),
                        target_fingerprint: creation.fingerprint,
                        env: creation.env,
                        config: creation.config,
                    },
                    remote_agent_type: remote.agent_type,
                    remote_component_id: remote.registered.implemented_by.component_id,
                    remote_method_streams: remote.method_streams,
                    remote_owner: remote.owner,
                }),
            })
        })?;
        Ok(Ok(Created {
            client,
            agent_id: creation.agent_id.into(),
        }))
    }

    async fn agent_id(
        accessor: &Accessor<U, Self>,
        self_: Resource<WasmRpcEntry>,
    ) -> anyhow::Result<Result<core_wire::AgentId, RpcError>> {
        accessor.with(|mut access| {
            let ctx = access.get();
            let payload = ctx
                .table()
                .get(&self_)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
            Ok(if payload.ephemeral.is_some() {
                Err(RpcError::RemoteAgentError(WitAgentError::InvalidType(
                    format!(
                        "A client of the ephemeral agent type {} has no agent ID: each invocation \
                     runs on a new instance",
                        payload.remote_agent_type.type_name
                    ),
                )))
            } else {
                Ok(payload.remote_agent_id.agent_id.into())
            })
        })
    }
}

fn schema_of(registered: RegisteredAgentType) -> RegisteredAgentTypeSchema {
    RegisteredAgentTypeSchema {
        agent_type: registered.agent_type,
        implemented_by: registered.implemented_by,
    }
}

async fn remote_agent_type<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    registered: RegisteredAgentType,
) -> anyhow::Result<RemoteAgentType> {
    let component_service = accessor.with(|mut access| access.get().component_service());
    let remote_component = component_service
        .get_metadata(
            registered.implemented_by.component_id,
            Some(registered.implemented_by.component_revision),
        )
        .await?;
    accessor.with(|mut access| {
        RemoteAgentType::new(
            registered,
            &remote_component,
            access.get().owner_component_metadata(),
        )
    })
}

/// The client of a recorded `create`, pinned to the recorded incarnation.
async fn replayed_creation<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: HostResponseGolemRpcCreated,
    span_id: &SpanId,
    creation_index: Option<golem_common::model::OplogIndex>,
) -> anyhow::Result<Result<Created, RpcError>> {
    let (agent_id, fingerprint) = match response.result {
        Ok(created) => created,
        Err(error) => {
            accessor.with(|mut access| finish_span_in_memory(access.get(), span_id))?;
            return Ok(Err(InternalRpcError::from(error).into()));
        }
    };
    let (Some(schema), Some(type_id)) = (response.agent_type, response.type_id) else {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "RPC create End with its agent type",
            "RPC create End without its agent type",
        )
        .into());
    };
    let remote = remote_agent_type(
        accessor,
        RegisteredAgentType {
            type_id,
            agent_type: schema.agent_type,
            implemented_by: schema.implemented_by,
        },
    )
    .await?;
    let client = accessor.with(|mut access| {
        access.get().table().push(WasmRpcEntry {
            payload: Box::new(WasmRpcEntryPayload {
                remote_agent_id: OwnedAgentId::new(response.target_environment_id, &agent_id),
                ephemeral: None,
                span_id: span_id.clone(),
                creation_index,
                target_activation: WasmRpcTargetActivation::ReplayPending {
                    target_fingerprint: fingerprint,
                    env: vec![],
                    config: vec![],
                },
                remote_agent_type: remote.agent_type,
                remote_component_id: remote.registered.implemented_by.component_id,
                remote_method_streams: remote.method_streams,
                remote_owner: remote.owner,
            }),
        })
    })?;
    Ok(Ok(Created {
        client,
        agent_id: agent_id.into(),
    }))
}

/// Runs a `create` live. `Ok(Err(_))` is the error the call records, with the agent type when
/// the lookup found it.
#[allow(clippy::too_many_arguments)]
async fn live_create<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<GolemRpcWasmRpcCreate, Cancellable>,
    agent_type_name: String,
    constructor: core_wire::SchemaValueTree,
    keyed: bool,
    agent_config: Vec<TypedAgentConfigValue>,
    env: Vec<(String, String)>,
    span_id: &SpanId,
) -> anyhow::Result<Result<LiveCreation, (InternalRpcError, Option<RegisteredAgentType>)>> {
    let auth_ctx = agent_auth_ctx_at_serialized_access(accessor, accessor.getter())
        .await
        .map_err(|error| handle.trap(error))?;
    let (agent_types, environment_id, owner_component_id, owner_revision) =
        accessor.with(|mut access| {
            let ctx = access.get();
            let owner = ctx.owner_component_metadata();
            (
                ctx.agent_types_service(),
                ctx.owned_agent_id.environment_id,
                owner.id,
                owner.revision,
            )
        });
    let registered = match agent_types
        .get(
            environment_id,
            owner_component_id,
            owner_revision,
            &AgentTypeName(agent_type_name.clone()),
        )
        .await
    {
        Ok(Some(registered)) => registered,
        Ok(None) => {
            accessor.with(|mut access| {
                let ctx = access.get();
                discard_owned_rpc_input(constructor, ctx);
                discard_owned_rpc_config(agent_config, ctx);
            });
            return Ok(Err((
                InternalRpcError::RemoteAgentError {
                    error: Box::new(golem_common::model::agent::AgentError::InvalidType(
                        agent_type_name,
                    )),
                },
                None,
            )));
        }
        Err(error) => return Err(handle.trap(error)),
    };
    let invalid = |details: String| InternalRpcError::RemoteAgentError {
        error: Box::new(golem_common::model::agent::AgentError::InvalidInput(
            details,
        )),
    };
    if registered.agent_type.mode != AgentMode::Durable {
        accessor.with(|mut access| {
            let ctx = access.get();
            discard_owned_rpc_input(constructor, ctx);
            discard_owned_rpc_config(agent_config, ctx);
        });
        return Ok(Err((
            invalid(format!(
                "create needs a durable agent type, and {agent_type_name} is ephemeral"
            )),
            Some(registered),
        )));
    }
    let decoded = accessor.with(|mut access| {
        let ctx = access.get();
        let parameters = schema_value_tree_to_typed_constructor_parameters(
            constructor,
            &registered.agent_type,
            ctx,
        );
        let config = decode_rpc_config(agent_config, ctx);
        (parameters, config)
    });
    let constructor = match decoded.0.and_then(|parameters| {
        ParsedAgentKey::try_new(AgentTypeName(agent_type_name.clone()), parameters)
    }) {
        Ok(constructor) => constructor,
        Err(err) => {
            return Ok(Err((
                invalid(format!("Invalid constructor input: {err}")),
                Some(registered),
            )));
        }
    };
    let config = match decoded.1 {
        Ok(config) => config,
        Err(err) => return Ok(Err((invalid(err), Some(registered)))),
    };
    let component_id = registered.implemented_by.component_id;
    let (rpc, created_by, caller, caller_fingerprint, call_key, stack, owner_component) = accessor
        .with(|mut access| {
            let ctx = access.get();
            (
                ctx.rpc(),
                ctx.created_by(),
                *ctx.agent_id(),
                ctx.public_state
                    .worker()
                    .get_initial_worker_metadata()
                    .fingerprint,
                handle
                    .idempotency_key_position(ctx)
                    .derive(handle.start_index()),
                ctx.clone_as_inherited_stack(span_id),
                ctx.owner_component_metadata().clone(),
            )
        });
    let target = if keyed {
        match keyed_target(
            environment_id,
            registered.type_id,
            component_id,
            constructor,
        ) {
            Ok(target) => target,
            Err(err) => return Ok(Err((invalid(err.to_string()), Some(registered)))),
        }
    } else {
        unkeyed_target(
            environment_id,
            AgentId::allocated(
                environment_id,
                registered.type_id.get(),
                caller,
                caller_fingerprint,
                &call_key,
            ),
            component_id,
            constructor,
            AgentMode::Durable,
        )
    };
    let agent_id = target.owned_agent_id.agent_id;
    let owner = AgentOwnerPattern::Agent {
        account: registered.implemented_by.account_email.clone(),
        application: owner_component.application_name.clone(),
        environment: owner_component.environment_name.clone(),
        component: golem_common::model::component::ComponentName(
            registered.implemented_by.component_name.clone(),
        ),
        agent: AgentOwnerLeafPattern::agent(
            agent_id,
            Some(registered.agent_type.type_name.clone()),
        ),
    };
    if let Err(error) = auth_ctx.authorize_permission(&agent_target(
        owner,
        AgentVerb::Invoke,
        AgentResourcePattern::Any,
    )) {
        return Ok(Err((
            InternalRpcError::Denied {
                details: error.to_string(),
            },
            Some(registered),
        )));
    }
    let interrupt = accessor.with(|mut access| access.get().create_interrupt_signal());
    let creation = rpc.create_and_initialize(
        target,
        true,
        created_by,
        &caller,
        &env,
        stack,
        config.clone(),
        &auth_ctx,
        call_key,
    );
    let result = match futures::future::select(Box::pin(creation), interrupt).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right((interrupt, _)) => {
            return Err(handle.trap(anyhow::Error::from(interrupt)));
        }
    };
    match result {
        Ok(demand) => Ok(Ok(LiveCreation {
            agent_id,
            fingerprint: demand.fingerprint(),
            registered,
            env,
            config,
        })),
        Err(
            error @ (WorkerExecutorError::AgentCreationFailed { .. }
            | WorkerExecutorError::PermissionDenied { .. }
            | WorkerExecutorError::AgentAlreadyExists { .. }
            | WorkerExecutorError::AgentKeyCollision { .. }
            | WorkerExecutorError::CreateIdempotencyKeyConflict { .. }
            | WorkerExecutorError::InvalidRequest { .. }),
        ) => Ok(Err((InternalRpcError::from(error), Some(registered)))),
        Err(error) => Err(handle.trap(error)),
    }
}
