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
    AccountId, AgentConfigEntryDto, AgentFingerprint, AgentId, AgentMetadata, AuthCtx,
    CancelInvocationRequest, DeleteWorkerRequest, EnvironmentId, Event, ForkWorkerRequest,
    ForkWorkerResponse, HasActiveAgents, HasAll, HasEvents, HasPromiseService, HasWorkerService,
    HashMap, InvocationContextStack, LookupResult, OplogIndex, OwnedAgentId, RecvError,
    ResolveRevertLastInvocationsRequest, ResolvedRevert, RevertWorkerRequest, UpdateMode,
    UpdateWorkerRequest, UsesAllDeps, Worker, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl,
    WorkerUpdateMode, common_model, extract_owned_agent_id, extract_principal,
    from_proto_invocation_context, golem,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    /// Checks whether a create-worker request carries the same per-worker override
    /// parameters that an already-persisted worker was created with. When this returns
    /// `true` the request is treated as an idempotent duplicate and execution falls
    /// through to `get_or_create_suspended` which handles the already-existing worker.
    ///
    /// All three fields store only per-worker overrides — agent-type defaults are
    /// applied at runtime and not stored. Direct equality is therefore used for all three.
    fn is_same_worker_creation_request(
        existing: &AgentMetadata,
        request_env: &[(String, String)],
        request_config: &[AgentConfigEntryDto],
    ) -> bool {
        // env — both sides hold only per-worker overrides; compare order-independently
        let existing_env: HashMap<&str, &str> = existing
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let request_env_map: HashMap<&str, &str> = request_env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        if existing_env != request_env_map {
            return false;
        }

        // config — both sides hold only per-worker overrides; compare path and value
        if existing.config.len() != request_config.len() {
            return false;
        }
        for (existing_entry, request_entry) in existing.config.iter().zip(request_config.iter()) {
            if existing_entry.path != request_entry.path {
                return false;
            }
            // Compare the existing entry's plain (schema-guided) JSON (the same
            // form the request DTO carries) against the requested value.
            let existing_json = golem_schema::schema::render::to_json_value(
                existing_entry.value.graph(),
                existing_entry.value.root_type(),
                existing_entry.value.value(),
            )
            .ok();
            if existing_json.as_ref() != Some(&request_entry.value.0) {
                return false;
            }
        }

        true
    }

    pub(super) async fn create_worker_internal(
        &self,
        request: golem::workerexecutor::v1::CreateWorkerRequest,
        wait_until_loaded: bool,
    ) -> Result<AgentFingerprint, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        let owned_agent_id = self.canonicalize_owned_agent_id(&owned_agent_id).await?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let principal = extract_principal(&request.principal);

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

        if !request.ignore_already_existing {
            if let Some(existing) = self.active_agents().try_get(&owned_agent_id).await {
                if !Self::is_same_worker_creation_request(
                    &existing.get_initial_worker_metadata(),
                    &env,
                    &config,
                ) {
                    return Err(WorkerExecutorError::worker_already_exists(
                        owned_agent_id.agent_id(),
                    ));
                }
            } else {
                let existing_worker = self.worker_service().get(&owned_agent_id).await?;
                if let Some(existing) = existing_worker
                    && !Self::is_same_worker_creation_request(
                        &existing.initial_worker_metadata,
                        &env,
                        &config,
                    )
                {
                    return Err(WorkerExecutorError::worker_already_exists(
                        owned_agent_id.agent_id(),
                    ));
                }
            }
        }

        let invocation_context = self.limit_invocation_context_stack_depth(
            from_proto_invocation_context(&request.invocation_context),
        );

        let worker = Worker::get_or_create_suspended(
            self,
            &owned_agent_id,
            Some(env),
            config,
            None,
            None,
            &invocation_context,
            principal,
        )
        .await?;

        let fingerprint = worker.get_initial_worker_metadata().fingerprint;

        if !wait_until_loaded {
            return Ok(fingerprint);
        }

        let mut subscription = self.events().subscribe();
        let start_attempt = Worker::start_if_needed(worker.clone()).await?;
        if let Some(start_attempt) = start_attempt {
            match subscription
                .wait_for(|event| match event {
                    Event::WorkerLoaded {
                        agent_id,
                        start_attempt: event_attempt,
                        result,
                    } if agent_id == &owned_agent_id.agent_id
                        && event_attempt == &start_attempt =>
                    {
                        Some(result.clone())
                    }
                    _ => None,
                })
                .await
            {
                Ok(Ok(())) => Ok(fingerprint),
                Ok(Err(e)) => Err(e),
                Err(RecvError::Closed) => {
                    Err(WorkerExecutorError::unknown("Events subscription closed"))
                }
                Err(RecvError::Lagged(_)) => Err(WorkerExecutorError::unknown(
                    "Worker executor is overloaded and could not wait for worker to load",
                )),
            }
        } else {
            Ok(fingerprint)
        }
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
            &(&request, promise_id.clone()),
            |(_, r)| &r.agent_id,
            |(r, _)| &r.environment_id,
        )?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        let data = request.data;

        let promise_id: common_model::PromiseId = promise_id
            .clone()
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let completed = self.promise_service().complete(promise_id, data).await?;

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

        let principal = extract_principal(&request.principal);

        Worker::<Ctx>::delete(self, &owned_agent_id, principal).await
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
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?;

        let target_agent_id: AgentId = target_agent_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let owned_target_agent_id = OwnedAgentId::new(environment_id, &target_agent_id);

        let source_agent_id_proto = request
            .source_agent_id
            .clone()
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

        let principal = extract_principal(&request.principal);

        let resolved_revert = request.resolved_revert.map(|resolved| ResolvedRevert {
            last_oplog_index: OplogIndex::from_u64(resolved.last_oplog_index),
            observed_oplog_index: OplogIndex::from_u64(resolved.observed_oplog_index),
        });

        let target = request
            .target
            .ok_or(WorkerExecutorError::invalid_request("target not found"))?;

        let target = target
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        Worker::<Ctx>::revert(self, &owned_agent_id, target, resolved_revert, principal).await
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

        let principal = extract_principal(&request.principal);

        let idempotency_key = request
            .idempotency_key
            .ok_or(WorkerExecutorError::invalid_request(
                "idempotency_key not found",
            ))?
            .into();

        let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_agent_id)
            .await?
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_agent_id.agent_id(),
            ))?;

        let status = &metadata.last_known_status;
        if status
            .pending_invocations
            .iter()
            .any(|invocation| invocation.idempotency_key() == Some(&idempotency_key))
        {
            let worker = Worker::get_or_create_suspended(
                self,
                &owned_agent_id,
                None,
                Vec::new(),
                None,
                None,
                &InvocationContextStack::fresh(),
                principal,
            )
            .await?;
            worker.cancel_invocation(idempotency_key).await?;
            Ok(true)
        } else if status.invocation_results.contains_key(&idempotency_key) {
            Ok(false)
        } else if status.current_idempotency_key.as_ref() == Some(&idempotency_key)
            || status.invocation_results.is_exact_complete()
            || !status.invocation_results.might_contain(&idempotency_key)
        {
            Err(WorkerExecutorError::invalid_request("Invocation not found"))
        } else {
            let worker = Worker::get_or_create_suspended(
                self,
                &owned_agent_id,
                None,
                Vec::new(),
                None,
                None,
                &InvocationContextStack::fresh(),
                principal,
            )
            .await?;
            match worker.lookup_invocation_result(&idempotency_key).await {
                LookupResult::Complete(_) | LookupResult::Interrupted => Ok(false),
                LookupResult::Pending => {
                    let status = worker.get_last_known_status().await;
                    if status
                        .pending_invocations
                        .iter()
                        .any(|invocation| invocation.idempotency_key() == Some(&idempotency_key))
                    {
                        worker.cancel_invocation(idempotency_key).await?;
                        Ok(true)
                    } else {
                        Err(WorkerExecutorError::invalid_request("Invocation not found"))
                    }
                }
                LookupResult::New => {
                    Err(WorkerExecutorError::invalid_request("Invocation not found"))
                }
            }
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

        let principal = extract_principal(&request.principal);

        Worker::<Ctx>::interrupt(
            self,
            &owned_agent_id,
            request.recover_immediately,
            principal,
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

        let principal = extract_principal(&request.principal);

        let force_resume = request.force.unwrap_or(false);

        Worker::<Ctx>::resume(self, &owned_agent_id, force_resume, principal).await
    }

    pub(super) async fn update_worker_internal(
        &self,
        request: UpdateWorkerRequest,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        let principal = extract_principal(&request.principal);

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
            principal,
        )
        .await
    }
}
