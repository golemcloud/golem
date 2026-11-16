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

use super::EphemeralRpcTarget;
use super::{
    RpcTargetActivation, WasmRpcEntry, WasmRpcEntryPayload, WasmRpcTargetActivation,
    classified_host_error, classify_rpc_error, install_embedded_span, restore_embedded_span,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::IdempotencyKeyPosition;
use crate::durable_host::InternalRetryResult;
use crate::durable_host::authorization::targets::{agent_method_target, agent_target};
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::ReplayedDemand;
use crate::services::rpc::Rpc;
use crate::services::rpc::RpcDemand;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::worker::target::{ResolvedTarget, unkeyed_target};
use crate::workerctx::InvocationContextManagement;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use futures::FutureExt;
use futures::future::Either;
use golem_common::base_model::agent::AgentMode;
use golem_common::model::AgentFingerprint;
use golem_common::model::AgentId;
use golem_common::model::IdempotencyKey;
use golem_common::model::InvocationRequestKey;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::OwnedInvocationId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::card::owner::AgentOwnerLeafPattern;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::card::{AgentResourcePattern, AgentVerb};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::AttributeValue;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::AttributeMap;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemRpcPrepareInvocation;
use golem_common::model::oplog::HostResponseGolemRpcCreate;
use golem_common::model::oplog::HostResponseGolemRpcPrepareInvocation;
use golem_common::model::oplog::SpanKind;
use golem_common::model::oplog::SpanStarted;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcPrepareInvocation;
use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::schema::agent::AgentTypeSchema;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::collections::BTreeMap;
use std::sync::Arc;
use wasmtime::component::Resource;

pub(super) fn construct_ephemeral_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    ephemeral: EphemeralRpcTarget,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_method_streams: Arc<BTreeMap<String, bool>>,
    remote_owner: AgentOwnerPattern,
    creation_index: Option<OplogIndex>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let remote_component_id = ephemeral.component_id;
    Ok(ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral: Some(ephemeral),
            span_id: span.span_id().clone(),
            target_activation: WasmRpcTargetActivation::DeferredEphemeral { env, config },
            remote_agent_type,
            remote_component_id,
            remote_method_streams,
            remote_owner,
            creation_index,
        }),
    })?)
}

/// The agent that one invocation of a client dispatches to: the client's agent, or for a client
/// of an ephemeral agent type, the one-shot instance of the invocation.
pub(super) fn invocation_target_agent_id(
    logical_remote_agent_id: &OwnedAgentId,
    ephemeral: Option<&EphemeralRpcTarget>,
    idempotency_key: &IdempotencyKey,
    method_name: &str,
) -> anyhow::Result<OwnedAgentId> {
    Ok(match ephemeral {
        None => logical_remote_agent_id.clone(),
        Some(ephemeral) => OwnedAgentId::new(
            logical_remote_agent_id.environment_id,
            &AgentId::ephemeral_invocation(
                logical_remote_agent_id.environment_id,
                ephemeral.type_id,
                &InvocationRequestKey::Internal(idempotency_key.clone()).owner_key(method_name),
            ),
        ),
    })
}

/// The target that a call to `remote_agent_id`, from [`invocation_target_agent_id`], carries.
/// A client of an ephemeral agent type creates the invocation's one-shot instance; any other
/// client reaches only the agent it is bound to.
pub(super) fn dispatch_target(
    remote_agent_id: &OwnedAgentId,
    ephemeral: Option<&EphemeralRpcTarget>,
) -> ResolvedTarget {
    match ephemeral {
        None => ResolvedTarget::existing(remote_agent_id.clone()),
        Some(ephemeral) => unkeyed_target(
            remote_agent_id.environment_id,
            remote_agent_id.agent_id,
            ephemeral.component_id,
            ephemeral.constructor.clone(),
            AgentMode::Ephemeral,
        ),
    }
}

/// The idempotency key inputs and the logical target of one durable RPC. Once the call's `Start`
/// index is known, [`Self::at`] gives its idempotency key and the agent it dispatches to. A live
/// call uses it in the `Start` writer step; replay and repair use it with the recorded index.
#[derive(Clone)]
pub(super) struct RpcCallTarget {
    key_position: IdempotencyKeyPosition,
    logical_remote_agent_id: OwnedAgentId,
    ephemeral: Option<EphemeralRpcTarget>,
    method_name: String,
}

impl RpcCallTarget {
    pub(super) fn new(
        key_position: IdempotencyKeyPosition,
        logical_remote_agent_id: &OwnedAgentId,
        ephemeral: Option<&EphemeralRpcTarget>,
        method_name: &str,
    ) -> Self {
        Self {
            key_position,
            logical_remote_agent_id: logical_remote_agent_id.clone(),
            ephemeral: ephemeral.cloned(),
            method_name: method_name.to_string(),
        }
    }

    pub(super) fn at(
        &self,
        start_index: OplogIndex,
    ) -> Result<(IdempotencyKey, OwnedAgentId), WorkerExecutorError> {
        let idempotency_key = self.key_position.derive(start_index);
        let remote_agent_id = invocation_target_agent_id(
            &self.logical_remote_agent_id,
            self.ephemeral.as_ref(),
            &idempotency_key,
            &self.method_name,
        )
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok((idempotency_key, remote_agent_id))
    }
}

pub(super) async fn prepare_rpc_invocation_identity<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    invocation_start_index: OplogIndex,
    remote_agent_id: &OwnedAgentId,
    child_key: &IdempotencyKey,
    method_name: &str,
    freshness: InvocationFreshnessDisposition,
) -> anyhow::Result<Result<(OwnedInvocationId, AgentFingerprint), InternalRpcError>> {
    let request_key = InvocationRequestKey::Internal(child_key.clone());
    let (fingerprint, creation_data, ephemeral) = {
        let payload = ctx
            .table()
            .get(resource)?
            .payload
            .downcast_ref::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
        (
            payload.target_activation.target_fingerprint(),
            (
                payload.target_activation.target_creation_data(),
                payload.span_id.clone(),
            ),
            payload.ephemeral.clone(),
        )
    };
    let fingerprint = if let Some(fingerprint) = fingerprint {
        fingerprint
    } else {
        let mut preparation =
            DurableCallSession::<GolemRpcWasmRpcPrepareInvocation, NotCancellable>::start_with_agent_authority(
                ctx,
                HostRequestGolemRpcPrepareInvocation {
                    invocation_start_index,
                    remote_agent_id: remote_agent_id.agent_id(),
                    method_name: method_name.to_string(),
                    request_key: request_key.clone(),
                },
                DurableFunctionType::WriteRemote,
            )
            .await?;
        let mut freshness = freshness;
        if !preparation.is_live() {
            match preparation.replay(ctx).await? {
                CallReplayOutcome::Replayed(response) => {
                    return Ok(response
                        .result
                        .map(|fingerprint| {
                            (
                                request_key.finalize(
                                    remote_agent_id.agent_id(),
                                    fingerprint,
                                    method_name,
                                ),
                                fingerprint,
                            )
                        })
                        .map_err(Into::into));
                }
                CallReplayOutcome::Incomplete(live) => {
                    preparation = live;
                    freshness = InvocationFreshnessDisposition::MayExist;
                }
            }
        }
        let ((env, config), span_id) = creation_data;
        let auth_ctx = preparation.agent_auth_ctx().clone();
        let result = loop {
            let rpc = ctx.rpc();
            let interrupt = ctx.create_interrupt_signal().map(Error::from);
            let demand = rpc.create_demand(
                dispatch_target(remote_agent_id, ephemeral.as_ref()),
                method_name,
                ctx.created_by(),
                ctx.agent_id(),
                &env,
                ctx.clone_as_inherited_stack(&span_id),
                config.clone(),
                &auth_ctx,
                Some(child_key.clone()),
                freshness,
            );
            let result = match futures::future::select(Box::pin(demand), interrupt).await {
                Either::Left((result, _)) => result.map(|demand| demand.fingerprint()),
                Either::Right((error, _)) => return Err(preparation.trap(error)),
            };
            freshness = InvocationFreshnessDisposition::MayExist;
            match preparation
                .try_trigger_retry_or_loop(ctx, &result, classify_rpc_error)
                .await?
            {
                InternalRetryResult::Persist => break result,
                InternalRetryResult::RetryInternally => continue,
            }
        };
        let response = preparation
            .complete(
                ctx,
                HostResponseGolemRpcPrepareInvocation {
                    result: result.map_err(Into::into),
                },
            )
            .await?;
        match response.result {
            Ok(fingerprint) => fingerprint,
            Err(error) => return Ok(Err(error.into())),
        }
    };
    Ok(Ok((
        request_key.finalize(remote_agent_id.agent_id(), fingerprint, method_name),
        fingerprint,
    )))
}

/// What a client binds to when its connection call runs live.
pub enum RpcBinding {
    /// The agent of the target, created when it is missing (`upsert`).
    Upsert(Box<ResolvedTarget>),
    /// Only an existing agent of the client's agent type (`get-by-id`).
    Existing(OwnedAgentId),
}

impl RpcBinding {
    pub(super) fn owned_agent_id(&self) -> &OwnedAgentId {
        match self {
            Self::Upsert(target) => &target.owned_agent_id,
            Self::Existing(owned_agent_id) => owned_agent_id,
        }
    }
}

pub async fn construct_wasm_rpc_resource<Ctx: WorkerCtx, F>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    mut handle: DurableCallSession<F, NotCancellable>,
    binding: RpcBinding,
    env: &[(String, String)],
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_component_id: ComponentId,
    remote_method_streams: Arc<BTreeMap<String, bool>>,
    remote_owner: AgentOwnerPattern,
    creation_index: Option<OplogIndex>,
) -> anyhow::Result<Result<Resource<WasmRpcEntry>, RpcError>>
where
    F: golem_common::model::oplog::payload::HostPayloadPair<Resp = HostResponseGolemRpcCreate>,
{
    let target_environment_id = ctx.owned_agent_id.environment_id;
    let remote_agent_id = binding.owned_agent_id().clone();
    let rpc = ctx.rpc();
    let created_by = ctx.created_by();
    let agent_id = *ctx.agent_id();
    let stack = ctx.clone_as_inherited_stack(span.span_id());
    let key = handle
        .idempotency_key_position(ctx)
        .derive(handle.start_index());
    let auth_ctx = handle.agent_auth_ctx().clone();
    let worker_service = ctx.state.worker_service.clone();
    let result = {
        let creation = async {
            let mut owner = remote_owner.clone();
            let AgentOwnerPattern::Agent { agent, .. } = &mut owner else {
                return Err(WorkerExecutorError::runtime(
                    "invalid RPC resource owner pattern",
                ));
            };
            *agent = AgentOwnerLeafPattern::agent(
                remote_agent_id.agent_id,
                Some(remote_agent_type.type_name.clone()),
            );
            auth_ctx
                .authorize_permission(&agent_target(
                    owner,
                    AgentVerb::Invoke,
                    AgentResourcePattern::Any,
                ))
                .map_err(|error| WorkerExecutorError::permission_denied(error.to_string()))?;
            match binding {
                RpcBinding::Upsert(target) => {
                    rpc.create_and_initialize(
                        *target,
                        false,
                        created_by,
                        &agent_id,
                        env,
                        stack,
                        config.clone(),
                        &auth_ctx,
                        key,
                    )
                    .await
                }
                RpcBinding::Existing(owned_agent_id) => {
                    let fingerprint = worker_service
                        .get(&owned_agent_id)
                        .await?
                        .map(|metadata| metadata.initial_worker_metadata)
                        .filter(|metadata| {
                            metadata.agent_type_name.as_ref() == Some(&remote_agent_type.type_name)
                        })
                        .map(|metadata| metadata.fingerprint)
                        .ok_or_else(|| {
                            WorkerExecutorError::worker_not_found(owned_agent_id.agent_id())
                        })?;
                    Ok(Box::new(ReplayedDemand::new(fingerprint)) as Box<dyn RpcDemand>)
                }
            }
        };
        let interrupt = ctx.create_interrupt_signal().map(Error::from);
        tokio::pin!(creation);
        match futures::future::select(creation, interrupt).await {
            Either::Left((result, _)) => result,
            Either::Right((error, _)) => return Err(handle.trap(error)),
        }
    };
    let demand = match result {
        Ok(demand) => demand,
        Err(
            error @ (WorkerExecutorError::AgentCreationFailed { .. }
            | WorkerExecutorError::PermissionDenied { .. }
            | WorkerExecutorError::AgentNotFound { .. }
            | WorkerExecutorError::AgentKeyCollision { .. }
            | WorkerExecutorError::InvalidRequest { .. }),
        ) => {
            let error = InternalRpcError::from(error);
            let finished = super::rpc_span_finished::<()>(span.span_id(), &Err(error.clone()));
            handle
                .complete_with_span(
                    ctx,
                    HostResponseGolemRpcCreate {
                        result: Err(error.clone().into()),
                        target_environment_id,
                    },
                    finished,
                )
                .await?;
            finish_span_in_memory(ctx, span.span_id())?;
            return Ok(Err(error.into()));
        }
        Err(error) => return Err(handle.trap(error)),
    };
    let target_activation = WasmRpcTargetActivation::Activated {
        target_fingerprint: demand.fingerprint(),
        demand,
        env: env.to_vec(),
        config,
    };
    let fingerprint = target_activation
        .target_fingerprint()
        .unwrap_or(AgentFingerprint(uuid::Uuid::nil()));
    handle
        .complete(
            ctx,
            HostResponseGolemRpcCreate {
                result: Ok(fingerprint),
                target_environment_id,
            },
        )
        .await?;

    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral: None,
            span_id: span.span_id().clone(),
            target_activation,
            remote_agent_type,
            remote_component_id,
            remote_method_streams,
            remote_owner,
            creation_index,
        }),
    })?;
    Ok(Ok(entry))
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
    remote_component_id: ComponentId,
    remote_method_streams: Arc<BTreeMap<String, bool>>,
    remote_owner: AgentOwnerPattern,
    creation_index: Option<OplogIndex>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let target_activation = WasmRpcTargetActivation::ReplayPending {
        target_fingerprint,
        env,
        config,
    };
    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral: None,
            span_id: span.span_id().clone(),
            target_activation,
            remote_agent_type,
            remote_component_id,
            remote_method_streams,
            remote_owner,
            creation_index,
        }),
    })?;
    Ok(entry)
}

/// Activates the remote RPC target. With `expected_fingerprint`, the activation was recorded
/// before: it returns a [`ReplayedDemand`] of that incarnation and makes no create request. Each
/// call to the target then carries the fingerprint (see [`Rpc::invoke_and_await`]), so a call
/// never runs on another incarnation. Without one, it creates a demand, and `idempotency_key`
/// identifies that create. A `create_demand` failure is returned as a [`ClassifiedHostError`]
/// with the transient/permanent kind from [`classify_rpc_error`].
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
    idempotency_key: Option<IdempotencyKey>,
) -> Result<Box<dyn RpcDemand>, Error> {
    if let Some(recorded_fingerprint) = expected_fingerprint {
        return Ok(Box::new(ReplayedDemand::new(recorded_fingerprint)));
    }
    let demand = rpc
        .create_demand(
            ResolvedTarget::existing(remote_agent_id.clone()),
            method_name,
            created_by,
            agent_id,
            env,
            stack,
            config,
            auth_ctx,
            idempotency_key,
            InvocationFreshnessDisposition::MayExist,
        )
        .await
        .map_err(|err| classified_host_error(classify_rpc_error(&err), err.to_string()))?;
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
        let agent_id = *ctx.agent_id();
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
            None,
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

impl WasmRpcTargetActivation {
    pub(super) fn target_creation_data(&self) -> (Vec<(String, String)>, Vec<AgentConfigEntryDto>) {
        match self {
            WasmRpcTargetActivation::DeferredEphemeral { env, config }
            | WasmRpcTargetActivation::Activated { env, config, .. }
            | WasmRpcTargetActivation::ReplayPending { env, config, .. } => {
                (env.clone(), config.clone())
            }
        }
    }

    /// The activation a spawned call rebuilds before it dispatches. Only a target whose activation
    /// was recorded before the Store was replaced has one; it is rebuilt from the recorded
    /// fingerprint and creates nothing.
    pub(super) fn deferred_activation(&self) -> Option<RpcTargetActivation> {
        match self {
            WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            } => Some(RpcTargetActivation {
                target_fingerprint: *target_fingerprint,
                env: env.clone(),
                config: config.clone(),
            }),
            WasmRpcTargetActivation::DeferredEphemeral { .. }
            | WasmRpcTargetActivation::Activated { .. } => None,
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
            WasmRpcTargetActivation::DeferredEphemeral { .. } => None,
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

pub(super) async fn start_rpc_connection_call<Ctx: WorkerCtx, F>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    request: HostRequestGolemRpcCreate,
    with_agent_authority: bool,
) -> anyhow::Result<(
    DurableCallSession<F, NotCancellable>,
    Arc<InvocationContextSpan>,
)>
where
    F: golem_common::model::oplog::payload::HostPayloadPair<Req = HostRequestGolemRpcCreate>,
{
    let expected = request.remote_agent_id;
    let (mut handle, span, recorded) =
        start_rpc_binding_call::<_, F>(ctx, request, &expected, with_agent_authority).await?;
    if recorded.remote_agent_id != expected {
        return Err(handle.trap(WorkerExecutorError::unexpected_oplog_entry(
            format!("RPC connection to {expected}"),
            format!("RPC connection to {}", recorded.remote_agent_id),
        )));
    }
    Ok((handle, span))
}

pub(super) async fn start_rpc_binding_call<Ctx, F>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    request: F::Req,
    target_agent_id: &AgentId,
    with_agent_authority: bool,
) -> anyhow::Result<(
    DurableCallSession<F, NotCancellable>,
    Arc<InvocationContextSpan>,
    F::Req,
)>
where
    Ctx: WorkerCtx,
    F: golem_common::model::oplog::payload::HostPayloadPair,
    F::Req: Clone,
{
    let function_type = rpc_connection_function_type(with_agent_authority);
    let begun = if with_agent_authority {
        DurableCallSession::<F, NotCancellable>::begin_with_agent_authority(
            ctx,
            function_type.clone(),
        )
        .await?
    } else {
        DurableCallSession::<F, NotCancellable>::begin(ctx, function_type.clone()).await?
    };
    match begun.resolve(ctx).await? {
        ResolvedCall::Live(begun) => {
            let attributes = BTreeMap::from([
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-connection".to_string()),
                ),
                (
                    "target_agent_id".to_string(),
                    AttributeValue::String(target_agent_id.to_string()),
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
            match begun
                .start_live_with_span(ctx, request.clone(), started)
                .await
            {
                Ok(handle) => Ok((handle, span, request)),
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
            Ok((handle, span, recorded))
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(super) fn outbound_agent_method_target(
        &mut self,
        rpc_resource: &Resource<WasmRpcEntry>,
        remote_agent_id: &OwnedAgentId,
        method_name: &str,
    ) -> anyhow::Result<golem_common::model::card::PermissionTarget> {
        let (remote_owner, remote_agent_type) = {
            let payload = self
                .table()
                .get(rpc_resource)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
            (
                payload.remote_owner.clone(),
                payload.remote_agent_type.type_name.clone(),
            )
        };
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
            agent: AgentOwnerLeafPattern::agent(remote_agent_id.agent_id, Some(remote_agent_type)),
        };
        agent_method_target(owner, AgentVerb::Invoke, method_name)
            .map_err(|err| anyhow::anyhow!(err))
    }
}
