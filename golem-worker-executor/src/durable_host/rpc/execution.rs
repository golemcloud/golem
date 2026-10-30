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
    DurableStreamingTaskParams, PreparedRpcInvocation, RpcTargetActivation, RpcTask, RpcTaskError,
    TaskRetryParams, WasmRpcEntry, WasmRpcEntryPayload, activate_rpc_target,
    ensure_rpc_target_activated, rpc_span_finished,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::InternalRetryResult;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::durability::ClassifiedHostError;
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::durability::InFunctionRetryHost;
use crate::durable_host::durable_session::StreamSession;
use crate::durable_host::durable_session::durable_stream_mapping_from_proto;
use crate::durable_host::durable_stream::SessionError;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::HasOplog;
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
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
use golem_common::model::NamedRetryPolicy;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::RetryContext;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::card::ScopeCard;
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::durable_stream::StreamRegistrationInvocation;
use golem_common::model::durable_stream::StreamSessionKey;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::HostResponseGolemRpcUnitOrFailure;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvoke;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
use golem_common::related_span;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::tracing::TraceOrigin;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::fmt::Formatter;
use std::sync::Arc;
use tracing::Instrument;
use tracing::Level;
use tracing::error;
use wasmtime::component::Resource;

pub(super) fn classify_rpc_error(err: &InternalRpcError) -> HostFailureKind {
    match err {
        InternalRpcError::ProtocolError { .. }
        | InternalRpcError::Denied { .. }
        | InternalRpcError::NotFound { .. }
        | InternalRpcError::RemoteAgentError { .. } => HostFailureKind::Permanent,
        InternalRpcError::RemoteInternalError { .. } => HostFailureKind::Transient,
    }
}

pub(super) fn current_streaming_origin<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    idempotency_key: IdempotencyKey,
) -> StreamInvocationId {
    let caller = ctx.public_state.worker().get_initial_worker_metadata();
    StreamInvocationId {
        callee_environment_id: caller.environment_id,
        callee: caller.agent_id,
        callee_fingerprint: caller.fingerprint,
        idempotency_key,
    }
}

pub(super) async fn caller_durable_rpc_streams<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    remote_agent_id: &OwnedAgentId,
    remote_fingerprint: AgentFingerprint,
    child_key: IdempotencyKey,
    logical_streaming_origin: StreamInvocationId,
    auth_ctx: AuthCtx,
) -> Result<
    (
        StreamSession,
        golem_api_grpc::proto::golem::worker::StreamInvocationIdentity,
    ),
    Error,
> {
    let worker = ctx.public_state.worker();
    let caller = worker.get_initial_worker_metadata();
    let parent_key = ctx
        .state
        .get_current_idempotency_key()
        .ok_or_else(|| anyhow::anyhow!("durable streaming RPC requires a caller invocation key"))?;
    let producer = worker.durable_stream_producer().await?;
    let origin_invocation = golem_api_grpc::proto::golem::worker::StreamInvocationIdentity {
        callee_environment_id: Some(logical_streaming_origin.callee_environment_id.into()),
        callee: Some(logical_streaming_origin.callee.into()),
        callee_fingerprint: Some(logical_streaming_origin.callee_fingerprint.0.into()),
        idempotency_key: Some(logical_streaming_origin.idempotency_key.into()),
    };
    let session_key = StreamSessionKey {
        callee_environment_id: remote_agent_id.environment_id,
        callee: remote_agent_id.agent_id(),
        callee_fingerprint: remote_fingerprint,
        idempotency_key: child_key,
    };
    let consumer_invocation = StreamInvocationId {
        callee_environment_id: caller.environment_id,
        callee: caller.agent_id,
        callee_fingerprint: caller.fingerprint,
        idempotency_key: parent_key,
    };
    let streams = StreamSession::new(
        producer,
        worker.oplog(),
        StreamRegistrationInvocation::Remote(session_key),
        [],
    )
    .with_consumer_invocation(consumer_invocation)
    .with_entity_parent_start_index(ctx.entity_parent_start_index())
    .with_rpc(ctx.rpc())
    .with_consumer_journal(worker.durable_stream_consumer_journal())
    .with_auth_ctx(Arc::new(auth_ctx))
    .require_root_attachment_before_production();
    streams
        .recover_session_mappings()
        .await
        .map_err(SessionError::into_trap)?;
    Ok((streams, origin_invocation))
}

pub(super) async fn streaming_target_fingerprint<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    prepared: &PreparedRpcInvocation,
    remote_agent_id: &OwnedAgentId,
    auth_ctx: Option<&AuthCtx>,
    idempotency_key: IdempotencyKey,
    freshness_disposition: InvocationFreshnessDisposition,
) -> Result<(AgentFingerprint, Option<Box<dyn RpcDemand>>), Error> {
    if let Some(fingerprint) = ctx
        .table()
        .get(resource)?
        .payload
        .downcast_ref::<WasmRpcEntryPayload>()
        .expect("wasm RPC resource has the wrong payload")
        .target_activation
        .target_fingerprint()
    {
        if ctx.state.is_live() {
            let auth_ctx = auth_ctx.ok_or_else(|| {
                anyhow::anyhow!("live durable streaming RPC has no captured agent authority")
            })?;
            ensure_rpc_target_activated(ctx, resource, auth_ctx, &prepared.method_name).await?;
        }
        return Ok((fingerprint, None));
    }
    let auth_ctx = auth_ctx.cloned().unwrap_or_else(|| ctx.agent_auth_ctx());
    let demand = ctx
        .rpc()
        .create_demand(
            remote_agent_id,
            &prepared.method_name,
            ctx.created_by(),
            ctx.agent_id(),
            &prepared.env,
            ctx.clone_as_inherited_stack(&prepared.connection_span_id),
            prepared.config.clone(),
            &auth_ctx,
            prepared.is_ephemeral().then_some(idempotency_key),
            freshness_disposition,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok((demand.fingerprint(), Some(demand)))
}

pub(super) async fn run_invoke_and_await<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: Resource<WasmRpcEntry>,
    prepared: PreparedRpcInvocation,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    span: Arc<InvocationContextSpan>,
    mut handle: DurableCallSession<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>,
    scope_card: Option<ScopeCard>,
) -> anyhow::Result<Result<SchemaValue, RpcError>> {
    let mut freshness_disposition =
        if known_fresh_dispatch_allowed(handle.is_live(), prepared.is_ephemeral()) {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };

    let result: Result<SchemaValue, InternalRpcError> = 'result: {
        if !handle.is_live() {
            match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(persisted) => {
                    break 'result persisted.result.map_err(Into::into);
                }
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }

        let auth_ctx = handle.take_agent_auth_ctx();
        if !prepared.is_ephemeral()
            && let Err(err) =
                ensure_rpc_target_activated(ctx, &resource, &auth_ctx, &prepared.method_name).await
        {
            return Err(handle.trap(err));
        }

        let retry_properties =
            RetryContext::rpc("invoke-and-await", &remote_agent_id, &prepared.method_name);
        let result: Result<SchemaValue, InternalRpcError> = loop {
            let stack = ctx.clone_as_inherited_stack(span.span_id());
            let interrupt_signal = ctx
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();
            let rpc = ctx.rpc();
            let created_by = ctx.created_by();
            let agent_id = ctx.agent_id().clone();
            let dispatch_freshness = freshness_disposition;
            freshness_disposition = InvocationFreshnessDisposition::MayExist;

            if dispatch_freshness == InvocationFreshnessDisposition::KnownFresh {
                ctx.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
            }

            let result = {
                let interrupt_signal = interrupt_signal.map(Error::from);
                let either_result = futures::future::select(
                    rpc.invoke_and_await(
                        &remote_agent_id,
                        Some(idempotency_key.clone()),
                        dispatch_freshness,
                        prepared.method_name.clone(),
                        prepared.input_value.clone(),
                        created_by,
                        &agent_id,
                        &prepared.env,
                        stack,
                        prepared.config.clone(),
                        &auth_ctx,
                        scope_card.clone(),
                    ),
                    interrupt_signal,
                )
                .await;
                match either_result {
                    Either::Left((result, _)) => result,
                    Either::Right((error, _)) => {
                        return Err(handle.trap(error));
                    }
                }
            };
            match handle
                .try_trigger_retry_or_loop_with_properties(
                    ctx,
                    &result,
                    classify_rpc_error,
                    retry_properties.clone(),
                )
                .await?
            {
                InternalRetryResult::Persist => break result,
                InternalRetryResult::RetryInternally => continue,
            }
        };
        let result = match result {
            Ok(value) if !ctx.secret_holds_allowed_for_value(&value).await? => {
                Err(InternalRpcError::Denied {
                    details: "permission denied".to_string(),
                })
            }
            result => result,
        };

        let span_finished = rpc_span_finished(span.span_id(), &result);
        handle
            .complete_with_span(
                ctx,
                HostResponseGolemRpcInvokeAndAwait {
                    result: result.clone().map_err(Into::into),
                },
                span_finished,
            )
            .await?;
        result
    };

    finish_span_in_memory(ctx, span.span_id())?;

    match result {
        Ok(value) => Ok(Ok(value)),
        Err(err) => {
            error!("RPC error: {err}");
            Ok(Err(err.into()))
        }
    }
}

pub(super) async fn run_invoke<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: Resource<WasmRpcEntry>,
    prepared: PreparedRpcInvocation,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    span: Arc<InvocationContextSpan>,
    mut handle: DurableCallSession<GolemRpcWasmRpcInvoke, NotCancellable>,
) -> anyhow::Result<Result<(), RpcError>> {
    let mut freshness_disposition =
        if known_fresh_dispatch_allowed(handle.is_live(), prepared.is_ephemeral()) {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };

    let result = 'result: {
        if !handle.is_live() {
            match handle.replay(ctx).await {
                Ok(CallReplayOutcome::Replayed(replayed)) => {
                    break 'result Ok(replayed);
                }
                Ok(CallReplayOutcome::Incomplete(live)) => handle = live,
                Err(err) => break 'result Err(err),
            }
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }

        let auth_ctx = handle.take_agent_auth_ctx();
        if !prepared.is_ephemeral()
            && let Err(err) =
                ensure_rpc_target_activated(ctx, &resource, &auth_ctx, &prepared.method_name).await
        {
            return Err(handle.trap(err));
        }

        let retry_properties = RetryContext::rpc("invoke", &remote_agent_id, &prepared.method_name);
        let result = loop {
            let stack = ctx.clone_as_inherited_stack(span.span_id());
            let dispatch_freshness = freshness_disposition;
            freshness_disposition = InvocationFreshnessDisposition::MayExist;

            if dispatch_freshness == InvocationFreshnessDisposition::KnownFresh {
                ctx.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await?;
            }

            let result = ctx
                .rpc()
                .invoke(
                    &remote_agent_id,
                    Some(idempotency_key.clone()),
                    dispatch_freshness,
                    prepared.method_name.clone(),
                    prepared.input_value.clone(),
                    ctx.created_by(),
                    ctx.agent_id(),
                    &prepared.env,
                    stack,
                    prepared.config.clone(),
                    &auth_ctx,
                )
                .await;
            match handle
                .try_trigger_retry_or_loop_with_properties(
                    ctx,
                    &result,
                    classify_rpc_error,
                    retry_properties.clone(),
                )
                .await?
            {
                InternalRetryResult::Persist => break result,
                InternalRetryResult::RetryInternally => continue,
            }
        };

        let result: Result<(), InternalRpcError> = result;
        let span_finished = rpc_span_finished(span.span_id(), &result);
        handle
            .complete_with_span(
                ctx,
                HostResponseGolemRpcUnitOrFailure {
                    result: result.map_err(Into::into),
                },
                span_finished,
            )
            .await
            .map_err(|err| WorkerExecutorError::runtime(err.to_string()))
    };

    finish_span_in_memory(ctx, span.span_id())?;

    match result?.result {
        Ok(_) => Ok(Ok(())),
        Err(err) => {
            let rpc_error: InternalRpcError = err.into();
            error!("RPC error: {rpc_error}");
            Ok(Err(rpc_error.into()))
        }
    }
}

impl std::fmt::Display for RpcTaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcTaskError::Rpc(err) => write!(f, "{err}"),
            RpcTaskError::Host(err) => write!(f, "{err}"),
        }
    }
}

fn classify_rpc_task_error(err: &RpcTaskError) -> HostFailureKind {
    match err {
        RpcTaskError::Rpc(err) => classify_rpc_error(err),
        RpcTaskError::Host(err) => err
            .downcast_ref::<ClassifiedHostError>()
            .map(|err| err.kind)
            .unwrap_or(HostFailureKind::Permanent),
    }
}

pub(super) fn classified_host_error(kind: HostFailureKind, message: String) -> Error {
    Error::new(ClassifiedHostError { kind, message })
}

pub(super) fn take_dispatch_freshness(
    known_fresh_dispatch_available: &std::sync::atomic::AtomicBool,
) -> InvocationFreshnessDisposition {
    if known_fresh_dispatch_available.swap(false, std::sync::atomic::Ordering::SeqCst) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

pub(super) fn known_fresh_dispatch_allowed(is_live: bool, is_ephemeral: bool) -> bool {
    is_live && is_ephemeral
}

pub(super) fn spawn_rpc_task_with_retry<Ctx: WorkerCtx>(
    activity: Option<crate::worker::suspension::RpcActivity>,
    rpc: Arc<dyn Rpc>,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    created_by: AccountId,
    agent_id: AgentId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    stack: InvocationContextStack,
    retry_params: Option<TaskRetryParams<Ctx>>,
    auth_ctx: AuthCtx,
    target_activation: Option<RpcTargetActivation>,
    initial_freshness_disposition: InvocationFreshnessDisposition,
    scope_card: Option<ScopeCard>,
) -> RpcTask {
    let first_dispatch = Arc::new(std::sync::atomic::AtomicBool::new(
        initial_freshness_disposition == InvocationFreshnessDisposition::KnownFresh,
    ));

    // The returned handle is owned by a guest resource, so this task can outlive
    // the invocation. It links back rather than running inside its span. See
    // `TraceOrigin`.
    let origin = TraceOrigin::capture_current();
    let caller_agent_id = agent_id.clone();

    // Built here, while the values are still borrowable, so nothing is cloned for
    // it and the fields stay lazy. The root of its own trace, so it has to say
    // which call is retrying: the caller alone matches every RPC that worker
    // ever made.
    let retry_span = related_span!(
        origin,
        Level::INFO,
        "rpc_invoke_retry",
        agent_id = %caller_agent_id,
        target_agent_id = %remote_agent_id.agent_id,
        method = %method_name,
        idempotency_key = %idempotency_key,
    );

    let revoker = activity.as_ref().map(|activity| activity.revoker());
    let remote = revoker.clone();
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_agent_id = remote_agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let created_by = created_by;
        let agent_id = agent_id.clone();
        let env = env.clone();
        let stack = stack.clone();
        let auth_ctx = auth_ctx.clone();
        let target_activation = target_activation.clone();
        let config = config.clone();
        let scope_card = scope_card.clone();
        let remote = remote.clone();
        let freshness_disposition = take_dispatch_freshness(&first_dispatch);
        async move {
            let _demand = if let Some(target_activation) = target_activation {
                let demand = activate_rpc_target(
                    rpc.as_ref(),
                    &remote_agent_id,
                    &method_name,
                    created_by,
                    &agent_id,
                    &target_activation.env,
                    stack.clone(),
                    target_activation.config,
                    &auth_ctx,
                    target_activation.target_fingerprint,
                )
                .await
                .map_err(RpcTaskError::Host)?;
                Some(demand)
            } else {
                None
            };

            let invocation = rpc.invoke_and_await(
                &remote_agent_id,
                Some(idempotency_key),
                freshness_disposition,
                method_name,
                input,
                created_by,
                &agent_id,
                &env,
                stack,
                config,
                &auth_ctx,
                scope_card,
            );
            let result = match &remote {
                Some(remote) => remote.remote_wait(invocation).await,
                None => invocation.await,
            }
            .map_err(RpcTaskError::Rpc)?;
            Ok(result)
        }
    };

    let application = async move {
        let scope = crate::worker::tasks::TaskScope::default();
        if let Some(params) = &retry_params {
            scope.bind(&params.worker.tasks).map_err(Error::msg)?;
        }
        scope
            .run(async move {
                let result = if let Some(retry_params) = retry_params {
                    let execution_status = retry_params.execution_status;
                    let current_retry_policy_state = retry_params
                        .worker
                        .get_last_known_status()
                        .await
                        .current_retry_state
                        .get(&retry_params.retry_point)
                        .cloned();
                    let task_ctx = crate::durable_host::durability::TaskRetryContext {
                        retry_point: retry_params.retry_point,
                        entity_parent_start_index: retry_params.entity_parent_start_index,
                        environment_state_service: retry_params.environment_state_service,
                        environment_id: retry_params.environment_id,
                        default_retry_policy: retry_params.default_retry_policy,
                        agent_config_retry_policies: retry_params.agent_config_retry_policies,
                        runtime_retry_policy_mutations: retry_params.runtime_retry_policy_mutations,
                        max_in_function_retry_delay: retry_params.max_in_function_retry_delay,
                        current_retry_policy_state,
                        retry_properties: retry_params.retry_properties,
                        worker: retry_params.worker,
                    };
                    crate::durable_host::durability::in_task_retry_loop(
                        task_ctx,
                        classify_rpc_task_error,
                        invoke,
                        || {
                            execution_status
                                .read()
                                .unwrap()
                                .create_await_interrupt_signal()
                        },
                    )
                    .await
                } else {
                    invoke().await
                };
                match result {
                    Ok(result) => Ok(Ok(result)),
                    Err(RpcTaskError::Rpc(err)) => Ok(Err(err)),
                    Err(RpcTaskError::Host(err)) => Err(err),
                }
            })
            .await
            .unwrap_or_else(|| Err(Error::msg("Worker is being deleted")))
    };
    let task = match activity {
        Some(activity) => {
            wasmtime_wasi::runtime::spawn(activity.coordinate(application).instrument(retry_span))
        }
        None => wasmtime_wasi::runtime::spawn(application.instrument(retry_span)),
    };
    RpcTask { task, revoker }
}

pub(super) fn spawn_invoke_and_await_task<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    deferred_activation: Option<RpcTargetActivation>,
    span_id: &SpanId,
    retry_point: OplogIndex,
    initial_freshness_disposition: InvocationFreshnessDisposition,
    scope_card: Option<ScopeCard>,
    auth_ctx: AuthCtx,
) -> RpcTask {
    let retry_params = {
        let mut retry_properties =
            RetryContext::rpc("invoke-and-await", remote_agent_id, &method_name);
        ctx.state.enrich_retry_properties(&mut retry_properties);
        Some(TaskRetryParams {
            environment_state_service: ctx.state.environment_state_service.clone(),
            environment_id: ctx.state.owned_agent_id.environment_id,
            default_retry_policy: NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
            agent_config_retry_policies: ctx.state.agent_config_retry_policies(),
            runtime_retry_policy_mutations: ctx.state.runtime_retry_policy_mutations.clone(),
            retry_properties,
            max_in_function_retry_delay: ctx.durable_execution_state().max_in_function_retry_delay,
            worker: ctx.public_state.worker(),
            retry_point,
            entity_parent_start_index: ctx.entity_parent_start_index(),
            execution_status: ctx.execution_status.clone(),
        })
    };
    let activity = ctx.runtime_suspension.as_ref().map(|runtime| {
        runtime.rpc_activity(
            ctx.state.config.suspend.rpc_suspend_after,
            ctx.state.config.suspend.rpc_resume_after,
        )
    });
    spawn_rpc_task_with_retry(
        activity,
        ctx.rpc(),
        remote_agent_id.clone(),
        idempotency_key,
        method_name,
        input,
        ctx.created_by(),
        ctx.agent_id().clone(),
        env,
        config,
        ctx.clone_as_inherited_stack(span_id),
        retry_params,
        auth_ctx,
        deferred_activation,
        initial_freshness_disposition,
        scope_card,
    )
}

/// Awaits a streaming RPC and settles the inputs it did not bind. The outer error is this caller's
/// own session write refused by the fence, or its own oplog unreadable, which traps rather than
/// reaching the guest as an RPC failure; the inner result is the RPC's.
pub(super) async fn await_streaming_rpc_acceptance(
    streams: &StreamSession,
    activity: Option<&crate::worker::suspension::RpcActivityRevoker>,
    mut acceptance: tokio::sync::oneshot::Receiver<
        golem_api_grpc::proto::golem::worker::InvocationAccepted,
    >,
    invocation: impl std::future::Future<
        Output = Result<crate::services::rpc::DurableRpcInvocationResult, InternalRpcError>,
    >,
) -> Result<Result<crate::services::rpc::DurableRpcInvocationResult, InternalRpcError>, SessionError>
{
    tokio::pin!(invocation);
    let (accepted, result) = tokio::select! {
        biased;
        result = async {
            match activity {
                Some(activity) => activity.remote_wait(&mut invocation).await,
                None => invocation.as_mut().await,
            }
        } => (acceptance.try_recv().ok(), Some(result)),
        accepted = &mut acceptance => (accepted.ok(), None),
    };
    if let Some(accepted) = accepted {
        let mappings = match accepted
            .stream_mappings
            .into_iter()
            .map(durable_stream_mapping_from_proto)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(mappings) => mappings,
            Err(details) => return Ok(Err(InternalRpcError::ProtocolError { details })),
        };
        match streams.cancel_unbound_rpc_inputs(&mappings).await {
            Ok(()) => {}
            Err(error @ (SessionError::Fenced(_) | SessionError::Unreadable(_))) => {
                return Err(error);
            }
            Err(SessionError::Failed(details)) => {
                return Ok(Err(InternalRpcError::ProtocolError { details }));
            }
        }
    }
    Ok(match result {
        Some(result) => result,
        None => match activity {
            Some(activity) => activity.remote_wait(invocation).await,
            None => invocation.await,
        },
    })
}

pub(super) fn spawn_streaming_invoke_and_await_task<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    params: DurableStreamingTaskParams,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    deferred_activation: Option<RpcTargetActivation>,
    span_id: &SpanId,
    scope_card: Option<ScopeCard>,
    auth_ctx: AuthCtx,
) -> RpcTask {
    let rpc = ctx.rpc();
    let created_by = ctx.created_by();
    let agent_id = ctx.agent_id().clone();
    let stack = ctx.clone_as_inherited_stack(span_id);
    let activity = ctx.runtime_suspension.as_ref().map(|runtime| {
        runtime.rpc_activity(
            ctx.state.config.suspend.rpc_suspend_after,
            ctx.state.config.suspend.rpc_resume_after,
        )
    });
    let revoker = activity.as_ref().map(|activity| activity.revoker());
    let remote = revoker.clone();
    let application = async move {
        let scope = crate::worker::tasks::TaskScope::default();
        scope
            .bind(params.streams.producer.tasks())
            .map_err(Error::msg)?;
        scope
            .run(async move {
                let _demand = if let Some(target_activation) = deferred_activation {
                    Some(
                        activate_rpc_target(
                            rpc.as_ref(),
                            &remote_agent_id,
                            &method_name,
                            created_by,
                            &agent_id,
                            &target_activation.env,
                            stack.clone(),
                            target_activation.config,
                            &auth_ctx,
                            target_activation.target_fingerprint,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                let (accepted_inputs, acceptance) = tokio::sync::oneshot::channel();
                let result = await_streaming_rpc_acceptance(
                    &params.streams,
                    remote.as_ref(),
                    acceptance,
                    rpc.invoke_and_await_streaming(
                        &remote_agent_id,
                        idempotency_key,
                        method_name,
                        params.input,
                        params.input_mappings,
                        params.expected_callee_fingerprint,
                        params.attempt_id,
                        params.origin_invocation,
                        accepted_inputs,
                        created_by,
                        &agent_id,
                        &env,
                        stack,
                        config,
                        &auth_ctx,
                        scope_card,
                    ),
                )
                .await
                .map_err(SessionError::into_trap)?;
                let result = match result {
                    Ok(result) => {
                        let mappings = result
                            .output_mappings
                            .into_iter()
                            .map(durable_stream_mapping_from_proto)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|details| InternalRpcError::ProtocolError { details });
                        match mappings {
                            Ok(mappings) => match params
                                .streams
                                .materialize_remote_result(
                                    result.value,
                                    mappings,
                                    &params.output_graph,
                                    &params.output_root,
                                )
                                .await
                            {
                                Ok(value) => Ok(value),
                                // This caller's own write refused, or its own oplog unreadable: a
                                // lost shard or a failed agent, not an RPC failure.
                                Err(
                                    error @ (SessionError::Fenced(_) | SessionError::Unreadable(_)),
                                ) => {
                                    return Err(error.into_trap());
                                }
                                Err(SessionError::Failed(details)) => {
                                    Err(InternalRpcError::ProtocolError { details })
                                }
                            },
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                Ok(result)
            })
            .await
            .unwrap_or_else(|| Err(Error::msg("Worker is being deleted")))
    };
    let task = match activity {
        Some(activity) => wasmtime_wasi::runtime::spawn(activity.coordinate(application)),
        None => wasmtime_wasi::runtime::spawn(application),
    };
    RpcTask { task, revoker }
}
