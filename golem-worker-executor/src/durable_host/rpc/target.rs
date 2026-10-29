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

use super::{
    OutboundRpcDenial, RpcTargetActivation, RpcTargetAdmission, WasmRpcEntry, WasmRpcEntryPayload,
    WasmRpcTargetActivation, classified_host_error, classify_rpc_error,
    embedded_invocation_span_builder, install_embedded_span, restore_embedded_span,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::IdempotencyKeyPosition;
use crate::durable_host::authorization::targets::agent_method_target;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::durability::HostFailureKind;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::oplog::OplogOps;
use crate::services::rpc::Rpc;
use crate::services::rpc::RpcDemand;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::InvocationContextManagement;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use futures::FutureExt;
use futures::future::Either;
use golem_common::model::AgentFingerprint;
use golem_common::model::AgentId;
use golem_common::model::IdempotencyKey;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::AgentVerb;
use golem_common::model::card::owner::AgentOwnerLeafPattern;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::AttributeValue;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::AttributeMap;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::oplog::HostRequest;
use golem_common::model::oplog::HostRequestGolemRpcActivate;
use golem_common::model::oplog::HostResponseGolemRpcActivate;
use golem_common::model::oplog::HostResponseGolemRpcCreate;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::oplog::SpanFinished;
use golem_common::model::oplog::SpanKind;
use golem_common::model::oplog::SpanOutcome;
use golem_common::model::oplog::SpanStarted;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcActivate;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcNew;
use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::schema::agent::AgentTypeSchema;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::component::Resource;

pub(super) fn construct_ephemeral_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    logical_agent_id: ParsedAgentId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_method_streams: Arc<BTreeMap<String, bool>>,
    remote_component_revision: ComponentRevision,
    remote_owner: AgentOwnerPattern,
    creation_index: Option<OplogIndex>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    Ok(ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral_logical_agent_id: Some(logical_agent_id),
            span_id: span.span_id().clone(),
            target_activation: WasmRpcTargetActivation::DeferredEphemeral { env, config },
            remote_agent_type,
            remote_method_streams,
            remote_component_revision,
            remote_owner,
            creation_index,
        }),
    })?)
}

pub(super) fn invocation_target_agent_id(
    logical_remote_agent_id: &OwnedAgentId,
    ephemeral_logical_agent_id: Option<&ParsedAgentId>,
    idempotency_key: &IdempotencyKey,
) -> anyhow::Result<OwnedAgentId> {
    let Some(logical_agent_id) = ephemeral_logical_agent_id else {
        return Ok(logical_remote_agent_id.clone());
    };

    let invocation_agent_id = logical_agent_id
        .with_ephemeral_invocation_phantom(idempotency_key)
        .map_err(|err| anyhow::anyhow!(err))?;
    let invocation_agent_id =
        AgentId::from_agent_id(logical_remote_agent_id.component_id(), &invocation_agent_id)
            .map_err(|err| anyhow::anyhow!(err))?;
    Ok(OwnedAgentId::new(
        logical_remote_agent_id.environment_id,
        &invocation_agent_id,
    ))
}

/// The idempotency key inputs and the logical target of one durable RPC. Once the call's `Start`
/// index is known, [`Self::at`] gives its idempotency key and the agent it dispatches to. A live
/// call uses it in the `Start` writer step; replay and repair use it with the recorded index.
#[derive(Clone)]
pub(super) struct RpcCallTarget {
    key_position: IdempotencyKeyPosition,
    logical_remote_agent_id: OwnedAgentId,
    ephemeral_logical_agent_id: Option<ParsedAgentId>,
}

impl RpcCallTarget {
    pub(super) fn new(
        key_position: IdempotencyKeyPosition,
        logical_remote_agent_id: &OwnedAgentId,
        ephemeral_logical_agent_id: Option<&ParsedAgentId>,
    ) -> Self {
        Self {
            key_position,
            logical_remote_agent_id: logical_remote_agent_id.clone(),
            ephemeral_logical_agent_id: ephemeral_logical_agent_id.cloned(),
        }
    }

    pub(super) fn at(
        &self,
        start_index: OplogIndex,
    ) -> Result<(IdempotencyKey, OwnedAgentId), WorkerExecutorError> {
        let idempotency_key = self.key_position.derive(start_index);
        let remote_agent_id = invocation_target_agent_id(
            &self.logical_remote_agent_id,
            self.ephemeral_logical_agent_id.as_ref(),
            &idempotency_key,
        )
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok((idempotency_key, remote_agent_id))
    }
}

pub async fn construct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: DurableCallSession<GolemRpcWasmRpcNew, NotCancellable>,
    remote_agent_id: AgentId,
    env: &[(String, String)],
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_method_streams: Arc<BTreeMap<String, bool>>,
    remote_component_revision: ComponentRevision,
    remote_owner: AgentOwnerPattern,
    pinned_ephemeral_identity: bool,
    creation_index: Option<OplogIndex>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let target_environment_id = ctx.owned_agent_id.environment_id;
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);

    handle
        .complete(
            ctx,
            HostResponseGolemRpcCreate {
                target_fingerprint: AgentFingerprint(uuid::Uuid::nil()),
                target_environment_id,
            },
        )
        .await?;

    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral_logical_agent_id: None,
            span_id: span.span_id().clone(),
            target_activation: initial_target_activation(
                env.to_vec(),
                config,
                pinned_ephemeral_identity,
            ),
            remote_agent_type,
            remote_method_streams,
            remote_component_revision,
            remote_owner,
            creation_index,
        }),
    })?;
    Ok(entry)
}

pub(super) async fn reconstruct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: AgentId,
    target_environment_id: EnvironmentId,
    target_fingerprint: AgentFingerprint,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_method_streams: Arc<BTreeMap<String, bool>>,
    remote_component_revision: ComponentRevision,
    remote_owner: AgentOwnerPattern,
    pinned_ephemeral_identity: bool,
    creation_index: Option<OplogIndex>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let target_activation = if pinned_ephemeral_identity || target_fingerprint.0.is_nil() {
        initial_target_activation(env, config, pinned_ephemeral_identity)
    } else {
        WasmRpcTargetActivation::ReplayPending {
            target_fingerprint,
            env,
            config,
        }
    };
    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral_logical_agent_id: None,
            span_id: span.span_id().clone(),
            target_activation,
            remote_agent_type,
            remote_method_streams,
            remote_component_revision,
            remote_owner,
            creation_index,
        }),
    })?;
    Ok(entry)
}

fn set_rpc_target_replay_pending<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    target_fingerprint: AgentFingerprint,
) -> anyhow::Result<()> {
    let payload = ctx
        .table()
        .get_mut(resource)?
        .payload
        .downcast_mut::<WasmRpcEntryPayload>()
        .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
    let WasmRpcTargetActivation::DeferredDurable { env, config } = &payload.target_activation
    else {
        anyhow::bail!("recorded RPC activation requires a deferred durable target")
    };
    payload.target_activation = WasmRpcTargetActivation::ReplayPending {
        target_fingerprint,
        env: env.clone(),
        config: config.clone(),
    };
    Ok(())
}

/// Activates the remote RPC target by creating a fresh demand and verifying that the live
/// fingerprint matches the one persisted at construction time. Failures are returned as
/// [`ClassifiedHostError`]s so every call site classifies activation errors the same way: a
/// `create_demand` failure inherits its transient/permanent kind from [`classify_rpc_error`], while
/// a fingerprint change is always permanent.
pub(super) async fn activate_rpc_target(
    rpc: &dyn Rpc,
    remote_agent_id: &OwnedAgentId,
    method_name: &str,
    created_by: AccountId,
    agent_id: &AgentId,
    env: &[(String, String)],
    stack: InvocationContextStack,
    config: Vec<AgentConfigEntryDto>,
    auth_ctx: &AuthCtx,
    expected_fingerprint: Option<AgentFingerprint>,
) -> Result<Box<dyn RpcDemand>, Error> {
    let demand = rpc
        .create_demand(
            remote_agent_id,
            method_name,
            created_by,
            agent_id,
            env,
            stack,
            config,
            auth_ctx,
        )
        .await
        .map_err(|err| classified_host_error(classify_rpc_error(&err), err.to_string()))?;
    let target_fingerprint = demand.fingerprint();
    if let Some(expected_fingerprint) = expected_fingerprint
        && target_fingerprint != expected_fingerprint
    {
        return Err(classified_host_error(
            HostFailureKind::Permanent,
            format!(
                "RPC target activation fingerprint changed during replay: persisted={expected_fingerprint}, live={target_fingerprint}"
            ),
        ));
    }
    Ok(demand)
}

pub(super) async fn ensure_rpc_target_activated<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: &Resource<WasmRpcEntry>,
    auth_ctx: &AuthCtx,
    method_name: &str,
) -> anyhow::Result<AgentFingerprint> {
    let (remote_agent_id, span_id, env, config, expected_fingerprint) = {
        let entry = ctx.table().get(this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        match &payload.target_activation {
            WasmRpcTargetActivation::Activated {
                target_fingerprint, ..
            } => return Ok(*target_fingerprint),
            WasmRpcTargetActivation::DeferredEphemeral { .. } => {
                return Err(anyhow::anyhow!(
                    "An ephemeral RPC target does not support pre-activation"
                ));
            }
            WasmRpcTargetActivation::DeferredDurable { .. } => {
                return Err(anyhow::anyhow!(
                    "A durable RPC target must record activation before dispatch"
                ));
            }
            WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            } => (
                payload.remote_agent_id.clone(),
                payload.span_id.clone(),
                env.clone(),
                config.clone(),
                Some(*target_fingerprint),
            ),
        }
    };

    let stack = ctx.clone_as_inherited_stack(&span_id);
    let rpc = ctx.rpc();
    let demand = {
        let created_by = ctx.created_by();
        let agent_id = ctx.agent_id().clone();
        let interrupt_signal = ctx.create_interrupt_signal().map(Error::from);
        let activation = activate_rpc_target(
            rpc.as_ref(),
            &remote_agent_id,
            method_name,
            created_by,
            &agent_id,
            &env,
            stack,
            config.clone(),
            auth_ctx,
            expected_fingerprint,
        );
        tokio::pin!(activation);
        match futures::future::select(activation, interrupt_signal).await {
            Either::Left((result, _)) => result?,
            Either::Right((interrupt_kind, _)) => return Err(interrupt_kind),
        }
    };
    let target_fingerprint = demand.fingerprint();

    let entry = ctx.table().get_mut(this)?;
    let payload = entry.payload.downcast_mut::<WasmRpcEntryPayload>().unwrap();
    payload.target_activation = WasmRpcTargetActivation::Activated {
        demand,
        target_fingerprint,
        env,
        config,
    };

    Ok(target_fingerprint)
}

pub(super) fn initial_target_activation(
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    pinned_ephemeral_identity: bool,
) -> WasmRpcTargetActivation {
    if pinned_ephemeral_identity {
        WasmRpcTargetActivation::DeferredEphemeral { env, config }
    } else {
        WasmRpcTargetActivation::DeferredDurable { env, config }
    }
}

impl WasmRpcTargetActivation {
    pub(super) fn target_creation_data(&self) -> (Vec<(String, String)>, Vec<AgentConfigEntryDto>) {
        match self {
            WasmRpcTargetActivation::DeferredEphemeral { env, config }
            | WasmRpcTargetActivation::DeferredDurable { env, config }
            | WasmRpcTargetActivation::Activated { env, config, .. }
            | WasmRpcTargetActivation::ReplayPending { env, config, .. } => {
                (env.clone(), config.clone())
            }
        }
    }

    pub(super) fn deferred_activation(&self) -> Option<RpcTargetActivation> {
        match self {
            WasmRpcTargetActivation::DeferredEphemeral { .. } => None,
            WasmRpcTargetActivation::DeferredDurable { env, config } => Some(RpcTargetActivation {
                target_fingerprint: None,
                env: env.clone(),
                config: config.clone(),
            }),
            WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            } => Some(RpcTargetActivation {
                target_fingerprint: Some(*target_fingerprint),
                env: env.clone(),
                config: config.clone(),
            }),
            WasmRpcTargetActivation::Activated { .. } => None,
        }
    }

    pub(super) fn target_fingerprint(&self) -> Option<AgentFingerprint> {
        match self {
            WasmRpcTargetActivation::Activated {
                target_fingerprint, ..
            }
            | WasmRpcTargetActivation::ReplayPending {
                target_fingerprint, ..
            } => Some(*target_fingerprint),
            WasmRpcTargetActivation::DeferredEphemeral { .. }
            | WasmRpcTargetActivation::DeferredDurable { .. } => None,
        }
    }
}

pub(super) fn rpc_connection_function_type(with_agent_authority: bool) -> DurableFunctionType {
    if with_agent_authority {
        DurableFunctionType::WriteRemote
    } else {
        DurableFunctionType::WriteLocal
    }
}

pub(super) async fn start_rpc_connection_call<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    request: HostRequestGolemRpcCreate,
    with_agent_authority: bool,
) -> anyhow::Result<(
    DurableCallSession<GolemRpcWasmRpcNew, NotCancellable>,
    Arc<InvocationContextSpan>,
)> {
    let function_type = rpc_connection_function_type(with_agent_authority);
    let begun = if with_agent_authority {
        DurableCallSession::<GolemRpcWasmRpcNew, NotCancellable>::begin_with_agent_authority(
            ctx,
            function_type.clone(),
        )
        .await?
    } else {
        DurableCallSession::<GolemRpcWasmRpcNew, NotCancellable>::begin(ctx, function_type.clone())
            .await?
    };
    match begun.resolve(ctx).await? {
        ResolvedCall::Live(begun) => {
            let attributes = HashMap::from([
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-connection".to_string()),
                ),
                (
                    "target_agent_id".to_string(),
                    AttributeValue::String(request.remote_agent_id.to_string()),
                ),
            ]);
            let started = SpanStarted {
                span_id: SpanId::generate(),
                trace_id: ctx.state.invocation_context.trace_id.clone(),
                trace_states: ctx.state.invocation_context.trace_states.clone(),
                parent_span_id: Some(ctx.state.current_span_id.clone()),
                links: Vec::new(),
                started_at: golem_common::model::Timestamp::now_utc(),
                attributes: AttributeMap(attributes),
                kind: SpanKind::Client,
            };
            let parent = ctx.state.current_span_id.clone();
            let span = install_embedded_span(ctx, &started, &parent)?;
            match begun.start_live_with_span(ctx, request, started).await {
                Ok(handle) => Ok((handle, span)),
                Err(error) => {
                    finish_span_in_memory(ctx, span.span_id())?;
                    Err(error.into())
                }
            }
        }
        ResolvedCall::Replay(mut handle) => {
            let recorded = match handle.recorded_request(ctx).await {
                Ok(recorded) => recorded,
                Err(error) => return Err(handle.trap(error)),
            };
            if recorded.remote_agent_id != request.remote_agent_id {
                return Err(handle.trap(WorkerExecutorError::unexpected_oplog_entry(
                    format!("RPC connection to {}", request.remote_agent_id),
                    format!("RPC connection to {}", recorded.remote_agent_id),
                )));
            }
            let started = handle.recorded_span_started(ctx).await?.ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    "RPC connection Start carrying span_started",
                    format!(
                        "RPC connection Start at {} without span metadata",
                        handle.start_index()
                    ),
                )
            })?;
            let span = restore_embedded_span(ctx, &started)?;
            Ok((handle, span))
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    fn outbound_agent_method_target(
        &mut self,
        rpc_resource: &Resource<WasmRpcEntry>,
        remote_agent_id: &OwnedAgentId,
        method_name: &str,
    ) -> anyhow::Result<golem_common::model::card::PermissionTarget> {
        let remote_owner = self
            .table()
            .get(rpc_resource)?
            .payload
            .downcast_ref::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?
            .remote_owner
            .clone();
        let AgentOwnerPattern::Agent {
            account,
            application,
            environment,
            component,
            ..
        } = remote_owner
        else {
            anyhow::bail!("invalid RPC resource owner pattern")
        };
        let owner = AgentOwnerPattern::Agent {
            account,
            application,
            environment,
            component,
            agent: AgentOwnerLeafPattern::Agent(remote_agent_id.agent_id.agent_id.clone()),
        };
        agent_method_target(owner, AgentVerb::Invoke, method_name)
            .map_err(|err| anyhow::anyhow!(err))
    }

    async fn load_recorded_rpc_activation_request(
        &self,
        start_index: OplogIndex,
    ) -> Result<HostRequestGolemRpcActivate, WorkerExecutorError> {
        let entry = self.state.oplog.read(start_index).await?;
        let request = match entry {
            OplogEntry::Start {
                function_name,
                request: Some(request),
                ..
            } if function_name == GolemRpcWasmRpcActivate::HOST_FUNCTION_NAME => request,
            other => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "golem::rpc::wasm-rpc.activate Start with a request",
                    format!("{other:?}"),
                ));
            }
        };
        let request: HostRequest =
            self.state
                .oplog
                .download_payload(request)
                .await
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "failed to load durable RPC activation request: {error}"
                    ))
                })?;
        request.try_into().map_err(|error| {
            WorkerExecutorError::unexpected_oplog_entry(
                "golem::rpc::wasm-rpc.activate request payload",
                error,
            )
        })
    }

    pub(super) async fn authorize_and_record_rpc_target_activation(
        &mut self,
        rpc_resource: &Resource<WasmRpcEntry>,
        remote_agent_id: &OwnedAgentId,
        method_name: &str,
    ) -> anyhow::Result<Result<RpcTargetAdmission, OutboundRpcDenial>> {
        let target =
            self.outbound_agent_method_target(rpc_resource, remote_agent_id, method_name)?;
        let deferred_durable = {
            let payload = self
                .table()
                .get(rpc_resource)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
            matches!(
                payload.target_activation,
                WasmRpcTargetActivation::DeferredDurable { .. }
            )
        };

        if !deferred_durable {
            return Ok(Ok(RpcTargetAdmission::LiveOnly(Box::new(target))));
        }

        let begun =
            DurableCallSession::<GolemRpcWasmRpcActivate, NotCancellable>::begin_with_agent_authority(
                self,
                DurableFunctionType::WriteRemote,
            )
            .await?;
        let mut handle;
        let decision;
        let mut rejection_span = None;
        let key_position = begun.idempotency_key_position(self);

        match begun.resolve(self).await? {
            ResolvedCall::Live(begun) => {
                decision = begun
                    .agent_auth_ctx()
                    .authorize_permission(&target)
                    .map_err(|error| SerializableRpcError::Denied {
                        details: error.to_string(),
                    });
                if decision.is_err() {
                    let connection_span_id = self
                        .table()
                        .get(rpc_resource)?
                        .payload
                        .downcast_ref::<WasmRpcEntryPayload>()
                        .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?
                        .span_id
                        .clone();
                    let span_started =
                        embedded_invocation_span_builder(self, &connection_span_id, method_name)?;
                    let request = HostRequestGolemRpcActivate {
                        remote_agent_id: remote_agent_id.agent_id(),
                        method_name: method_name.to_string(),
                        decision: decision.clone(),
                    };
                    let request_key_position = key_position.clone();
                    handle = begun
                        .start_live_with_index_and_span(self, move |start_index| {
                            let started = span_started(&request_key_position.derive(start_index));
                            Ok((request, started))
                        })
                        .await?;
                    let started = handle.recorded_span_started(self).await?.ok_or_else(|| {
                        WorkerExecutorError::runtime("denied RPC activation Start has no span")
                    })?;
                    rejection_span = Some((
                        restore_embedded_span(self, &started)?,
                        key_position.derive(handle.start_index()),
                    ));
                } else {
                    handle = begun
                        .start_live(
                            self,
                            HostRequestGolemRpcActivate {
                                remote_agent_id: remote_agent_id.agent_id(),
                                method_name: method_name.to_string(),
                                decision: decision.clone(),
                            },
                        )
                        .await?;
                }
            }
            ResolvedCall::Replay(replay) => {
                let start_index = replay.start_index();
                match replay.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        return match response.result {
                            Ok(target_fingerprint) => {
                                set_rpc_target_replay_pending(
                                    self,
                                    rpc_resource,
                                    target_fingerprint,
                                )?;
                                Ok(Ok(RpcTargetAdmission::Recorded))
                            }
                            Err(error) => Ok(Err(OutboundRpcDenial {
                                error: InternalRpcError::from(error).into(),
                                activation_decision: Some(key_position.derive(start_index)),
                            })),
                        };
                    }
                    CallReplayOutcome::Incomplete(live) => {
                        let recorded = self
                            .load_recorded_rpc_activation_request(live.start_index())
                            .await?;
                        decision = recorded.decision;
                        if decision.is_err() {
                            let started =
                                live.recorded_span_started(self).await?.ok_or_else(|| {
                                    WorkerExecutorError::unexpected_oplog_entry(
                                        "denied RPC activation Start carrying span_started",
                                        format!(
                                            "activation Start at {} without span metadata",
                                            live.start_index()
                                        ),
                                    )
                                })?;
                            let idempotency_key = key_position.derive(live.start_index());
                            rejection_span =
                                Some((restore_embedded_span(self, &started)?, idempotency_key));
                        }
                        handle = live;
                    }
                }
            }
        }

        if let Err(error) = decision {
            let error = match error {
                SerializableRpcError::Denied { details } => RpcError::Denied(details),
                other => InternalRpcError::from(other).into(),
            };
            let (span, idempotency_key) =
                rejection_span.expect("denied activation has an opening span and a reserved key");
            handle
                .complete_with_span(
                    self,
                    HostResponseGolemRpcActivate {
                        result: Err(match &error {
                            RpcError::Denied(details) => SerializableRpcError::Denied {
                                details: details.clone(),
                            },
                            _ => unreachable!("RPC activation authorization only records denial"),
                        }),
                    },
                    SpanFinished {
                        span_id: span.span_id().clone(),
                        finished_at: golem_common::model::Timestamp::now_utc(),
                        outcome: SpanOutcome::Denied,
                    },
                )
                .await?;
            finish_span_in_memory(self, span.span_id())?;
            return Ok(Err(OutboundRpcDenial {
                error,
                activation_decision: Some(idempotency_key),
            }));
        }

        let (env, config, span_id) = {
            let payload = self
                .table()
                .get(rpc_resource)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
            let WasmRpcTargetActivation::DeferredDurable { env, config } =
                &payload.target_activation
            else {
                anyhow::bail!("RPC activation decision requires a deferred durable target")
            };
            (env.clone(), config.clone(), payload.span_id.clone())
        };
        let stack = self.clone_as_inherited_stack(&span_id);
        let demand = {
            let rpc = self.rpc();
            let created_by = self.created_by();
            let agent_id = self.agent_id().clone();
            let interrupt_signal = self.create_interrupt_signal().map(Error::from);
            let activation = activate_rpc_target(
                rpc.as_ref(),
                remote_agent_id,
                method_name,
                created_by,
                &agent_id,
                &env,
                stack,
                config.clone(),
                handle.agent_auth_ctx(),
                None,
            );
            tokio::pin!(activation);
            match futures::future::select(activation, interrupt_signal).await {
                Either::Left((result, _)) => result,
                Either::Right((interrupt_kind, _)) => Err(interrupt_kind),
            }
        };
        let demand = match demand {
            Ok(demand) => demand,
            Err(error) => {
                return Err(handle.trap(error));
            }
        };
        let target_fingerprint = demand.fingerprint();
        handle
            .complete(
                self,
                HostResponseGolemRpcActivate {
                    result: Ok(target_fingerprint),
                },
            )
            .await?;
        let payload = self
            .table()
            .get_mut(rpc_resource)?
            .payload
            .downcast_mut::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
        payload.target_activation = WasmRpcTargetActivation::Activated {
            demand,
            target_fingerprint,
            env,
            config,
        };
        Ok(Ok(RpcTargetAdmission::Recorded))
    }
}
