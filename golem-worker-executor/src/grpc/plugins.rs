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
    AccountId, ActivatePluginRequest, AgentInvocation, AgentMetadataDto, AgentMetadataForGuests,
    AuthCtx, DeactivatePluginRequest, HasAll, IdempotencyKey, InvocationContextStack, OplogIndex,
    PluginPriority, Principal, ProcessOplogEntriesRequest, TypedAgentConfigEntry, UsesAllDeps,
    Worker, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, extract_owned_agent_id,
    extract_principal,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn activate_plugin_internal(
        &self,
        request: ActivatePluginRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let principal = extract_principal(&request.principal);

        let plugin_priority: PluginPriority = PluginPriority(request.plugin_priority);

        Worker::<Ctx>::activate_plugin(self, &owned_agent_id, plugin_priority, principal).await
    }

    pub(super) async fn deactivate_plugin_internal(
        &self,
        request: DeactivatePluginRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let principal = extract_principal(&request.principal);

        let plugin_priority = PluginPriority(request.plugin_priority);

        Worker::<Ctx>::deactivate_plugin(self, &owned_agent_id, plugin_priority, principal).await
    }

    pub(super) async fn process_oplog_entries_internal(
        &self,
        request: ProcessOplogEntriesRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {error}"))
            })?;
        auth_ctx
            .authorize_system_only("process oplog entries")
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;

        let component_revision: golem_common::model::component::ComponentRevision =
            request.component_revision.try_into().map_err(|e| {
                WorkerExecutorError::invalid_request(format!("invalid component_revision: {e}"))
            })?;

        let idempotency_key: IdempotencyKey = request
            .idempotency_key
            .ok_or(WorkerExecutorError::invalid_request(
                "idempotency_key not found",
            ))?
            .into();

        let account_id: AccountId = request
            .account_id
            .ok_or(WorkerExecutorError::invalid_request("account_id not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("invalid account_id: {e}"))
            })?;

        let config: Vec<(String, String)> = request.config.into_iter().collect();

        let metadata_dto: AgentMetadataDto = request
            .metadata
            .ok_or(WorkerExecutorError::invalid_request("metadata not found"))?
            .try_into()
            .map_err(|e: String| {
                WorkerExecutorError::invalid_request(format!("invalid metadata: {e}"))
            })?;

        let metadata = AgentMetadataForGuests {
            agent_id: metadata_dto.agent_id,
            args: vec![],
            env: metadata_dto.env.into_iter().collect(),
            config: TypedAgentConfigEntry::to_flat_map(&metadata_dto.config),
            status: metadata_dto.status,
            component_revision: metadata_dto.component_revision,
            retry_count: metadata_dto.retry_count as u64,
            environment_id: metadata_dto.environment_id,
        };

        let entries: Vec<golem_common::model::oplog::OplogEntry> = request
            .entries
            .into_iter()
            .map(golem_common::model::oplog::OplogEntry::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(WorkerExecutorError::invalid_request)?;

        let first_entry_index = OplogIndex::from_u64(request.first_entry_index);

        let worker = Worker::get_or_create_running(
            self,
            &owned_agent_id,
            None,
            Vec::new(),
            Some(component_revision),
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await?;

        let _result = worker
            .invoke(AgentInvocation::ProcessOplogEntries {
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            })
            .await?;

        Ok(())
    }
}
