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
    AgentMode, AgentStatusRecord, Arc, HasActiveAgents, HasAll, HasComponentService, Instrument,
    InvocationFreshnessDisposition, OwnedAgentId, ParsedAgentId, UsesAllDeps, Worker, WorkerCtx,
    WorkerExecutorImpl, info_span,
};
use golem_common::base_model::AgentId;
use golem_common::base_model::agent::Principal;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tracing::warn;

pub trait CanStartWorker {
    fn environment_id(&self) -> Result<EnvironmentId, WorkerExecutorError>;
    fn agent_id(&self) -> Result<AgentId, WorkerExecutorError>;
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn config(&self) -> Result<Vec<AgentConfigEntryDto>, WorkerExecutorError>;
    fn parent(&self) -> Option<AgentId>;
    fn maybe_invocation_context(&self) -> Option<InvocationContextStack> {
        None
    }
    fn principal(&self) -> Principal {
        Principal::anonymous()
    }
}

trait ProtobufInvocationDetails {
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId>;
    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId>;
    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext>;
    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &None
    }
    fn proto_config(&self) -> &[golem_api_grpc::proto::golem::worker::AgentConfigEntryDto] {
        &[]
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

    fn agent_id(&self) -> Result<AgentId, WorkerExecutorError> {
        let agent_id = self
            .proto_agent_id()
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?;

        AgentId::validate_length(&agent_id.name).map_err(WorkerExecutorError::invalid_request)?;

        agent_id
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)
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
                .and_then(|agent_id| agent_id.clone().try_into().ok())
        })
    }

    fn principal(&self) -> Principal {
        self.proto_principal()
            .as_ref()
            .and_then(|p| {
                let result: Result<Principal, String> = p.clone().try_into();
                result.ok()
            })
            .unwrap_or_else(Principal::anonymous)
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

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
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

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
    }
}

impl ProtobufInvocationDetails
    for golem_api_grpc::proto::golem::workerexecutor::v1::GetFileContentsRequest
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

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
    }
}

impl ProtobufInvocationDetails for golem_api_grpc::proto::golem::worker::InvocationStart {
    fn proto_agent_id(&self) -> &Option<golem_api_grpc::proto::golem::worker::AgentId> {
        &self.agent_id
    }

    fn proto_environment_id(&self) -> &Option<golem_api_grpc::proto::golem::common::EnvironmentId> {
        &self.environment_id
    }

    fn proto_invocation_context(
        &self,
    ) -> &Option<golem_api_grpc::proto::golem::worker::InvocationContext> {
        &self.context
    }

    fn proto_principal(&self) -> &Option<golem_api_grpc::proto::golem::component::Principal> {
        &self.principal
    }

    fn proto_config(&self) -> &[golem_api_grpc::proto::golem::worker::AgentConfigEntryDto] {
        &self.config
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

    pub(super) async fn get_or_create<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        let agent_id = request.agent_id()?;
        if golem_common::model::agent::OwnerKind::is_reserved_instance_name(&agent_id.agent_id) {
            self.ensure_worker_belongs_to_this_executor(&agent_id)?;
            let worker = Worker::get_exact_existing_suspended(
                self,
                &OwnedAgentId::new(request.environment_id()?, &agent_id),
                request.principal(),
            )
            .await?;
            Worker::start_if_needed(worker.clone()).await?;
            return Ok(worker);
        }
        self.get_or_create_with_freshness(request, InvocationFreshnessDisposition::MayExist)
            .await
    }

    pub(super) async fn get_or_create_with_freshness<Req: CanStartWorker>(
        &self,
        request: &Req,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        async {
            let (worker, _response_lease) = self
                .get_or_create_pending_with_freshness(request, freshness_disposition)
                .await?;
            Worker::start_if_needed(worker.clone()).await?;
            Ok(worker)
        }
        .instrument(info_span!("get_or_create_worker"))
        .await
    }

    pub(super) async fn get_or_create_pending_for_lookup<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<
        Option<(
            Arc<Worker<Ctx>>,
            Option<Arc<crate::worker::EphemeralResponseLease>>,
        )>,
        WorkerExecutorError,
    > {
        let agent_id = request.agent_id()?;
        let environment_id = request.environment_id()?;
        if golem_common::model::agent::OwnerKind::is_reserved_instance_name(&agent_id.agent_id) {
            self.ensure_worker_belongs_to_this_executor(&agent_id)?;
            let owner = OwnedAgentId::new(environment_id, &agent_id);
            if Worker::<Ctx>::get_latest_metadata(self, &owner)
                .await?
                .is_none()
            {
                return Ok(None);
            }
            return Worker::get_exact_existing_suspended_for_response(
                self,
                &owner,
                request.principal(),
            )
            .await
            .map(Some);
        }

        let owned_agent_id = self
            .canonicalize_owned_agent_id(&OwnedAgentId::new(environment_id, &agent_id))
            .await?;
        self.ensure_worker_belongs_to_this_executor(&agent_id)?;

        let component = self
            .component_service()
            .get_metadata(agent_id.component_id, None)
            .await?;
        let is_ephemeral =
            ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                .ok()
                .and_then(|parsed_agent_id| {
                    component
                        .metadata
                        .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
                })
                .is_some_and(|agent_type| agent_type.mode == AgentMode::Ephemeral);
        if is_ephemeral
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
            request.principal(),
            InvocationFreshnessDisposition::MayExist,
        )
        .await
        .map(Some)
    }

    pub(super) async fn get_or_create_pending_with_freshness<Req: CanStartWorker>(
        &self,
        request: &Req,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<
        (
            Arc<Worker<Ctx>>,
            Option<Arc<crate::worker::EphemeralResponseLease>>,
        ),
        WorkerExecutorError,
    > {
        let agent_id = request.agent_id()?;
        let environment_id = request.environment_id()?;

        let owned_agent_id = self
            .canonicalize_owned_agent_id(&OwnedAgentId::new(environment_id, &agent_id))
            .await?;
        self.ensure_worker_belongs_to_this_executor(&agent_id)?;

        // This runs on every invocation. For a resident agent, read the status the worker already
        // publishes; only an agent that is not resident has its record materialised (from the
        // cache and the oplog), which is the same cold path as before.
        let failure_check = if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
            None
        } else if let Some(worker) = self.active_agents().try_get(&owned_agent_id).await {
            if worker.pending_startup_attempt().is_some() {
                None
            } else {
                Some((worker.agent_mode(), worker.get_last_known_status().await))
            }
        } else {
            Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
                .await?
                .map(|metadata| (metadata.agent_mode, Arc::new(metadata.last_known_status)))
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
            request.principal(),
            freshness_disposition,
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
}
