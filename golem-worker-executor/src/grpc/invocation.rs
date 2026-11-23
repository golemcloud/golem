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
    AgentMode, AgentStatusRecord, Arc, HasActiveAgents, HasAll, Instrument,
    InvocationFreshnessDisposition, OwnedAgentId, ParsedAgentKey, UsesAllDeps, Worker, WorkerCtx,
    WorkerExecutorImpl, info_span,
};
use crate::worker::CreateIntent;
use crate::worker::target::ResolvedTarget;
use golem_common::base_model::{AgentId, IdempotencyKey, RESERVED_TYPE_TAG};
use golem_common::model::AgentFingerprint;
use golem_common::model::agent::{AgentConfigEntryDto, AgentTarget};
use golem_common::model::caller::Caller;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tracing::warn;

pub trait CanStartWorker {
    fn environment_id(&self) -> Result<EnvironmentId, WorkerExecutorError>;
    fn agent_target(&self) -> Result<AgentTarget, WorkerExecutorError>;
    /// The type and constructor parameters of an agent that an ID target creates when it is
    /// missing.
    fn constructor(&self) -> Result<Option<ParsedAgentKey>, WorkerExecutorError> {
        Ok(None)
    }
    /// The component that implements the agent type, for a request that may create.
    fn component_id(&self) -> Result<Option<ComponentId>, WorkerExecutorError> {
        Ok(None)
    }
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn config(&self) -> Result<Vec<AgentConfigEntryDto>, WorkerExecutorError>;
    fn parent(&self) -> Option<AgentId>;
    fn maybe_invocation_context(&self) -> Option<InvocationContextStack> {
        None
    }
    /// The caller of the request. A request without one is refused.
    fn caller(&self) -> Result<Caller, WorkerExecutorError>;
    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(None)
    }
}

trait ProtobufInvocationDetails {
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId>;
    fn proto_agent_target(&self) -> Result<AgentTarget, WorkerExecutorError> {
        let agent_id = (*self.proto_agent_id())
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?;
        AgentId::try_from(agent_id)
            .map(AgentTarget::Id)
            .map_err(WorkerExecutorError::invalid_request)
    }
    fn proto_constructor(&self) -> Result<Option<ParsedAgentKey>, WorkerExecutorError> {
        Ok(None)
    }
    fn proto_component_id(&self) -> Result<Option<ComponentId>, WorkerExecutorError> {
        Ok(None)
    }
    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId>;
    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext>;
    fn proto_caller(&self) -> &Option<golem_api_grpc::proto::golem::component::Caller>;
    /// The decoded caller. A request without one is refused.
    fn decoded_caller(&self) -> Result<Caller, WorkerExecutorError> {
        super::extract_caller(self.proto_caller())
    }
    fn proto_config(&self) -> &[golem_api_grpc::proto::golem::worker::AgentConfigEntryDto] {
        &[]
    }
    fn proto_idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(None)
    }
}

impl<T: ProtobufInvocationDetails> CanStartWorker for T {
    fn environment_id(&self) -> Result<EnvironmentId, WorkerExecutorError> {
        (*self.proto_environment_id())
            .ok_or(WorkerExecutorError::invalid_request(
                "environment_id not found",
            ))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
    }

    fn agent_target(&self) -> Result<AgentTarget, WorkerExecutorError> {
        self.proto_agent_target()
    }

    fn constructor(&self) -> Result<Option<ParsedAgentKey>, WorkerExecutorError> {
        self.proto_constructor()
    }

    fn component_id(&self) -> Result<Option<ComponentId>, WorkerExecutorError> {
        self.proto_component_id()
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        self.proto_invocation_context()
            .as_ref()
            .map(|ctx| ctx.env.clone().into_iter().collect::<Vec<_>>())
    }

    fn config(&self) -> Result<Vec<AgentConfigEntryDto>, WorkerExecutorError> {
        self.proto_config()
            .iter()
            .cloned()
            .map(|entry| {
                entry
                    .try_into()
                    .map_err(WorkerExecutorError::invalid_request)
            })
            .collect()
    }

    fn parent(&self) -> Option<AgentId> {
        self.proto_invocation_context().as_ref().and_then(|ctx| {
            ctx.parent
                .as_ref()
                .and_then(|agent_id| (*agent_id).try_into().ok())
        })
    }

    fn caller(&self) -> Result<Caller, WorkerExecutorError> {
        self.decoded_caller()
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        self.proto_idempotency_key()
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetFileSystemNodeRequest
{
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId> {
        &self.agent_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }

    fn proto_caller(&self) -> &Option<golem_api_grpc::proto::golem::component::Caller> {
        &self.caller
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetAgentWalletRequest
{
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId> {
        &self.agent_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }

    fn proto_caller(&self) -> &Option<golem_api_grpc::proto::golem::component::Caller> {
        &self.caller
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetFileContentsRequest
{
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId> {
        &None
    }

    fn proto_agent_target(&self) -> Result<AgentTarget, WorkerExecutorError> {
        self.target
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("target not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
    }

    fn proto_component_id(&self) -> Result<Option<ComponentId>, WorkerExecutorError> {
        self.component_id
            .map(|id| id.try_into().map_err(WorkerExecutorError::invalid_request))
            .transpose()
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }

    fn proto_caller(&self) -> &Option<golem_api_grpc::proto::golem::component::Caller> {
        &self.caller
    }
}

impl ProtobufInvocationDetails for golem_api_grpc::proto::golem::worker::InvocationStart {
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId> {
        &None
    }

    fn proto_agent_target(&self) -> Result<AgentTarget, WorkerExecutorError> {
        self.target
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("target not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
    }

    fn proto_constructor(&self) -> Result<Option<ParsedAgentKey>, WorkerExecutorError> {
        self.constructor
            .clone()
            .map(|key| ParsedAgentKey::try_from(key).map_err(WorkerExecutorError::invalid_request))
            .transpose()
    }

    fn proto_component_id(&self) -> Result<Option<ComponentId>, WorkerExecutorError> {
        self.component_id
            .map(|id| id.try_into().map_err(WorkerExecutorError::invalid_request))
            .transpose()
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }

    fn proto_caller(&self) -> &Option<golem_api_grpc::proto::golem::component::Caller> {
        &self.caller
    }

    fn proto_config(&self) -> &[golem_api_grpc::proto::golem::worker::AgentConfigEntryDto] {
        &self.config
    }

    fn proto_idempotency_key(&self) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        let request_key: golem_common::model::InvocationRequestKey = self
            .request_key
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("request_key is required"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let method_name = self
            .method_name
            .as_deref()
            .ok_or_else(|| WorkerExecutorError::invalid_request("method_name is required"))?;
        Ok(Some(request_key.owner_key(method_name)))
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::DeliverCardTransferRequest
{
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId> {
        &self.target_agent_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &None
    }

    fn proto_caller(&self) -> &Option<golem_api_grpc::proto::golem::component::Caller> {
        &None
    }

    /// A card transfer is delivered by the executor of the sending agent and carries no caller
    /// field, so it is explicitly anonymous.
    fn decoded_caller(&self) -> Result<Caller, WorkerExecutorError> {
        Ok(Caller::anonymous())
    }
}

pub fn from_proto_invocation_context(
    context: &Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
) -> InvocationContextStack {
    let provided_context = context.as_ref().and_then(|context| {
        context.tracing.as_ref().and_then(|tracing_context| {
            let result: Result<InvocationContextStack, String> = tracing_context.clone().try_into();
            if let Err(err) = &result {
                warn!("Failed to parse tracing context: {}", err);
            }
            result.ok()
        })
    });
    provided_context.unwrap_or_else(InvocationContextStack::fresh)
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) fn limit_invocation_context_stack_depth(
        &self,
        invocation_context: InvocationContextStack,
    ) -> InvocationContextStack {
        invocation_context.limit_depth(
            self.services
                .config()
                .limits
                .max_invocation_context_stack_depth,
        )
    }

    /// Takes the agent's latest status record rather than its whole metadata, so the invoke path
    /// can hand over the status the resident agent already publishes instead of materialising an
    /// [`golem_common::model::AgentMetadata`] around a copy of it.
    pub(super) async fn ensure_not_failed(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        status: &AgentStatusRecord,
    ) -> Result<(), WorkerExecutorError> {
        Worker::<Ctx>::ensure_not_failed(self, owned_agent_id, agent_mode, status).await
    }

    /// Gives `AgentNotFound` when a delete is running on the agent's cached generation or has
    /// removed its durable state, so the read and management paths answer it like a missing
    /// agent. A failed delete that left the agent in place keeps its deletion error.
    pub(super) async fn ensure_not_being_deleted(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<Arc<Worker<Ctx>>>, WorkerExecutorError> {
        match self.active_agents().try_get(owned_agent_id).await {
            Some(worker) if worker.answers_as_deleted().await => Err(
                WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()),
            ),
            resident => Ok(resident),
        }
    }

    /// Gives `AgentNotFound` instead of the failure of an operation on a worker that a delete
    /// claimed after [`Self::ensure_not_being_deleted`] passed, under the same rule.
    pub(super) async fn not_found_if_deleting<T>(
        worker: &Worker<Ctx>,
        result: Result<T, WorkerExecutorError>,
    ) -> Result<T, WorkerExecutorError> {
        match result {
            Err(_) if worker.answers_as_deleted().await => Err(
                WorkerExecutorError::worker_not_found(worker.owned_agent_id().agent_id()),
            ),
            result => result,
        }
    }

    /// Resolves an existing component agent for `operation` without starting it. A missing agent,
    /// or one that a running delete has claimed, gives `AgentNotFound`.
    pub(super) async fn get_existing_for_management(
        &self,
        owned_agent_id: &OwnedAgentId,
        caller: Caller,
        operation: &str,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        let worker = Worker::get_exact_existing_suspended(self, owned_agent_id, caller).await?;
        worker.ensure_component_agent_ingress(operation)?;
        if worker.answers_as_deleted().await {
            return Err(WorkerExecutorError::worker_not_found(
                owned_agent_id.agent_id(),
            ));
        }
        Ok(worker)
    }

    /// Resolves an agent that already exists and starts it. This never creates an agent: a
    /// missing agent, or one that a running delete has claimed, gives `AgentNotFound`. It also
    /// refuses to make an inspection the first start of an ephemeral agent.
    pub(super) async fn get_existing_running<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        // The caller is decoded first, so a missing caller always gives the same error.
        request.caller()?;
        async {
            let resolved = self.resolve_request_target(request).await?;
            let owned_agent_id = resolved.owned_agent_id.clone();
            let reserved = owned_agent_id.agent_id.type_tag() == RESERVED_TYPE_TAG;
            self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
            // The status comes from `get_latest_metadata`, which stays safe when a delete has
            // already stopped the resident worker's actors, and which rejects a failed agent
            // without loading it.
            let pending_startup = match self.ensure_not_being_deleted(&owned_agent_id).await? {
                Some(worker) => worker.pending_startup_attempt().is_some(),
                None => false,
            };
            if !reserved && !pending_startup {
                let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
                    .await?
                    .ok_or_else(|| {
                        WorkerExecutorError::worker_not_found(owned_agent_id.agent_id())
                    })?;
                if let Some(owner) = &resolved.new_owner {
                    owner.ensure_owns_key(
                        owned_agent_id.agent_id,
                        metadata.agent_mode,
                        metadata.canonical_key.as_deref(),
                    )?;
                }
                if metadata.agent_mode != AgentMode::Ephemeral {
                    self.ensure_not_failed(
                        &owned_agent_id,
                        metadata.agent_mode,
                        &metadata.last_known_status,
                    )
                    .await?;
                }
            }
            let worker =
                Worker::get_exact_existing_suspended(self, &owned_agent_id, request.caller()?)
                    .await?;
            if !reserved {
                Self::not_found_if_deleting(
                    &worker,
                    worker
                        .ensure_inspection_may_start(resolved.new_owner.as_ref())
                        .await,
                )
                .await?;
            }
            Self::not_found_if_deleting(&worker, Worker::start_if_needed(worker.clone()).await)
                .await?;
            Ok(worker)
        }
        .instrument(info_span!("get_existing_worker"))
        .await
    }

    /// With an expected fingerprint, only that existing incarnation is looked up: a missing agent
    /// or another incarnation is `AgentNotFound`, and no agent is created.
    pub(super) async fn get_or_create_pending_for_lookup<Req: CanStartWorker>(
        &self,
        request: &Req,
        resolved: &ResolvedTarget,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> Result<
        Option<(
            Arc<Worker<Ctx>>,
            Option<Arc<crate::worker::EphemeralResponseLease>>,
        )>,
        WorkerExecutorError,
    > {
        let owned_agent_id = resolved.owned_agent_id.clone();
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        if expected_fingerprint.is_some() {
            let response = Worker::get_exact_existing_suspended_for_response(
                self,
                &owned_agent_id,
                request.caller()?,
                expected_fingerprint,
            )
            .await?;
            response.0.ensure_owns_key(resolved.new_owner.as_ref())?;
            return Ok(Some(response));
        }

        // A lookup creates only a keyed durable agent. Any other missing agent has no result
        // to look up.
        let creates = resolved.new_owner.as_ref().is_some_and(|owner| owner.keyed)
            && resolved.agent_mode == Some(AgentMode::Durable);
        if !creates
            && Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
                .await?
                .is_none()
        {
            return Ok(None);
        }

        let invocation_context = request
            .maybe_invocation_context()
            .unwrap_or_else(InvocationContextStack::fresh);
        let invocation_context = self.limit_invocation_context_stack_depth(invocation_context);

        // Lookup must be able to observe the current idempotency state even while the
        // invocation is still retrying and has transient error entries in the oplog.
        Worker::get_or_create_suspended_for_response(
            self,
            &owned_agent_id,
            request.env(),
            request.config()?,
            None,
            request.parent(),
            &invocation_context,
            request.caller()?,
            InvocationFreshnessDisposition::MayExist,
            if creates {
                resolved.invocation_intent(None)
            } else {
                CreateIntent::Existing
            },
        )
        .await
        .map(Some)
    }

    /// With an expected fingerprint, only that existing incarnation is acquired: a missing agent
    /// or another incarnation is `AgentNotFound`, and no agent is created.
    pub(super) async fn get_or_create_pending_with_freshness<Req: CanStartWorker>(
        &self,
        request: &Req,
        resolved: &ResolvedTarget,
        freshness_disposition: InvocationFreshnessDisposition,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> Result<
        (
            Arc<Worker<Ctx>>,
            Option<Arc<crate::worker::EphemeralResponseLease>>,
        ),
        WorkerExecutorError,
    > {
        let owned_agent_id = resolved.owned_agent_id.clone();
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        if let Some(expected) = expected_fingerprint {
            // The incarnation is pinned before its status is read, so the failure of a re-created
            // agent never answers a caller that named the deleted incarnation.
            let (worker, response_lease) = Worker::get_exact_existing_suspended_for_response(
                self,
                &owned_agent_id,
                request.caller()?,
                Some(expected),
            )
            .await?;
            worker.ensure_owns_key(resolved.new_owner.as_ref())?;
            if worker.agent_mode() != AgentMode::Ephemeral
                && worker.pending_startup_attempt().is_none()
            {
                let status = worker
                    .deleted_incarnation_as_not_found(
                        Some(expected),
                        worker.try_get_last_known_status().await,
                    )
                    .await?;
                self.ensure_not_failed(&owned_agent_id, worker.agent_mode(), &status)
                    .await?;
            }
            return Ok((worker, response_lease));
        }

        // This runs on every invocation. For a resident agent, read the status the worker already
        // publishes; only an agent that is not resident has its record materialised (from the
        // cache and the oplog), which is the same cold path as before.
        let failure_check = if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
            None
        } else if let Some(worker) = self.active_agents().try_get(&owned_agent_id).await {
            worker.ensure_owns_key(resolved.new_owner.as_ref())?;
            if worker.pending_startup_attempt().is_some() {
                None
            } else {
                Some((
                    worker.agent_mode(),
                    worker.try_get_last_known_status().await?,
                ))
            }
        } else {
            Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
                .await?
                .map(|metadata| -> Result<_, WorkerExecutorError> {
                    if let Some(owner) = &resolved.new_owner {
                        owner.ensure_owns_key(
                            owned_agent_id.agent_id,
                            metadata.agent_mode,
                            metadata.canonical_key.as_deref(),
                        )?;
                    }
                    Ok((metadata.agent_mode, Arc::new(metadata.last_known_status)))
                })
                .transpose()?
        };

        if let Some((agent_mode, status)) = &failure_check
            && *agent_mode != AgentMode::Ephemeral
        {
            self.ensure_not_failed(&owned_agent_id, *agent_mode, status)
                .await?;
        }

        let invocation_context = request
            .maybe_invocation_context()
            .unwrap_or_else(InvocationContextStack::fresh);
        let invocation_context = self.limit_invocation_context_stack_depth(invocation_context);

        Worker::get_or_create_suspended_for_response(
            self,
            &owned_agent_id,
            request.env(),
            request.config()?,
            None,
            request.parent(),
            &invocation_context,
            request.caller()?,
            freshness_disposition,
            resolved.invocation_intent(request.idempotency_key()?),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::CanStartWorker;
    use test_r::test;

    #[test]
    fn invoke_agent_request_decodes_creation_config() {
        let request = golem_api_grpc::proto::golem::worker::InvocationStart {
            config: vec![golem_api_grpc::proto::golem::worker::AgentConfigEntryDto {
                path: vec!["database".into(), "port".into()],
                value: "5432".into(),
            }],
            ..Default::default()
        };

        let config = request.config().unwrap();
        assert_eq!(config.len(), 1);
        assert_eq!(config[0].path, vec!["database", "port"]);
        assert_eq!(config[0].value.to_string(), "5432");
    }

    #[test]
    fn invoke_agent_request_rejects_invalid_creation_config_json() {
        let request = golem_api_grpc::proto::golem::worker::InvocationStart {
            config: vec![golem_api_grpc::proto::golem::worker::AgentConfigEntryDto {
                path: vec!["database".into(), "port".into()],
                value: "not-json".into(),
            }],
            ..Default::default()
        };

        assert!(request.config().is_err());
    }

    // The executor checks a KnownFresh ephemeral ID against the create key of the invocation,
    // and invocation-session clients derive that ID from `request_key.owner_key(method)`, not from
    // the raw key the public API also sends.
    #[test]
    fn invocation_start_create_key_is_the_owner_key_its_ephemeral_id_derives_from() {
        use golem_common::model::{IdempotencyKey, IdempotencyScope, InvocationRequestKey};

        let method = "handle";
        let raw = IdempotencyKey::new("user-key".to_string());
        let cases = [
            // Raw HTTP handler / OpenAPI provider: request_key only.
            (InvocationRequestKey::external(None, None), None),
            // Public invocation API: scoped request_key plus the raw user key.
            (
                InvocationRequestKey::external(
                    Some(IdempotencyScope::Pair {
                        issuer: "https://issuer.example".to_string(),
                        subject: "caller".to_string(),
                    }),
                    Some(&raw),
                ),
                Some(raw.clone()),
            ),
        ];
        for (request_key, raw) in cases {
            let request = golem_api_grpc::proto::golem::worker::InvocationStart {
                method_name: Some(method.to_string()),
                request_key: Some(request_key.clone().into()),
                idempotency_key: raw.map(Into::into),
                ..Default::default()
            };
            assert_eq!(
                request.idempotency_key().unwrap(),
                Some(request_key.owner_key(method)),
                "create key for {request_key:?} is not its owner key"
            );
        }
    }
}
