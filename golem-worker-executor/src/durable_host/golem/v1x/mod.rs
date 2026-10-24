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

use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::InternalRetryResult;
use crate::durable_host::authorization::targets::agent_worker_target;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::durability::HostFailureKind;
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::ForkDetails;
use crate::preview2::golem_api_1_x::host::ForkResult;
use crate::preview2::golem_api_1_x::host::Host;
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::services::worker_proxy::WorkerProxyError;
use crate::worker::Worker;
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::StatusManagement;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::AgentId;
use golem_common::model::OplogIndex;
use golem_common::model::OwnedAgentId;
use golem_common::model::PromiseId;
use golem_common::model::RetryContext;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::AgentResourcePattern;
use golem_common::model::card::AgentVerb;
use golem_common::model::card::owner::AgentOwnerLeafPattern;
use golem_common::model::card::owner::AgentOwnerPattern;
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostRequestGolemApiAgentId;
use golem_common::model::oplog::HostRequestGolemApiComponentSlug;
use golem_common::model::oplog::HostRequestGolemApiComponentSlugAndAgentName;
use golem_common::model::oplog::HostRequestGolemApiForkAgent;
use golem_common::model::oplog::HostRequestGolemApiPromiseId;
use golem_common::model::oplog::HostRequestGolemApiRevertAgent;
use golem_common::model::oplog::HostRequestGolemApiUpdateAgent;
use golem_common::model::oplog::HostRequestNoInput;
use golem_common::model::oplog::HostResponseGolemApiAgentId;
use golem_common::model::oplog::HostResponseGolemApiAgentMetadata;
use golem_common::model::oplog::HostResponseGolemApiComponentId;
use golem_common::model::oplog::HostResponseGolemApiFork;
use golem_common::model::oplog::HostResponseGolemApiIdempotencyKey;
use golem_common::model::oplog::HostResponseGolemApiPromiseCompletion;
use golem_common::model::oplog::HostResponseGolemApiSelfAgentMetadata;
use golem_common::model::oplog::HostResponseGolemApiUnit;
use golem_common::model::oplog::host_functions::GolemApiCompletePromise;
use golem_common::model::oplog::host_functions::GolemApiFork;
use golem_common::model::oplog::host_functions::GolemApiForkWorker;
use golem_common::model::oplog::host_functions::GolemApiGenerateIdempotencyKey;
use golem_common::model::oplog::host_functions::GolemApiGetAgentMetadata;
use golem_common::model::oplog::host_functions::GolemApiGetSelfMetadata;
use golem_common::model::oplog::host_functions::GolemApiResolveAgentIdStrict;
use golem_common::model::oplog::host_functions::GolemApiResolveComponentId;
use golem_common::model::oplog::host_functions::GolemApiRevertWorker;
use golem_common::model::oplog::host_functions::GolemApiUpdateWorker;
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::time::Duration;
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::Resource;

fn classify_worker_proxy_error(err: &WorkerProxyError) -> HostFailureKind {
    match err {
        WorkerProxyError::BadRequest(_)
        | WorkerProxyError::Unauthorized(_)
        | WorkerProxyError::LimitExceeded(_)
        | WorkerProxyError::NotFound(_)
        | WorkerProxyError::AlreadyExists(_) => HostFailureKind::Permanent,
        WorkerProxyError::InternalError(_) => HostFailureKind::Transient,
    }
}

const AGENT_OPERATION_PERMISSION_DENIED: &str = "agent operation permission denied";

fn agent_operation_denial(denied: bool) -> Option<String> {
    denied.then(|| AGENT_OPERATION_PERMISSION_DENIED.to_string())
}

fn agent_operation_error(error: String) -> golem_api_1_x::host::AgentOperationError {
    if error == AGENT_OPERATION_PERMISSION_DENIED {
        golem_api_1_x::host::AgentOperationError::PermissionDenied
    } else {
        golem_api_1_x::host::AgentOperationError::BackendError(error)
    }
}

fn classify_worker_executor_error(err: &WorkerExecutorError) -> HostFailureKind {
    match err {
        WorkerExecutorError::InvalidRequest { .. }
        | WorkerExecutorError::AgentAlreadyExists { .. }
        | WorkerExecutorError::AgentNotFound { .. }
        | WorkerExecutorError::PromiseNotFound { .. }
        | WorkerExecutorError::PromiseDropped { .. }
        | WorkerExecutorError::PromiseAlreadyCompleted { .. }
        | WorkerExecutorError::ParamTypeMismatch { .. }
        | WorkerExecutorError::NoValueInMessage
        | WorkerExecutorError::ValueMismatch { .. }
        | WorkerExecutorError::UnexpectedOplogEntry { .. }
        | WorkerExecutorError::InvalidAccount
        | WorkerExecutorError::PreviousInvocationFailed { .. }
        | WorkerExecutorError::PreviousInvocationExited
        | WorkerExecutorError::ComponentNotFound { .. } => HostFailureKind::Permanent,
        _ => HostFailureKind::Transient,
    }
}

pub(super) async fn resolve_agent_owner<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    component_id: &ComponentId,
    agent: Option<&str>,
) -> Result<(AgentOwnerPattern, EnvironmentId), WorkerExecutorError> {
    let component = if *component_id == ctx.owned_agent_id.component_id() {
        ctx.owner_component_metadata().clone()
    } else {
        ctx.state
            .component_service
            .get_metadata(*component_id, None)
            .await?
    };
    let environment_id = component.environment_id;
    let owner = match agent {
        Some(agent) => AgentOwnerPattern::Agent {
            account: component.account_email,
            application: component.application_name,
            environment: component.environment_name,
            component: component.component_name,
            agent: AgentOwnerLeafPattern::Agent(agent.to_string()),
        },
        None => AgentOwnerPattern::ComponentAgents {
            account: component.account_email,
            application: component.application_name,
            environment: component.environment_name,
            component: component.component_name,
        },
    };
    Ok((owner, environment_id))
}

pub(super) async fn agent_operation_denied<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    agent_id: &AgentId,
    verb: AgentVerb,
    resource: AgentResourcePattern,
) -> Result<bool, WorkerExecutorError> {
    let (owner, _) =
        resolve_agent_owner(ctx, &agent_id.component_id, Some(&agent_id.agent_id)).await?;
    let target = agent_worker_target(owner, verb, resource);
    Ok(ctx.authorize_live_permission(&target).await?.is_err())
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<Resource<GetPromiseResultEntry>> {
        let entry =
            GetPromiseResultEntry::new(promise_id.into(), self.state.promise_service.clone());
        Ok(self.table().push(entry)?)
    }

    async fn complete_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        let promise_id: PromiseId = promise_id.into();

        let initial_is_local_worker = if self.state.is_live() {
            Some(
                match self.state.shard_service.check_worker(&promise_id.agent_id) {
                    Ok(()) => true,
                    Err(WorkerExecutorError::InvalidShardId { .. }) => false,
                    Err(other) => return Err(other.into()),
                },
            )
        } else {
            None
        };
        let request = HostRequestGolemApiPromiseId {
            promise_id: promise_id.clone(),
        };
        let mut handle = if initial_is_local_worker == Some(false) {
            DurableCallSession::<GolemApiCompletePromise, NotCancellable>::start_with_agent_authority(
                self,
                request,
                DurableFunctionType::WriteLocal,
            )
            .await?
        } else {
            DurableCallSession::<GolemApiCompletePromise, NotCancellable>::start(
                self,
                request,
                DurableFunctionType::WriteLocal,
            )
            .await?
        };

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            // A promise must be completed on the instance that is owning the agent that originally created here.
            let agent_id = &promise_id.agent_id;

            let is_local_worker = match initial_is_local_worker {
                Some(is_local_worker) => is_local_worker,
                None => match self.state.shard_service.check_worker(agent_id) {
                    Ok(()) => true,
                    Err(WorkerExecutorError::InvalidShardId { .. }) => false,
                    Err(other) => {
                        return Err(handle.trap(other));
                    }
                },
            };

            let promise_completion_result = if is_local_worker {
                match self
                    .public_state
                    .promise_service
                    .complete(promise_id.clone(), data)
                    .await
                {
                    Ok(completed) => completed,
                    Err(err) => {
                        return Err(handle.trap(err));
                    }
                }
            } else {
                // talk to the executor that actually owns the promise
                let newly_captured_auth_ctx = if initial_is_local_worker == Some(false) {
                    None
                } else {
                    Some(match self.capture_agent_auth_ctx_at_boundary().await {
                        Ok(Some(auth_ctx)) => auth_ctx,
                        Ok(None) => {
                            return Err(handle.trap(WorkerExecutorError::runtime(
                                "remote promise completion did not switch to live execution",
                            )));
                        }
                        Err(error) => return Err(handle.trap(error)),
                    })
                };
                let auth_ctx = newly_captured_auth_ctx
                    .as_ref()
                    .unwrap_or_else(|| handle.agent_auth_ctx());
                match self
                    .state
                    .worker_proxy
                    .complete_promise(promise_id.clone(), data, auth_ctx)
                    .await
                {
                    Ok(completed) => completed,
                    Err(err) => {
                        return Err(handle.trap(err));
                    }
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemApiPromiseCompletion {
                        completed: promise_completion_result,
                    },
                )
                .await?
        };

        Ok(result.completed)
    }

    async fn oplog_commit(&mut self, replicas: u8) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "oplog_commit");
        if self.state.is_live() {
            let timeout = Duration::from_secs(1);
            debug!("Worker committing oplog to {replicas} replicas");
            loop {
                // Applying a timeout to make sure the worker remains interruptible
                // A refusal means the shard has a new owner, so nothing was committed and nothing
                // can be. It surfaces as `ShardLost`, which gives the agent up without writing,
                // instead of acknowledging a commit that did not happen or retrying one that never
                // will.
                let committed = self
                    .state
                    .oplog
                    .wait_for_replicas(replicas, timeout)
                    .await
                    .map_err(|error| anyhow!(WorkerExecutorError::from(error)))?;
                if committed {
                    debug!("Worker committed oplog to {replicas} replicas");
                    return Ok(());
                } else {
                    debug!("Worker failed to commit oplog to {replicas} replicas, retrying",);
                }

                if let Some(kind) = self.check_interrupt() {
                    return Err(kind.into());
                }
            }
        } else {
            Ok(())
        }
    }

    async fn trap(&mut self, reason: String) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "trap");
        Err(anyhow::Error::new(wasmtime::Trap::UnreachableCodeReached)
            .context(format!("guest-requested trap: {reason}")))
    }

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<golem_api_1_x::host::Uuid> {
        let handle = DurableCallSession::<GolemApiGenerateIdempotencyKey, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemote,
        )
        .await?;

        // Even though `IdempotencyKey::derived` is used, we still need to persist this, because the
        // derived key depends on the oplog index. The key derives from this call's own `Start`
        // index, which an incomplete-replay re-execution reuses.
        // Reserve the logical position on replay too, even when the recorded result is returned.
        let key = self.derive_idempotency_key(handle.start_index());

        let result = handle
            .run(self, async |_| {
                let uuid = Uuid::parse_str(&key.value.to_string()).unwrap(); // this is guaranteed to be an uuid
                Ok::<_, anyhow::Error>(HostResponseGolemApiIdempotencyKey { uuid })
            })
            .await?;
        Ok(result.uuid.into())
    }

    async fn update_agent(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
        target_version: u64,
        mode: golem_api_1_x::host::UpdateMode,
    ) -> anyhow::Result<Result<(), golem_api_1_x::host::AgentOperationError>> {
        // NOTE: Mode-changing updates are rejected by the target worker-executor's gRPC
        // `update_worker` handler. The error returned by `worker_proxy.update` below
        // propagates back to the caller agent as a regular update error, so no additional
        // local check is needed here.
        let agent_id: AgentId = agent_id.into();
        let owned_agent_id = OwnedAgentId::new(self.owned_agent_id.environment_id, &agent_id);

        let mode = match mode {
            golem_api_1_x::host::UpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            golem_api_1_x::host::UpdateMode::SnapshotBased => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Manual
            }
        };

        let target_revision: ComponentRevision =
            target_version.try_into().map_err(|e: String| anyhow!(e))?;

        let begun =
            DurableCallSession::<GolemApiUpdateWorker, NotCancellable>::begin_with_agent_authority(
                self,
                DurableFunctionType::WriteRemote,
            )
            .await?;

        let result = 'result: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = agent_operation_denied(
                        self,
                        &agent_id,
                        AgentVerb::UpdateRevision,
                        AgentResourcePattern::Empty,
                    )
                    .await?;
                    let request = HostRequestGolemApiUpdateAgent {
                        agent_id: agent_id.clone(),
                        target_revision,
                        mode,
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("update-agent");
            let result = loop {
                let result = self
                    .state
                    .worker_proxy
                    .update(
                        &owned_agent_id,
                        target_revision,
                        mode,
                        false,
                        handle.agent_auth_ctx(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_proxy_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiUnit { result })
                .await?
        };

        Ok(result.result.map_err(agent_operation_error))
    }

    async fn get_self_metadata(
        &mut self,
    ) -> anyhow::Result<
        Result<golem_api_1_x::host::AgentMetadata, golem_api_1_x::host::AgentOperationError>,
    > {
        let begun = DurableCallSession::<GolemApiGetSelfMetadata, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = 'result: {
            let handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let self_agent_id = self.agent_id().clone();
                    let denied = agent_operation_denied(
                        self,
                        &self_agent_id,
                        AgentVerb::View,
                        AgentResourcePattern::Empty,
                    )
                    .await?;
                    let request = HostRequestNoInput {};
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            handle
                .run(self, async |ctx| {
                    let metadata = ctx
                        .public_state
                        .worker()
                        .get_latest_worker_metadata()
                        .await
                        .into();
                    Ok::<_, anyhow::Error>(HostResponseGolemApiSelfAgentMetadata {
                        result: Ok(metadata),
                    })
                })
                .await?
        };

        Ok(result.result.map(Into::into).map_err(agent_operation_error))
    }

    async fn get_agent_metadata(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentMetadata>> {
        let agent_id: AgentId = agent_id.into();

        let begun = DurableCallSession::<GolemApiGetAgentMetadata, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = 'result: {
            let handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = agent_operation_denied(
                        self,
                        &agent_id,
                        AgentVerb::View,
                        AgentResourcePattern::Empty,
                    )
                    .await?;
                    let request = HostRequestGolemApiAgentId {
                        agent_id: agent_id.clone(),
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            handle
                .run(self, async |ctx| {
                    let owned_agent_id =
                        OwnedAgentId::new(ctx.owned_agent_id.environment_id, &agent_id);
                    let result = ctx.state.worker_service.get(&owned_agent_id).await?;
                    let metadata: Option<AgentMetadataForGuests> = if let Some(result) = result {
                        let mut metadata = result.initial_worker_metadata;
                        if let Some(last_known_status) = &result.last_known_status {
                            metadata.last_known_status = last_known_status.clone();
                        }
                        let agent_mode = metadata.agent_mode;
                        if let Some(status) = calculate_last_known_status_with_checkpoint(
                            &ctx.state,
                            &owned_agent_id,
                            metadata.fingerprint,
                            agent_mode,
                            result.last_known_status,
                        )
                        .await
                        .map_err(|error| anyhow::Error::from(WorkerExecutorError::from(error)))?
                        {
                            metadata.last_known_status = status;
                        }
                        Some(metadata.into())
                    } else {
                        None
                    };
                    Ok::<_, anyhow::Error>(HostResponseGolemApiAgentMetadata {
                        result: Ok(metadata),
                    })
                })
                .await?
        };

        result
            .result
            .map(|metadata| metadata.map(Into::into))
            .map_err(|error| anyhow!(error))
    }

    async fn fork_agent(
        &mut self,
        source_agent_id: golem_api_1_x::host::AgentId,
        target_agent_id: golem_api_1_x::host::AgentId,
        oplog_idx_cut_off: golem_api_1_x::host::OplogIndex,
    ) -> anyhow::Result<Result<(), golem_api_1_x::host::AgentOperationError>> {
        let source_agent_id: AgentId = source_agent_id.into();
        let target_agent_id: AgentId = target_agent_id.into();

        let oplog_index_cut_off: OplogIndex = OplogIndex::from_u64(oplog_idx_cut_off);

        let begun =
            DurableCallSession::<GolemApiForkWorker, NotCancellable>::begin_with_agent_authority(
                self,
                DurableFunctionType::WriteRemote,
            )
            .await?;

        let result = 'result: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = agent_operation_denied(
                        self,
                        &source_agent_id,
                        AgentVerb::Fork,
                        AgentResourcePattern::Empty,
                    )
                    .await?;
                    let request = HostRequestGolemApiForkAgent {
                        source_agent_id: source_agent_id.clone(),
                        target_agent_id: target_agent_id.clone(),
                        oplog_index_cut_off,
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("fork-agent");
            let result = loop {
                let result = self
                    .state
                    .worker_proxy
                    .fork_worker(
                        &source_agent_id,
                        &target_agent_id,
                        &oplog_index_cut_off,
                        handle.agent_auth_ctx(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_proxy_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiUnit { result })
                .await?
        };

        Ok(result.result.map_err(agent_operation_error))
    }

    async fn revert_agent(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
        revert_target: golem_api_1_x::host::RevertAgentTarget,
    ) -> anyhow::Result<Result<(), golem_api_1_x::host::AgentOperationError>> {
        let agent_id: AgentId = agent_id.into();
        let is_self = agent_id == self.owned_agent_id.agent_id;
        let target: golem_common::model::agent::RevertWorkerTarget = revert_target.into();

        let mut resolved_revert = None;
        let begun =
            DurableCallSession::<GolemApiRevertWorker, NotCancellable>::begin_with_agent_authority(
                self,
                DurableFunctionType::WriteRemote,
            )
            .await?;

        let result = 'result: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    if !is_self
                        && let golem_common::model::agent::RevertWorkerTarget::RevertLastInvocations(
                            count,
                        ) = &target
                    {
                        let owned_agent_id =
                            OwnedAgentId::new(self.owned_agent_id.environment_id, &agent_id);
                        resolved_revert = match Worker::<Ctx>::resolve_revert_last_invocations(
                            &self.state,
                            &owned_agent_id,
                            count.number_of_invocations,
                        )
                        .await
                        {
                            Ok(resolved) => Some(resolved),
                            Err(error) => {
                                let error = match error {
                                    WorkerExecutorError::AgentNotFound { .. } => {
                                        format!("agent {owned_agent_id} does not exist")
                                    }
                                    error => error.to_string(),
                                };
                                // Freeze the refusal in Start too: incomplete recovery must
                                // not resolve a formerly invalid target against newer history.
                                break 'result begun
                                    .start_live_denied(
                                        self,
                                        HostRequestGolemApiRevertAgent {
                                            agent_id: agent_id.clone(),
                                            target: target.clone(),
                                            resolved_revert: None,
                                        },
                                        error,
                                    )
                                    .await?;
                            }
                        };
                    }
                    let resource = match (&target, &resolved_revert) {
                        (
                            golem_common::model::agent::RevertWorkerTarget::RevertToOplogIndex(
                                index,
                            ),
                            _,
                        ) => AgentResourcePattern::OplogIndex(index.last_oplog_index.into()),
                        (_, Some(resolved)) => {
                            AgentResourcePattern::OplogIndex(resolved.last_oplog_index.into())
                        }
                        (_, None) => AgentResourcePattern::Empty,
                    };
                    let denied = !is_self
                        && agent_operation_denied(self, &agent_id, AgentVerb::Revert, resource)
                            .await?;
                    let request = HostRequestGolemApiRevertAgent {
                        agent_id: agent_id.clone(),
                        target: target.clone(),
                        resolved_revert: resolved_revert.clone(),
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => {
                        let recorded_request = live.recorded_request(self).await?;
                        resolved_revert = recorded_request.resolved_revert;
                        handle = live;
                    }
                }
            }

            if is_self {
                break 'result handle
                    .complete(
                        self,
                        HostResponseGolemApiUnit {
                            result: Err(
                                "Self-revert is not supported; revert this agent through the external API or CLI"
                                    .to_string(),
                            ),
                        },
                    )
                    .await?;
            }

            let retry_properties = RetryContext::golem_api("revert-agent");
            let result = loop {
                let result = self
                    .worker_proxy()
                    .revert(
                        &agent_id,
                        target.clone(),
                        resolved_revert.clone(),
                        handle.agent_auth_ctx(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_proxy_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiUnit { result })
                .await?
        };

        Ok(result.result.map_err(agent_operation_error))
    }

    async fn resolve_component_id(
        &mut self,
        component_slug: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::ComponentId>> {
        let mut handle = DurableCallSession::<GolemApiResolveComponentId, NotCancellable>::start(
            self,
            HostRequestGolemApiComponentSlug {
                component_slug: component_slug.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("resolve-component-id");
            let result = loop {
                let result = self
                    .state
                    .component_service
                    .resolve_component(
                        component_slug.clone(),
                        self.owner_component_metadata().environment_id,
                        self.owner_component_metadata().application_id,
                        self.owner_component_metadata().account_id,
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_executor_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiComponentId { result })
                .await?
        };

        result
            .result
            .map(|opt| opt.map(golem_api_1_x::host::ComponentId::from))
            .map_err(|err| anyhow!(err))
    }

    async fn resolve_agent_id(
        &mut self,
        component_slug: String,
        agent_id: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentId>> {
        let component_id = self.resolve_component_id(component_slug).await?;
        Ok(
            component_id.map(|component_id| golem_api_1_x::host::AgentId {
                component_id,
                agent_id,
            }),
        )
    }

    async fn resolve_agent_id_strict(
        &mut self,
        component_slug: String,
        agent_name: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentId>> {
        let Some(component_id) = self.resolve_component_id(component_slug.clone()).await? else {
            return Ok(None);
        };
        let agent_id = AgentId {
            component_id: component_id.into(),
            agent_id: agent_name.clone(),
        };
        let begun = DurableCallSession::<GolemApiResolveAgentIdStrict, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = agent_operation_denied(
                        self,
                        &agent_id,
                        AgentVerb::View,
                        AgentResourcePattern::Empty,
                    )
                    .await?;
                    let request = HostRequestGolemApiComponentSlugAndAgentName {
                        component_slug,
                        agent_name,
                    };
                    if let Some(denial) = agent_operation_denial(denied) {
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("resolve-agent-id-strict");
            let result = loop {
                let owned_id = OwnedAgentId::new(self.owned_agent_id.environment_id, &agent_id);
                let result = Ok(
                    if self.state.worker_service.get(&owned_id).await?.is_some() {
                        Some(agent_id.clone())
                    } else {
                        None
                    },
                );

                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_executor_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiAgentId { result })
                .await?
        };

        result
            .result
            .map(|opt| opt.map(golem_api_1_x::host::AgentId::from))
            .map_err(|error| anyhow!(error))
    }

    async fn fork(
        &mut self,
    ) -> anyhow::Result<Result<ForkResult, golem_api_1_x::host::AgentOperationError>> {
        let begun = DurableCallSession::<GolemApiFork, NotCancellable>::begin_with_agent_authority(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let result = 'result: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let self_agent_id = self.agent_id().clone();
                    let denied = agent_operation_denied(
                        self,
                        &self_agent_id,
                        AgentVerb::Fork,
                        AgentResourcePattern::Empty,
                    )
                    .await?;
                    let request = HostRequestNoInput {};
                    if let Some(denial) = agent_operation_denial(denied) {
                        // The replay of the denied call reserves the logical counter below, so
                        // the live denial reserves it too.
                        let _ = self.idempotency_key_position();
                        break 'result begun.start_live_denied(self, request, denial).await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };

            // Reserve the logical counter on replay too so subsequent calls keep their keys.
            // The recorded Start, unlike the pre-call cursor, is unaffected by skipped hints.
            let fork_key = self.derive_idempotency_key(handle.start_index());
            let source_fingerprint = self
                .public_state
                .worker()
                .get_initial_worker_metadata()
                .fingerprint;
            let forked_phantom_id = Uuid::new_v5(
                &source_fingerprint.0,
                format!("golem:agent-fork:{}", fork_key.value).as_bytes(),
            );
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("fork");

            let new_name = if let Some(agent_id) = self.parsed_agent_id() {
                match ParsedAgentId::try_new(
                    agent_id.agent_type.clone(),
                    agent_id.parameters.clone(),
                    Some(forked_phantom_id),
                ) {
                    Ok(parsed) => parsed.to_string(),
                    Err(err) => {
                        return Err(handle.trap(anyhow!(err)));
                    }
                }
            } else {
                format!("{}-{}", self.agent_id().agent_id, forked_phantom_id)
            };

            let target_agent_id = AgentId {
                component_id: self.owned_agent_id.component_id(),
                agent_id: new_name.clone(),
            };
            // The forked worker must inherit the source state from the source-side fork call's
            // durable replay point. The source call completes with `ForkResult::Original`.
            // Force a commit so the eager `Start` is durable for the source's crash recovery.
            let copied_scope_start = None;
            let oplog_index_cut_off = handle.start_index().previous();
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;

            let created_by = self.created_by();
            let fork_result = loop {
                let fork_result = self
                    .state
                    .worker_fork
                    .fork_and_write_fork_result(
                        created_by,
                        &self.owned_agent_id,
                        &target_agent_id,
                        oplog_index_cut_off,
                        copied_scope_start,
                        handle.invocation_id(),
                        forked_phantom_id,
                        handle.agent_auth_ctx(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &fork_result,
                        classify_worker_executor_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break fork_result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let fork_result = fork_result
                .map(|_| golem_common::model::ForkResult::Original)
                .map_err(|err| err.to_string());
            handle
                .complete(
                    self,
                    HostResponseGolemApiFork {
                        forked_phantom_id,
                        result: fork_result,
                    },
                )
                .await?
        };

        match result.result {
            Ok(fork_result) => {
                let details = ForkDetails {
                    forked_phantom_id: result.forked_phantom_id.into(),
                };
                Ok(Ok(match fork_result {
                    golem_common::model::ForkResult::Original => ForkResult::Original(details),
                    golem_common::model::ForkResult::Forked => ForkResult::Forked(details),
                }))
            }
            Err(err) => Ok(Err(agent_operation_error(err))),
        }
    }
}

mod agents;
mod conversions;
mod oplog;
mod promises;
pub use agents::GetAgentsEntry;
pub use oplog::{GetOplogEntry, SearchOplogEntry};
pub use promises::GetPromiseResultEntry;
