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
    DurableStreamingTaskParams, RpcTargetActivation, RpcTask, RpcTaskError, RpcTaskResult,
    TaskRetryParams, activate_rpc_target,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::InternalRetryResult;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::authorize_live_permissions_at_serialized_access;
use crate::durable_host::durability::ClassifiedHostError;
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::durability::InFunctionRetryHost;
use crate::durable_host::durability::retry_backoff_waits_durably;
use crate::durable_host::durable_session::StreamSession;
use crate::durable_host::durable_session::durable_stream_mapping_from_proto;
use crate::durable_host::durable_stream::SessionError;
use crate::durable_host::secrets::secret_hold_targets_for_value;
use crate::services::HasOplog;
use crate::services::HasWorker;
use crate::services::rpc::Rpc;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::worker::target::ResolvedTarget;
use crate::workerctx::InvocationContextManagement;
use crate::workerctx::WorkerCtx;
use anyhow::Error;
use golem_common::model::AgentFingerprint;
use golem_common::model::AgentId;
use golem_common::model::IdempotencyKey;
use golem_common::model::NamedRetryPolicy;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::caller::Caller;
use golem_common::model::card::ScopeCard;
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::durable_stream::StreamRegistrationInvocation;
use golem_common::model::durable_stream::StreamSessionKey;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostPayloadPair;
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
use wasmtime::component::Accessor;
use wasmtime::component::HasSelf;

pub fn classify_rpc_error(err: &InternalRpcError) -> HostFailureKind {
    match err {
        InternalRpcError::ProtocolError { .. }
        | InternalRpcError::Denied { .. }
        | InternalRpcError::NotFound { .. }
        | InternalRpcError::RemoteAgentError { .. }
        | InternalRpcError::Conflict { .. }
        | InternalRpcError::Cancelled { .. }
        | InternalRpcError::CannotCancelRunning { .. } => HostFailureKind::Permanent,
        InternalRpcError::RemoteInternalError { .. } => HostFailureKind::Transient,
    }
}

/// Splits the result of a remote step that does not retry in the call: a permanent refusal is
/// the call's result, and a transient failure traps the call, which a repair retries.
pub fn permanent_refusal<Pair: HostPayloadPair, T>(
    handle: &mut DurableCallSession<Pair, Cancellable>,
    result: Result<T, InternalRpcError>,
) -> anyhow::Result<Result<T, InternalRpcError>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(error) => match classify_rpc_error(&error) {
            HostFailureKind::Permanent => Ok(Err(error)),
            HostFailureKind::Transient => Err(handle.trap(anyhow::Error::from(error))),
        },
    }
}

pub fn current_streaming_origin<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    idempotency_key: golem_common::model::InvocationId,
) -> StreamInvocationId {
    let caller = ctx.public_state.worker().get_initial_worker_metadata();
    StreamInvocationId {
        callee_environment_id: caller.environment_id,
        callee: caller.agent_id,
        callee_fingerprint: caller.fingerprint,
        idempotency_key,
    }
}

/// What a caller's stream session of a streaming RPC needs from the calling Store, read
/// synchronously so the session can be opened after an await.
pub struct CallerStreamsContext<Ctx: WorkerCtx> {
    worker: Arc<crate::worker::Worker<Ctx>>,
    parent_key: golem_common::model::InvocationId,
    entity_parent_start_index: Option<OplogIndex>,
    rpc: Arc<dyn crate::services::rpc::Rpc>,
}

impl<Ctx: WorkerCtx> CallerStreamsContext<Ctx> {
    pub fn capture(ctx: &DurableWorkerCtx<Ctx>) -> Result<Self, Error> {
        Ok(Self {
            worker: ctx.public_state.worker(),
            parent_key: ctx.state.get_current_idempotency_key().ok_or_else(|| {
                anyhow::anyhow!("durable streaming RPC requires a caller invocation key")
            })?,
            entity_parent_start_index: ctx.entity_parent_start_index(),
            rpc: ctx.rpc(),
        })
    }
}

pub async fn caller_durable_rpc_streams<Ctx: WorkerCtx>(
    context: CallerStreamsContext<Ctx>,
    remote_agent_id: &OwnedAgentId,
    remote_fingerprint: AgentFingerprint,
    child_key: golem_common::model::InvocationId,
    logical_streaming_origin: StreamInvocationId,
    auth_ctx: AuthCtx,
) -> Result<
    (
        StreamSession,
        golem_api_grpc::proto::golem::worker::StreamInvocationIdentity,
    ),
    Error,
> {
    let CallerStreamsContext {
        worker,
        parent_key,
        entity_parent_start_index,
        rpc,
    } = context;
    let caller = worker.get_initial_worker_metadata();
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
    .with_entity_parent_start_index(entity_parent_start_index)
    .with_rpc(rpc)
    .with_consumer_journal(worker.durable_stream_consumer_journal())
    .with_auth_ctx(Arc::new(auth_ctx))
    .require_root_attachment_before_production();
    streams
        .recover_session_mappings()
        .await
        .map_err(SessionError::into_trap)?;
    Ok((streams, origin_invocation))
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

pub fn classified_host_error(kind: HostFailureKind, message: String) -> Error {
    Error::new(ClassifiedHostError { kind, message })
}

pub fn take_dispatch_freshness(
    known_fresh_dispatch_available: &std::sync::atomic::AtomicBool,
) -> InvocationFreshnessDisposition {
    if known_fresh_dispatch_available.swap(false, std::sync::atomic::Ordering::SeqCst) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

pub fn known_fresh_dispatch_allowed(is_live: bool, is_ephemeral: bool) -> bool {
    is_live && is_ephemeral
}

pub fn spawn_rpc_task_with_retry<Ctx: WorkerCtx>(
    activity: Option<crate::worker::suspension::RpcActivity>,
    rpc: Arc<dyn Rpc>,
    remote_target: ResolvedTarget,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    created_by: AccountId,
    agent_id: AgentId,
    caller: Caller,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    stack: InvocationContextStack,
    retry_params: Option<TaskRetryParams<Ctx>>,
    auth_ctx: AuthCtx,
    target_activation: Option<RpcTargetActivation>,
    expected_callee_fingerprint: Option<AgentFingerprint>,
    initial_freshness_disposition: InvocationFreshnessDisposition,
    scope_card: Option<ScopeCard>,
) -> RpcTask {
    let first_dispatch = Arc::new(std::sync::atomic::AtomicBool::new(
        initial_freshness_disposition == InvocationFreshnessDisposition::KnownFresh,
    ));

    // The task runs in its own trace and links back to the calling span rather
    // than running inside it. See `TraceOrigin`.
    let origin = TraceOrigin::capture_current();
    let caller_agent_id = agent_id;

    // Built here, while the values are still borrowable, so nothing is cloned for
    // it and the fields stay lazy. The root of its own trace, so it has to say
    // which call is retrying: the caller alone matches every RPC that worker
    // ever made.
    let retry_span = related_span!(
        origin,
        Level::INFO,
        "rpc_invoke_retry",
        agent_id = %caller_agent_id,
        target_agent_id = %remote_target.owned_agent_id.agent_id,
        method = %method_name,
        idempotency_key = %idempotency_key,
    );

    let revoker = activity.as_ref().map(|activity| activity.revoker());
    let remote = revoker.clone();
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_target = remote_target.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let created_by = created_by;
        let agent_id = agent_id;
        let caller = caller.clone();
        let env = env.clone();
        let stack = stack.clone();
        let auth_ctx = auth_ctx.clone();
        let target_activation = target_activation.clone();
        let config = config.clone();
        let scope_card = scope_card.clone();
        let remote = remote.clone();
        let freshness_disposition = take_dispatch_freshness(&first_dispatch);
        async move {
            let (_demand, expected_callee_fingerprint) =
                if let Some(target_activation) = target_activation {
                    let demand = activate_rpc_target(
                        rpc.as_ref(),
                        &remote_target.owned_agent_id,
                        &method_name,
                        created_by,
                        &agent_id,
                        &caller,
                        &target_activation.env,
                        stack.clone(),
                        target_activation.config,
                        &auth_ctx,
                        Some(target_activation.target_fingerprint),
                        None,
                    )
                    .await
                    .map_err(RpcTaskError::Host)?;
                    let fingerprint = demand.fingerprint();
                    (Some(demand), Some(fingerprint))
                } else {
                    (None, expected_callee_fingerprint)
                };

            let invocation = rpc.invoke_and_await(
                remote_target,
                Some(idempotency_key),
                freshness_disposition,
                expected_callee_fingerprint,
                method_name,
                input,
                created_by,
                &agent_id,
                &env,
                stack,
                &caller,
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
                        retrying_start_index: Some(retry_params.retrying_start_index),
                        entity_parent_start_index: retry_params.entity_parent_start_index,
                        invocation_id: retry_params.invocation_id,
                        environment_state_service: retry_params.environment_state_service,
                        environment_id: retry_params.environment_id,
                        default_retry_policy: retry_params.default_retry_policy,
                        agent_config_retry_policies: retry_params.agent_config_retry_policies,
                        runtime_retry_policy_mutations: retry_params.runtime_retry_policy_mutations,
                        max_in_function_retry_delay: retry_params.max_in_function_retry_delay,
                        durable_wait: false,
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

pub fn spawn_invoke_and_await_task<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_target: &ResolvedTarget,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    deferred_activation: Option<RpcTargetActivation>,
    expected_callee_fingerprint: Option<AgentFingerprint>,
    mut retry_properties: golem_common::model::retry_policy::RetryProperties,
    span_id: &SpanId,
    retry_point: OplogIndex,
    retrying_start_index: OplogIndex,
    initial_freshness_disposition: InvocationFreshnessDisposition,
    scope_card: Option<ScopeCard>,
    auth_ctx: AuthCtx,
) -> RpcTask {
    let retry_params = {
        ctx.state.enrich_retry_properties(&mut retry_properties);
        Some(TaskRetryParams {
            environment_state_service: ctx.state.environment_state_service.clone(),
            environment_id: ctx.state.owned_agent_id.environment_id,
            default_retry_policy: NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
            agent_config_retry_policies: ctx.state.agent_config_retry_policies(),
            runtime_retry_policy_mutations: ctx.state.retry_policy_mutations(),
            retry_properties,
            max_in_function_retry_delay: ctx.durable_execution_state().max_in_function_retry_delay,
            worker: ctx.public_state.worker(),
            retry_point,
            retrying_start_index,
            entity_parent_start_index: ctx.entity_parent_start_index(),
            invocation_id: ctx.state.get_current_idempotency_key(),
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
        remote_target.clone(),
        idempotency_key,
        method_name,
        input,
        ctx.created_by(),
        *ctx.agent_id(),
        ctx.outgoing_caller(),
        env,
        config,
        ctx.clone_as_inherited_stack(span_id),
        retry_params,
        auth_ctx,
        deferred_activation,
        expected_callee_fingerprint,
        initial_freshness_disposition,
        scope_card,
    )
}

/// Awaits a streaming RPC and settles the inputs it did not bind. The outer error is this caller's
/// own session write refused by the fence, or its own oplog unreadable, which traps rather than
/// reaching the guest as an RPC failure; the inner result is the RPC's.
pub async fn await_streaming_rpc_acceptance(
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
            Err(
                error @ (SessionError::Fenced(_)
                | SessionError::Unreadable(_)
                | SessionError::IncompleteOutput(_)),
            ) => {
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

pub fn spawn_streaming_invoke_and_await_task<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_target: ResolvedTarget,
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
    let agent_id = *ctx.agent_id();
    let caller = ctx.outgoing_caller();
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
                            &remote_target.owned_agent_id,
                            &method_name,
                            created_by,
                            &agent_id,
                            &caller,
                            &target_activation.env,
                            stack.clone(),
                            target_activation.config,
                            &auth_ctx,
                            Some(target_activation.target_fingerprint),
                            None,
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
                        remote_target,
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
                        &caller,
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
                                    error @ (SessionError::Fenced(_)
                                    | SessionError::Unreadable(_)
                                    | SessionError::IncompleteOutput(_)),
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

/// Whether an awaited RPC sends the call again after it waited out a retry backoff.
///
/// The background task retries a transient failure inline only up to
/// `max_in_function_retry_delay`. When the call can wait durably, a longer backoff is waited out
/// here, through the runtime timer of the call: the `Error` entry records when it retries, and the
/// suspension coordinator can suspend the agent meanwhile. The new attempt uses the same key.
/// Otherwise the failure stays the recorded result, as before.
pub async fn retry_after_backoff<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>,
    mut properties: golem_common::model::RetryProperties,
    result: &Result<SchemaValue, InternalRpcError>,
) -> anyhow::Result<bool>
where
    U: 'static,
    Ctx: WorkerCtx,
{
    let Err(error) = result else {
        return Ok(false);
    };
    if classify_rpc_error(error) != HostFailureKind::Transient
        || !retry_backoff_waits_durably(accessor, accessor.getter())
    {
        return Ok(false);
    }
    // The same properties the background task resolved its policy with.
    accessor.with(|mut access| access.get().state.enrich_retry_properties(&mut properties));
    let decision = handle
        .try_trigger_retry_or_loop_context(
            accessor,
            accessor.getter(),
            result,
            classify_rpc_error,
            properties,
        )
        .await?;
    Ok(decision == InternalRetryResult::RetryInternally)
}

impl Drop for RpcTask {
    fn drop(&mut self) {
        if let Some(revoker) = &self.revoker {
            revoker.revoke();
        }
    }
}

impl std::future::Future for RpcTask {
    type Output = RpcTaskResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.task).poll(cx)
    }
}

pub async fn admit_rpc_result_secret_holds<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    result: Result<SchemaValue, InternalRpcError>,
) -> Result<Result<SchemaValue, InternalRpcError>, WorkerExecutorError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let targets = match &result {
        Ok(value) => {
            accessor.with(|mut access| secret_hold_targets_for_value(access.get(), value))?
        }
        Err(_) => return Ok(result),
    };
    if targets.is_empty() {
        return Ok(result);
    }
    match authorize_live_permissions_at_serialized_access(accessor, accessor.getter(), &targets)
        .await?
    {
        Ok(_) => Ok(result),
        Err(_) => Ok(Err(InternalRpcError::Denied {
            details: "permission denied".to_string(),
        })),
    }
}
