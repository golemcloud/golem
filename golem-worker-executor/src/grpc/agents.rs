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
    AccountId, AgentConfigEntryDto, AgentFingerprint, AgentId, AuthCtx, CancelInvocationRequest,
    DeleteWorkerRequest, EnvironmentId, ForkWorkerRequest, ForkWorkerResponse, HasAll,
    HasPromiseService, LookupResult, OplogIndex, OwnedAgentId, ResolveRevertLastInvocationsRequest,
    ResolvedRevert, RevertWorkerRequest, UpdateMode, UpdateWorkerRequest, UsesAllDeps, Worker,
    WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, WorkerUpdateMode, common_model,
    extract_caller, extract_owned_agent_id, from_proto_invocation_context, golem,
};
use crate::services::{HasAgentTypesService, HasComponentService};
use crate::worker::{CancelInvocationOutcome, CreateIntent};
use golem_common::model::agent::{InvocationFreshnessDisposition, ParsedAgentKey, ScheduleId};
use golem_common::model::caller::Principal;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{CreateRequest, CreateRequestKind};
use std::sync::Arc;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    /// Creates the keyed agent of `constructor`, or, with `unkeyed_id`, an agent without a key at
    /// that pre-allocated ID. Returns the fingerprint and the ID of the agent.
    pub(super) async fn create_worker_internal(
        &self,
        request: golem::workerexecutor::v1::CreateWorkerRequest,
        wait_until_initialized: bool,
    ) -> Result<(AgentFingerprint, AgentId), WorkerExecutorError> {
        let environment_id: EnvironmentId = request
            .environment_id
            .ok_or(WorkerExecutorError::invalid_request(
                "environment_id not found",
            ))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let component_id = request
            .component_id
            .ok_or(WorkerExecutorError::invalid_request(
                "component_id not found",
            ))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let constructor = ParsedAgentKey::try_from(request.constructor.clone().ok_or(
            WorkerExecutorError::invalid_request("constructor not found"),
        )?)
        .map_err(WorkerExecutorError::invalid_request)?;
        let target = match request.unkeyed_id {
            Some(agent_id) => {
                let agent_id: AgentId = agent_id
                    .try_into()
                    .map_err(WorkerExecutorError::invalid_request)?;
                crate::worker::target::resolve_unkeyed(
                    self.component_service().as_ref(),
                    self.agent_types().as_ref(),
                    environment_id,
                    component_id,
                    agent_id,
                    constructor,
                )
                .await?
            }
            None => {
                crate::worker::target::resolve_key(
                    self.component_service().as_ref(),
                    self.agent_types().as_ref(),
                    environment_id,
                    component_id,
                    constructor,
                )
                .await?
            }
        };
        let owned_agent_id = target.owned_agent_id;
        let owner = target
            .new_owner
            .expect("a resolved constructor creates an owner");

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let freshness_disposition = if !wait_until_initialized
            && request.freshness_disposition
                == golem::worker::InvocationFreshnessDisposition::KnownFresh as i32
        {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        if let Some(key) = request.idempotency_key.clone() {
            Worker::<Ctx>::validate_invocation_freshness(
                self,
                &owned_agent_id,
                &key.into(),
                freshness_disposition,
                Some(&owner),
            )
            .await?;
        } else if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
            return Err(WorkerExecutorError::invalid_request(
                "KnownFresh requires an idempotency key",
            ));
        }

        let caller = extract_caller(&request.caller)?;

        let env: Vec<(String, String)> = request
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let config = request
            .config
            .into_iter()
            .map(AgentConfigEntryDto::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                WorkerExecutorError::invalid_request(format!(
                    "Failed parsing local agent config: {err}"
                ))
            })?;

        let invocation_context = self.limit_invocation_context_stack_depth(
            from_proto_invocation_context(&request.invocation_context),
        );

        let worker = Worker::get_or_create_suspended_with_freshness(
            self,
            &owned_agent_id,
            Some(env),
            config,
            None,
            None,
            &invocation_context,
            caller,
            freshness_disposition,
            if request.invocation.is_some() {
                CreateIntent::Invocation {
                    owner,
                    idempotency_key: request.idempotency_key.map(Into::into),
                    invocation: request.invocation,
                }
            } else {
                CreateIntent::Create {
                    owner,
                    exclusive: !request.ignore_already_existing,
                    idempotency_key: request.idempotency_key.clone().map(Into::into),
                }
            },
        )
        .await?;

        let fingerprint = worker.get_initial_worker_metadata().fingerprint;
        let agent_id = owned_agent_id.agent_id;

        if !wait_until_initialized {
            return Ok((fingerprint, agent_id));
        }

        worker.await_initialization().await?;
        Ok((fingerprint, agent_id))
    }

    pub(super) async fn complete_promise_internal(
        &self,
        request: golem::workerexecutor::v1::CompletePromiseRequest,
    ) -> Result<golem::workerexecutor::v1::CompletePromiseSuccess, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let promise_id = request
            .promise_id
            .as_ref()
            .ok_or(WorkerExecutorError::invalid_request("promise_id not found"))?;

        let owned_agent_id = extract_owned_agent_id(
            &(&request, *promise_id),
            |(_, r)| &r.agent_id,
            |(r, _)| &r.environment_id,
        )?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        let data = request.data;

        let promise_id: common_model::PromiseId = (*promise_id)
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let completed = self
            .promise_service()
            .complete(owned_agent_id.environment_id, promise_id, data)
            .await?;

        let success = golem::workerexecutor::v1::CompletePromiseSuccess { completed };

        Ok(success)
    }

    pub(super) async fn delete_worker_internal(
        &self,
        request: DeleteWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        extract_caller(&request.caller)?;

        let expected_fingerprint = AgentFingerprint(
            request
                .expected_fingerprint
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request("expected_fingerprint not found")
                })?
                .into(),
        );
        Worker::<Ctx>::delete(self, &owned_agent_id, expected_fingerprint).await
    }

    pub(super) async fn fork_worker_internal(
        &self,
        request: ForkWorkerRequest,
    ) -> Result<ForkWorkerResponse, WorkerExecutorError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let account_id_proto = request
            .component_owner_account_id
            .ok_or(WorkerExecutorError::invalid_request("account_id not found"))?;

        let account_id: AccountId = account_id_proto.try_into().map_err(|e| {
            WorkerExecutorError::invalid_request(format!("Invalid account id: {e}"))
        })?;

        let environment_id_proto = request
            .environment_id
            .ok_or(WorkerExecutorError::invalid_request("project_id not found"))?;
        let environment_id: EnvironmentId = environment_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let target_agent_id_proto = request
            .target_agent_id
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?;

        let target_agent_id: AgentId = target_agent_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let owned_target_agent_id = OwnedAgentId::new(environment_id, &target_agent_id);

        let source_agent_id_proto = request
            .source_agent_id
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?;

        let source_agent_id: AgentId = source_agent_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let owned_source_agent_id = OwnedAgentId::new(environment_id, &source_agent_id);

        self.services
            .worker_fork_service()
            .fork(
                account_id,
                &owned_source_agent_id,
                &owned_target_agent_id.agent_id,
                OplogIndex::from_u64(request.oplog_index_cutoff),
                &auth_ctx,
                CreateRequest::new(
                    CreateRequestKind::Fork,
                    None,
                    extract_caller(&request.caller)?,
                    &InvocationContextStack::fresh(),
                ),
                request
                    .expected_fingerprint
                    .map(|fingerprint| AgentFingerprint(fingerprint.into())),
            )
            .await?;

        Ok(ForkWorkerResponse {
            result: Some(
                golem::workerexecutor::v1::fork_worker_response::Result::Success(
                    golem_schema::proto::golem::common::Empty {},
                ),
            ),
        })
    }

    pub(super) async fn revert_worker_internal(
        &self,
        request: RevertWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        extract_caller(&request.caller)?;

        let resolved_revert = request.resolved_revert.map(|resolved| ResolvedRevert {
            last_oplog_index: OplogIndex::from_u64(resolved.last_oplog_index),
        });

        let target = request
            .target
            .ok_or(WorkerExecutorError::invalid_request("target not found"))?;

        let target = target
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        Worker::<Ctx>::revert(
            self,
            &owned_agent_id,
            target,
            resolved_revert,
            request
                .expected_fingerprint
                .map(|fingerprint| AgentFingerprint(fingerprint.into())),
        )
        .await
    }

    pub(super) async fn resolve_revert_last_invocations_internal(
        &self,
        request: ResolveRevertLastInvocationsRequest,
    ) -> Result<ResolvedRevert, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        Worker::<Ctx>::resolve_revert_last_invocations(
            self,
            &owned_agent_id,
            request.number_of_invocations,
        )
        .await
    }

    pub(super) async fn cancel_invocation_internal(
        &self,
        request: CancelInvocationRequest,
    ) -> Result<bool, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        extract_caller(&request.caller)?;

        let idempotency_key: golem_common::model::InvocationId = request
            .idempotency_key
            .ok_or(WorkerExecutorError::invalid_request(
                "idempotency_key not found",
            ))?
            .into();

        if let Some(scheduled) = request.scheduled {
            let auth: AuthCtx = request.auth_ctx.unwrap().try_into().map_err(|error| {
                WorkerExecutorError::invalid_request(format!("invalid auth_ctx: {error}"))
            })?;
            if !matches!(auth, AuthCtx::System | AuthCtx::Agent(_)) {
                return Err(WorkerExecutorError::invalid_request(
                    "schedule cancellation requires an authenticated internal caller",
                ));
            }
            let canceller: Principal = scheduled
                .canceller
                .ok_or_else(|| WorkerExecutorError::invalid_request("canceller not found"))?
                .try_into()
                .map_err(WorkerExecutorError::invalid_request)?;
            let schedule_id = ScheduleId {
                id: scheduled
                    .schedule_id
                    .ok_or_else(|| WorkerExecutorError::invalid_request("schedule_id not found"))?
                    .into(),
            };
            let environment: EnvironmentId = scheduled
                .target_environment_id
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request("target_environment_id not found")
                })?
                .try_into()
                .map_err(WorkerExecutorError::invalid_request)?;
            if environment != owned_agent_id.environment_id {
                return Err(WorkerExecutorError::invalid_request(
                    "target environment mismatch",
                ));
            }
            let worker = match Worker::get_exact_existing_suspended(self, &owned_agent_id).await {
                Ok(worker) => worker,
                Err(WorkerExecutorError::AgentNotFound { .. }) => return Ok(false),
                Err(error) => return Err(error),
            };
            return worker
                .cancel_scheduled_invocation(idempotency_key, schedule_id, &canceller)
                .await;
        }

        self.ensure_not_being_deleted(&owned_agent_id).await?;
        let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_agent_id)
            .await?
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_agent_id.agent_id(),
            ))?;
        let expected_fingerprint = request
            .expected_fingerprint
            .map(|fingerprint| AgentFingerprint(fingerprint.into()));
        if expected_fingerprint.is_some_and(|expected| expected != metadata.fingerprint) {
            return Err(WorkerExecutorError::worker_not_found(
                owned_agent_id.agent_id(),
            ));
        }

        // The worker decides under its instance lock first: a recent result can belong to an
        // invocation that still runs (a retry marker) or is still queued (a fail-fast error after
        // an earlier invocation trapped), and neither is terminal.
        let worker = self
            .get_existing_for_management(&owned_agent_id, "cancel")
            .await?;
        worker
            .ensure_management_fingerprint(expected_fingerprint)
            .await?;
        if let Some(decided) = Self::cancel_decided(&worker, idempotency_key).await? {
            return Ok(decided);
        }
        // A deletion, even one that failed later and left the generation cached, can have
        // stopped its state actor. The lookup then fails, and the deletion answers instead.
        let lookup = match worker.try_lookup_invocation_result(&idempotency_key).await {
            Err(_)
                if !worker.retired_for_lost_shard() && worker.deletion_owns_retirement().await =>
            {
                Err(WorkerExecutorError::invalid_request(
                    "Worker is being deleted",
                ))
            }
            lookup => lookup,
        };
        match Self::not_found_if_deleting(&worker, lookup).await? {
            LookupResult::Complete(_) => Ok(false),
            LookupResult::New => Err(WorkerExecutorError::invalid_request("Invocation not found")),
            // Accepted after the first decision.
            LookupResult::Pending | LookupResult::Interrupted => {
                Ok(Self::cancel_decided(&worker, idempotency_key)
                    .await?
                    .unwrap_or(false))
            }
        }
    }

    /// Asks the worker to cancel under its instance lock: `true` when withdrawn, a typed error
    /// when it is running, and `None` when it is neither pending nor running.
    async fn cancel_decided(
        worker: &Arc<Worker<Ctx>>,
        idempotency_key: golem_common::model::InvocationId,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        match Self::not_found_if_deleting(worker, worker.cancel_invocation(idempotency_key).await)
            .await?
        {
            CancelInvocationOutcome::Withdrawn => Ok(Some(true)),
            CancelInvocationOutcome::Running => {
                Err(WorkerExecutorError::CannotCancelRunningInvocation)
            }
            CancelInvocationOutcome::NotPending => Ok(None),
        }
    }

    pub(super) async fn interrupt_worker_internal(
        &self,
        request: golem::workerexecutor::v1::InterruptWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        extract_caller(&request.caller)?;

        Worker::<Ctx>::interrupt(
            self,
            &owned_agent_id,
            request.recover_immediately,
            request
                .expected_fingerprint
                .map(|fingerprint| AgentFingerprint(fingerprint.into())),
        )
        .await
    }

    pub(super) async fn resume_worker_internal(
        &self,
        request: golem::workerexecutor::v1::ResumeWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        extract_caller(&request.caller)?;

        let force_resume = request.force.unwrap_or(false);

        Worker::<Ctx>::resume(
            self,
            &owned_agent_id,
            force_resume,
            request
                .expected_fingerprint
                .map(|fingerprint| AgentFingerprint(fingerprint.into())),
        )
        .await
    }

    pub(super) async fn update_worker_internal(
        &self,
        request: UpdateWorkerRequest,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        extract_caller(&request.caller)?;

        let target_revision = request.target_revision.try_into().map_err(|e| {
            WorkerExecutorError::invalid_request(format!(
                "failed converting component_revision: {e}"
            ))
        })?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let mode = match request.mode() {
            UpdateMode::Automatic => WorkerUpdateMode::Automatic,
            UpdateMode::Manual => WorkerUpdateMode::Manual,
        };

        Worker::<Ctx>::update(
            self,
            &owned_agent_id,
            mode,
            target_revision,
            request.disable_wakeup,
            request
                .expected_fingerprint
                .map(|fingerprint| AgentFingerprint(fingerprint.into())),
        )
        .await
    }
}
