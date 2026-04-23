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

use super::WorkerResult;
use super::{ConnectWorkerStream, WorkerClient, WorkerServiceError};
use crate::api::agents::{
    AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult, CreateAgentRequest,
    CreateAgentResponse,
};
use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::auth::AuthService;
use crate::service::component::ComponentService;
use crate::service::limit::LimitService;
use bytes::Bytes;
use futures::Stream;
use golem_api_grpc::proto::golem::worker::InvocationContext;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentMode, AgentTypeName, DataValue, GolemUserPrincipal, ParsedAgentId, Principal,
    UntypedDataValue,
};
use golem_common::model::component::{
    CanonicalFilePath, ComponentId, ComponentRevision, PluginPriority,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogCursor;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::worker::AgentUpdateMode;
use golem_common::model::worker::{AgentMetadataDto, RevertWorkerTarget};
use golem_common::model::{AgentFilter, AgentId, IdempotencyKey, ScanCursor};
use golem_service_base::model::auth::{AuthCtx, EnvironmentAction};
use golem_service_base::model::component::Component;
use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};

fn build_public_agent_id(
    component_id: ComponentId,
    agent_type_name: AgentTypeName,
    constructor_parameters: DataValue,
    phantom_id: Option<uuid::Uuid>,
    agent_mode: AgentMode,
) -> WorkerResult<AgentId> {
    let agent_id = ParsedAgentId::new_auto_phantom(
        agent_type_name,
        constructor_parameters,
        phantom_id,
        agent_mode,
    )
    .map_err(|err| WorkerServiceError::TypeChecker(format!("Agent ID formatting error: {err}")))?;

    Ok(AgentId {
        component_id,
        agent_id: agent_id.to_string(),
    })
}

fn required_environment_action_for_invocation_mode(mode: i32) -> EnvironmentAction {
    if mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32 {
        EnvironmentAction::ViewWorker
    } else {
        EnvironmentAction::UpdateWorker
    }
}

pub struct WorkerService {
    component_service: Arc<dyn ComponentService>,
    auth_service: Arc<dyn AuthService>,
    limit_service: Arc<dyn LimitService>,
    worker_client: Arc<dyn WorkerClient>,
    agent_resolution_cache: Arc<AgentResolutionCache>,
}

impl WorkerService {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        auth_service: Arc<dyn AuthService>,
        limit_service: Arc<dyn LimitService>,
        worker_client: Arc<dyn WorkerClient>,
        agent_resolution_cache: Arc<AgentResolutionCache>,
    ) -> Self {
        Self {
            component_service,
            auth_service,
            limit_service,
            worker_client,
            agent_resolution_cache,
        }
    }

    pub async fn create(
        &self,
        agent_id: &AgentId,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<ComponentRevision> {
        let component = self
            .component_service
            .get_latest_by_id_uncached(agent_id.component_id)
            .await?;

        self.create_with_component(
            agent_id,
            component,
            environment_variables,
            config,
            ignore_already_existing,
            auth_ctx,
            invocation_context,
            principal,
        )
        .await
    }

    // Like create, but skip fetching the component.
    pub async fn create_with_component(
        &self,
        agent_id: &AgentId,
        component: Component,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<ComponentRevision> {
        assert!(component.id == agent_id.component_id);

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::CreateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .create(
                agent_id,
                environment_variables,
                config,
                ignore_already_existing,
                environment_auth_details.account_id_owning_environment,
                component.environment_id,
                auth_ctx,
                invocation_context,
                principal,
            )
            .await?;

        Ok(component.revision)
    }

    pub async fn connect(
        &self,
        agent_id: &AgentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let stream = self
            .worker_client
            .connect(
                agent_id,
                component.environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
            )
            .await?;

        self.limit_service
            .update_worker_connection_limit(
                environment_auth_details.account_id_owning_environment,
                agent_id,
                true,
            )
            .await?;

        Ok(ConnectWorkerStream::new(
            stream,
            agent_id.clone(),
            environment_auth_details.account_id_owning_environment,
            self.limit_service.clone(),
        ))
    }

    pub async fn delete(&self, agent_id: &AgentId, auth_ctx: AuthCtx) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::DeleteWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .delete(agent_id, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn complete_promise(
        &self,
        agent_id: &AgentId,
        oplog_id: u64,
        data: Vec<u8>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .complete_promise(agent_id, oplog_id, data, component.environment_id, auth_ctx)
            .await?;

        Ok(result)
    }

    pub async fn interrupt(
        &self,
        agent_id: &AgentId,
        recover_immediately: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .interrupt(
                agent_id,
                recover_immediately,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn get_metadata(
        &self,
        agent_id: &AgentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentMetadataDto> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .get_metadata(agent_id, component.environment_id, auth_ctx)
            .await?;

        Ok(result)
    }

    pub async fn find_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
        let component = self
            .component_service
            .get_latest_by_id(component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .find_metadata(
                component_id,
                filter,
                cursor,
                count,
                precise,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn resume(
        &self,
        agent_id: &AgentId,
        force: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .resume(agent_id, force, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn update(
        &self,
        agent_id: &AgentId,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .update(
                agent_id,
                update_mode,
                target_revision,
                disable_wakeup,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn get_oplog(
        &self,
        agent_id: &AgentId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .get_oplog(
                agent_id,
                from_oplog_index,
                cursor,
                count,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn search_oplog(
        &self,
        agent_id: &AgentId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .search_oplog(
                agent_id,
                cursor,
                count,
                query,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn get_file_system_node(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let nodes = self
            .worker_client
            .get_file_system_node(
                agent_id,
                path,
                component.environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
            )
            .await?;

        Ok(nodes)
    }

    pub async fn get_file_contents(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let contents_stream = self
            .worker_client
            .get_file_contents(
                agent_id,
                path,
                component.environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
            )
            .await?;

        Ok(contents_stream)
    }

    pub async fn activate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .activate_plugin(
                agent_id,
                plugin_priority,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn deactivate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .deactivate_plugin(
                agent_id,
                plugin_priority,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(source_agent_id.component_id)
            .await?;

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .fork_worker(
                source_agent_id,
                target_agent_id,
                oplog_index_cut_off,
                component.environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn revert_worker(
        &self,
        agent_id: &AgentId,
        target: RevertWorkerTarget,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .revert_worker(agent_id, target, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let component = self
            .component_service
            .get_latest_by_id(agent_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        let canceled = self
            .worker_client
            .cancel_invocation(
                agent_id,
                idempotency_key,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(canceled)
    }

    pub async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_revision: ComponentRevision,
        idempotency_key: IdempotencyKey,
        _account_id: AccountId,
        config: std::collections::HashMap<String, String>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .process_oplog_entries(
                target_agent_id,
                environment_id,
                component_revision,
                idempotency_key,
                environment_auth_details.account_id_owning_environment,
                config,
                metadata,
                first_entry_index,
                entries,
                auth_ctx,
            )
            .await
    }

    pub async fn invoke_agent(
        &self,
        agent_id: &AgentId,
        method_name: Option<String>,
        method_parameters: Option<golem_api_grpc::proto::golem::component::UntypedDataValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
        known_environment_id: Option<EnvironmentId>,
    ) -> WorkerResult<AgentInvocationOutput> {
        let environment_id = match known_environment_id {
            Some(id) => id,
            None => {
                self.component_service
                    .get_latest_by_id(agent_id.component_id)
                    .await?
                    .environment_id
            }
        };

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                environment_id,
                required_environment_action_for_invocation_mode(mode),
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .invoke_agent(
                agent_id,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                idempotency_key,
                invocation_context,
                environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
                principal,
            )
            .await
    }

    /// REST/JSON path: resolves agent via registry, converts JSON parameters, then creates the agent.
    pub async fn create_agent_rest(
        &self,
        request: CreateAgentRequest,
        auth: AuthCtx,
    ) -> WorkerResult<CreateAgentResponse> {
        let resolved = self
            .agent_resolution_cache
            .resolve(
                &request.app_name,
                &request.env_name,
                &request.agent_type_name,
                None,
                &auth,
            )
            .await?;

        let registered_agent_type = resolved.registered_agent_type;
        let _environment_id = resolved.environment_id;
        let component_id = registered_agent_type.implemented_by.component_id;
        let agent_type = &registered_agent_type.agent_type;

        let constructor_parameters: DataValue = DataValue::try_from_untyped_json(
            request.parameters,
            agent_type.constructor.input_schema.clone(),
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;

        let agent_id = build_public_agent_id(
            component_id,
            request.agent_type_name.clone(),
            constructor_parameters,
            request.phantom_id,
            agent_type.mode,
        )?;

        let component = self
            .component_service
            .get_revision(
                component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;

        let component_revision = self
            .create_with_component(
                &agent_id,
                component,
                HashMap::new(),
                request.config,
                true,
                auth,
                None,
                None,
            )
            .await?;

        Ok(CreateAgentResponse {
            agent_id,
            component_revision,
        })
    }

    /// REST/JSON path: resolves agent via registry, converts JSON parameters, then delegates.
    pub async fn invoke_agent_rest(
        &self,
        request: AgentInvocationRequest,
        auth: AuthCtx,
    ) -> WorkerResult<AgentInvocationResult> {
        let deployment_revision = request
            .deployment_revision
            .map(|rev| {
                let rev_u64 = u64::try_from(rev).map_err(|_| {
                    WorkerServiceError::Internal(format!(
                        "Invalid deployment revision (must be non-negative): {rev}"
                    ))
                })?;
                DeploymentRevision::new(rev_u64).map_err(|e| {
                    WorkerServiceError::Internal(format!("Invalid deployment revision: {e}"))
                })
            })
            .transpose()?;

        let resolved = match deployment_revision {
            None => {
                self.agent_resolution_cache
                    .resolve(
                        &request.app_name,
                        &request.env_name,
                        &request.agent_type_name,
                        request.owner_account_email.as_deref(),
                        &auth,
                    )
                    .await?
            }
            Some(rev) => {
                self.agent_resolution_cache
                    .resolve_pinned(
                        &request.app_name,
                        &request.env_name,
                        &request.agent_type_name,
                        rev,
                        request.owner_account_email.as_deref(),
                        &auth,
                    )
                    .await?
            }
        };

        let registered_agent_type = resolved.registered_agent_type;
        let environment_id = resolved.environment_id;
        let component_id = registered_agent_type.implemented_by.component_id;
        let agent_type = &registered_agent_type.agent_type;

        let constructor_parameters: DataValue = DataValue::try_from_untyped_json(
            request.parameters,
            agent_type.constructor.input_schema.clone(),
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;

        let agent_id = build_public_agent_id(
            component_id,
            request.agent_type_name.clone(),
            constructor_parameters,
            request.phantom_id,
            agent_type.mode,
        )?;

        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == request.method_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent method {} not found in agent type {}",
                    request.method_name, request.agent_type_name
                ))
            })?;

        let method_parameters: DataValue = DataValue::try_from_untyped_json(
            request.method_parameters,
            method.input_schema.clone(),
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!("Agent method parameters type error: {err}"))
        })?;

        let proto_method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue =
            UntypedDataValue::from(method_parameters).into();

        let proto_mode = match request.mode {
            AgentInvocationMode::Await => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32
            }
        };

        let proto_schedule_at = request.schedule_at.map(|dt| ::prost_types::Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        });

        let principal: golem_api_grpc::proto::golem::component::Principal =
            Principal::GolemUser(GolemUserPrincipal {
                account_id: auth.account_id(),
            })
            .into();

        let method_name = request.method_name.clone();
        let agent_type_name = request.agent_type_name.clone();

        let output = self
            .invoke_agent(
                &agent_id,
                Some(request.method_name),
                Some(proto_method_parameters),
                proto_mode,
                proto_schedule_at,
                request.idempotency_key,
                None,
                auth,
                principal,
                Some(environment_id),
            )
            .await?;

        match output.result {
            golem_common::model::AgentInvocationResult::AgentMethod {
                output: untyped_data_value,
            } => {
                let decode_revision = output
                    .component_revision
                    .unwrap_or(registered_agent_type.implemented_by.component_revision);
                let component_metadata_for_decode = self
                    .component_service
                    .get_revision(component_id, decode_revision)
                    .await?;
                let decode_agent_type = component_metadata_for_decode
                    .metadata
                    .find_agent_type_by_name(&agent_type_name)
                    .ok_or_else(|| {
                        WorkerServiceError::Internal(format!(
                            "Agent type {agent_type_name} not found in component metadata at revision {decode_revision}",
                        ))
                    })?;
                let decode_method = decode_agent_type
                    .methods
                    .iter()
                    .find(|m| m.name == method_name)
                    .ok_or_else(|| {
                        WorkerServiceError::Internal(format!(
                            "Agent method {method_name} not found in agent type {agent_type_name} at revision {decode_revision}",
                        ))
                    })?;
                let typed_data_value = DataValue::try_from_untyped(
                    untyped_data_value,
                    decode_method.output_schema.clone(),
                )
                .map_err(|err| {
                    WorkerServiceError::TypeChecker(format!("DataValue conversion error: {err}"))
                })?;
                Ok(AgentInvocationResult {
                    agent_id: agent_id.clone(),
                    result: Some(typed_data_value.into()),
                    component_revision: Some(decode_revision),
                })
            }
            _ => Ok(AgentInvocationResult {
                agent_id,
                result: None,
                component_revision: output.component_revision,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkerService, required_environment_action_for_invocation_mode};
    use crate::api::agents::{AgentInvocationMode, AgentInvocationRequest, CreateAgentRequest};
    use crate::service::agent_resolution_cache::AgentResolutionCache;
    use crate::service::auth::{AuthService, AuthServiceError};
    use crate::service::component::{ComponentService, ComponentServiceError};
    use crate::service::limit::{LimitService, LimitServiceError};
    use crate::service::worker::{WorkerClient, WorkerResult, WorkerStream};
    use async_trait::async_trait;
    use bytes::Bytes;
    use chrono::Utc;
    use futures::Stream;
    use golem_api_grpc::proto::golem::worker::{InvocationContext, LogEvent};
    use golem_common::base_model::component_metadata::KnownExports;
    use golem_common::model::AgentInvocationOutput;
    use golem_common::model::Empty;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{
        AgentConstructor, AgentMethod, AgentMode, AgentType, AgentTypeName, DataSchema,
        HttpEndpointDetails, NamedElementSchemas, RegisteredAgentType,
        RegisteredAgentTypeImplementer, ResolvedAgentType, Snapshotting, UntypedJsonDataValue,
    };
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::auth::EnvironmentRole;
    use golem_common::model::component::{
        CanonicalFilePath, ComponentId, ComponentName, ComponentRevision, PluginPriority,
    };
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use golem_common::model::diff::Hash;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::oplog::{OplogCursor, OplogIndex};
    use golem_common::model::worker::{AgentConfigEntryDto, AgentMetadataDto, RevertWorkerTarget};
    use golem_common::model::{AgentFilter, AgentId, IdempotencyKey, ScanCursor};
    use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
    use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment, EnvironmentAction};
    use golem_service_base::model::component::Component;
    use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use test_r::test;
    use uuid::Uuid;

    #[derive(Clone)]
    struct TestRegistryService {
        resolved: ResolvedAgentType,
    }

    #[async_trait]
    impl RegistryService for TestRegistryService {
        async fn authenticate_token(
            &self,
            _: &golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_auth_details_for_environment(
            &self,
            _: EnvironmentId,
            _: bool,
            _: &AuthCtx,
        ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _: AccountId,
        ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_connection_limit(
            &self,
            _: AccountId,
            _: &golem_common::base_model::AgentId,
            _: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_resource_usage(
            &self,
            _: HashMap<AccountId, golem_service_base::clients::registry::ResourceUsageUpdate>,
        ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn download_component(
            &self,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_component_metadata(
            &self,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_deployed_component_metadata(
            &self,
            _: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_deployed_component_revisions(
            &self,
            _: ComponentId,
        ) -> Result<Vec<Component>, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_component(
            &self,
            _: AccountId,
            _: ApplicationId,
            _: EnvironmentId,
            _: &str,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_agent_types(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_agent_type(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
            _: &AgentTypeName,
        ) -> Result<RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_by_names(
            &self,
            _: &ApplicationName,
            _: &EnvironmentName,
            _: &AgentTypeName,
            _: Option<DeploymentRevision>,
            _: Option<&str>,
            _: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            Ok(self.resolved.clone())
        }

        async fn get_active_routes_for_domain(
            &self,
            _: &golem_common::model::domain_registration::Domain,
        ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_compiled_mcps_for_domain(
            &self,
            _: &golem_common::model::domain_registration::Domain,
        ) -> Result<golem_service_base::mcp::CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_current_environment_state(
            &self,
            _: EnvironmentId,
        ) -> Result<golem_service_base::model::environment::EnvironmentState, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn get_resource_definition_by_id(
            &self,
            _: golem_common::model::quota::ResourceDefinitionId,
        ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_name(
            &self,
            _: EnvironmentId,
            _: golem_common::model::quota::ResourceName,
        ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn subscribe_registry_invalidations(
            &self,
            _: Option<u64>,
        ) -> Result<
            Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                golem_common::model::agent::RegistryInvalidationEvent,
                                RegistryServiceError,
                            >,
                        > + Send,
                >,
            >,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn run_registry_invalidation_event_subscriber(
            &self,
            _: &'static str,
            _: Option<tokio_util::sync::CancellationToken>,
            _: Arc<dyn golem_service_base::clients::registry::RegistryInvalidationHandler>,
        ) {
            unimplemented!()
        }
    }

    struct StaticComponentService {
        component: Component,
    }

    #[async_trait]
    impl ComponentService for StaticComponentService {
        async fn get_latest_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component> {
            (self.component.id == component_id).then(|| self.component.clone())
        }

        async fn get_latest_by_id_uncached(
            &self,
            component_id: ComponentId,
        ) -> Result<Component, ComponentServiceError> {
            if self.component.id == component_id {
                Ok(self.component.clone())
            } else {
                Err(ComponentServiceError::ComponentNotFound)
            }
        }

        async fn get_revision(
            &self,
            component_id: ComponentId,
            component_revision: ComponentRevision,
        ) -> Result<Component, ComponentServiceError> {
            if self.component.id == component_id && self.component.revision == component_revision {
                Ok(self.component.clone())
            } else {
                Err(ComponentServiceError::ComponentNotFound)
            }
        }

        async fn get_all_revisions(
            &self,
            component_id: ComponentId,
        ) -> Result<Vec<Component>, ComponentServiceError> {
            if self.component.id == component_id {
                Ok(vec![self.component.clone()])
            } else {
                Err(ComponentServiceError::ComponentNotFound)
            }
        }
    }

    struct AllowAllAuthService {
        account_id: AccountId,
    }

    #[async_trait]
    impl AuthService for AllowAllAuthService {
        async fn authenticate_token(
            &self,
            _: golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, AuthServiceError> {
            unimplemented!()
        }

        async fn authorize_environment_actions(
            &self,
            _: EnvironmentId,
            _: EnvironmentAction,
            _: &AuthCtx,
        ) -> Result<AuthDetailsForEnvironment, AuthServiceError> {
            Ok(AuthDetailsForEnvironment {
                account_id_owning_environment: self.account_id,
                environment_roles_from_shares: BTreeSet::<EnvironmentRole>::new(),
            })
        }
    }

    struct NoopLimitService;

    #[async_trait]
    impl LimitService for NoopLimitService {
        async fn update_worker_connection_limit(
            &self,
            _: AccountId,
            _: &AgentId,
            _: bool,
        ) -> Result<(), LimitServiceError> {
            Ok(())
        }
    }

    struct RecordingWorkerClient {
        created_agent_ids: Mutex<Vec<AgentId>>,
        invoked_agent_ids: Mutex<Vec<AgentId>>,
        invocation_output: AgentInvocationOutput,
    }

    impl RecordingWorkerClient {
        fn new(invocation_output: AgentInvocationOutput) -> Self {
            Self {
                created_agent_ids: Mutex::new(Vec::new()),
                invoked_agent_ids: Mutex::new(Vec::new()),
                invocation_output,
            }
        }

        fn created_agent_id(&self) -> AgentId {
            self.created_agent_ids.lock().unwrap()[0].clone()
        }

        fn invoked_agent_id(&self) -> AgentId {
            self.invoked_agent_ids.lock().unwrap()[0].clone()
        }
    }

    #[async_trait]
    impl WorkerClient for RecordingWorkerClient {
        async fn create(
            &self,
            agent_id: &AgentId,
            _: HashMap<String, String>,
            _: Vec<AgentConfigEntryDto>,
            _: bool,
            _: AccountId,
            _: EnvironmentId,
            _: AuthCtx,
            _: Option<InvocationContext>,
            _: Option<golem_api_grpc::proto::golem::component::Principal>,
        ) -> WorkerResult<AgentId> {
            self.created_agent_ids
                .lock()
                .unwrap()
                .push(agent_id.clone());
            Ok(agent_id.clone())
        }

        async fn connect(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<WorkerStream<LogEvent>> {
            unimplemented!()
        }

        async fn delete(&self, _: &AgentId, _: EnvironmentId, _: AuthCtx) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn complete_promise(
            &self,
            _: &AgentId,
            _: u64,
            _: Vec<u8>,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<bool> {
            unimplemented!()
        }

        async fn interrupt(
            &self,
            _: &AgentId,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn get_metadata(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<AgentMetadataDto> {
            unimplemented!()
        }

        async fn find_metadata(
            &self,
            _: ComponentId,
            _: Option<AgentFilter>,
            _: ScanCursor,
            _: u64,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
            unimplemented!()
        }

        async fn resume(
            &self,
            _: &AgentId,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn update(
            &self,
            _: &AgentId,
            _: golem_common::model::worker::AgentUpdateMode,
            _: ComponentRevision,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn get_oplog(
            &self,
            _: &AgentId,
            _: OplogIndex,
            _: Option<OplogCursor>,
            _: u64,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<GetOplogResponse, super::WorkerServiceError> {
            unimplemented!()
        }

        async fn search_oplog(
            &self,
            _: &AgentId,
            _: Option<OplogCursor>,
            _: u64,
            _: String,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<GetOplogResponse, super::WorkerServiceError> {
            unimplemented!()
        }

        async fn get_file_system_node(
            &self,
            _: &AgentId,
            _: CanonicalFilePath,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
            unimplemented!()
        }

        async fn get_file_contents(
            &self,
            _: &AgentId,
            _: CanonicalFilePath,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>>
        {
            unimplemented!()
        }

        async fn activate_plugin(
            &self,
            _: &AgentId,
            _: PluginPriority,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn deactivate_plugin(
            &self,
            _: &AgentId,
            _: PluginPriority,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn fork_worker(
            &self,
            _: &AgentId,
            _: &AgentId,
            _: OplogIndex,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn revert_worker(
            &self,
            _: &AgentId,
            _: RevertWorkerTarget,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn cancel_invocation(
            &self,
            _: &AgentId,
            _: &IdempotencyKey,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<bool> {
            unimplemented!()
        }

        async fn invoke_agent(
            &self,
            agent_id: &AgentId,
            _: Option<String>,
            _: Option<golem_api_grpc::proto::golem::component::UntypedDataValue>,
            _: i32,
            _: Option<::prost_types::Timestamp>,
            _: Option<IdempotencyKey>,
            _: Option<InvocationContext>,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
            _: golem_api_grpc::proto::golem::component::Principal,
        ) -> WorkerResult<AgentInvocationOutput> {
            self.invoked_agent_ids
                .lock()
                .unwrap()
                .push(agent_id.clone());
            Ok(self.invocation_output.clone())
        }

        async fn process_oplog_entries(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: ComponentRevision,
            _: IdempotencyKey,
            _: AccountId,
            _: HashMap<String, String>,
            _: golem_api_grpc::proto::golem::worker::AgentMetadata,
            _: OplogIndex,
            _: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }
    }

    struct RestHarness {
        worker_service: WorkerService,
        worker_client: Arc<RecordingWorkerClient>,
        agent_type_name: AgentTypeName,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    }

    impl RestHarness {
        fn new(mode: AgentMode) -> Self {
            let component_id = ComponentId(Uuid::new_v4());
            let environment_id = EnvironmentId(Uuid::new_v4());
            let account_id = AccountId(Uuid::new_v4());
            let component_revision = ComponentRevision::INITIAL;
            let agent_type_name = AgentTypeName("weather-agent".to_string());
            let agent_type = test_agent_type(agent_type_name.clone(), mode);
            let component = test_component(
                component_id,
                environment_id,
                account_id,
                component_revision,
                agent_type.clone(),
            );
            let worker_client = Arc::new(RecordingWorkerClient::new(AgentInvocationOutput {
                result: golem_common::model::AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: None,
                component_revision: Some(component_revision),
            }));
            let registry = Arc::new(TestRegistryService {
                resolved: ResolvedAgentType {
                    registered_agent_type: RegisteredAgentType {
                        agent_type,
                        implemented_by: RegisteredAgentTypeImplementer {
                            component_id,
                            component_revision,
                        },
                    },
                    environment_id,
                    deployment_revision: DeploymentRevision::INITIAL,
                    current_deployment_revision: Some(CurrentDeploymentRevision::INITIAL),
                },
            });
            let agent_resolution_cache = Arc::new(AgentResolutionCache::new(
                registry,
                1,
                Duration::from_secs(60),
                Duration::from_secs(60),
            ));

            Self {
                worker_service: WorkerService::new(
                    Arc::new(StaticComponentService { component }),
                    Arc::new(AllowAllAuthService { account_id }),
                    Arc::new(NoopLimitService),
                    worker_client.clone(),
                    agent_resolution_cache,
                ),
                worker_client,
                agent_type_name,
                component_id,
                component_revision,
            }
        }

        fn create_request(&self) -> CreateAgentRequest {
            CreateAgentRequest {
                app_name: ApplicationName::try_from("weather-app".to_string()).unwrap(),
                env_name: EnvironmentName::try_from("prod").unwrap(),
                agent_type_name: self.agent_type_name.clone(),
                parameters: empty_json_tuple(),
                phantom_id: None,
                config: vec![],
            }
        }

        fn invoke_request(&self) -> AgentInvocationRequest {
            AgentInvocationRequest {
                app_name: ApplicationName::try_from("weather-app".to_string()).unwrap(),
                env_name: EnvironmentName::try_from("prod").unwrap(),
                agent_type_name: self.agent_type_name.clone(),
                parameters: empty_json_tuple(),
                phantom_id: None,
                method_name: "run".to_string(),
                method_parameters: empty_json_tuple(),
                mode: AgentInvocationMode::Await,
                schedule_at: None,
                idempotency_key: None,
                deployment_revision: None,
                owner_account_email: None,
            }
        }
    }

    fn test_agent_type(agent_type_name: AgentTypeName, mode: AgentMode) -> AgentType {
        AgentType {
            type_name: agent_type_name,
            description: String::new(),
            source_language: String::new(),
            constructor: AgentConstructor {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
            },
            methods: vec![AgentMethod {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
                output_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
                http_endpoint: vec![HttpEndpointDetails {
                    http_method: golem_common::model::agent::HttpMethod::Get(Empty {}),
                    path_suffix: vec![],
                    header_vars: vec![],
                    query_vars: vec![],
                    auth_details: None,
                    cors_options: golem_common::model::agent::CorsOptions {
                        allowed_patterns: vec![],
                    },
                }],
            }],
            dependencies: vec![],
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    fn test_component(
        component_id: ComponentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        component_revision: ComponentRevision,
        agent_type: AgentType,
    ) -> Component {
        Component {
            id: component_id,
            revision: component_revision,
            environment_id,
            component_name: ComponentName("weather-component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId(Uuid::new_v4()),
            account_id,
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                KnownExports::default(),
                vec![],
                None,
                None,
                vec![agent_type],
                BTreeMap::new(),
            ),
            created_at: Utc::now(),
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        }
    }

    fn empty_json_tuple() -> UntypedJsonDataValue {
        UntypedJsonDataValue::Tuple(golem_common::model::agent::UntypedJsonElementValues {
            elements: vec![],
        })
    }

    fn phantom_id(agent_id: &AgentId) -> Option<Uuid> {
        agent_id
            .agent_id
            .rsplit_once('[')
            .and_then(|(_, phantom_id)| phantom_id.strip_suffix(']'))
            .and_then(|phantom_id| Uuid::parse_str(phantom_id).ok())
    }

    #[test]
    fn lookup_invocation_requires_view_worker_permission() {
        assert_eq!(
            required_environment_action_for_invocation_mode(
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
            ),
            EnvironmentAction::ViewWorker,
        );
    }

    #[test]
    fn non_lookup_invocation_requires_update_worker_permission() {
        assert_eq!(
            required_environment_action_for_invocation_mode(
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            ),
            EnvironmentAction::UpdateWorker,
        );
    }

    #[test]
    async fn create_agent_rest_auto_generates_phantom_for_ephemeral_agents() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let response = harness
            .worker_service
            .create_agent_rest(harness.create_request(), AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(response.agent_id.component_id, harness.component_id);
        assert_eq!(response.component_revision, harness.component_revision);
        assert_eq!(response.agent_id, harness.worker_client.created_agent_id());
        assert!(phantom_id(&response.agent_id).is_some());
    }

    #[test]
    async fn invoke_agent_rest_auto_generates_phantom_for_ephemeral_agents() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let response = harness
            .worker_service
            .invoke_agent_rest(harness.invoke_request(), AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(response.agent_id.component_id, harness.component_id);
        assert_eq!(
            response.component_revision,
            Some(harness.component_revision)
        );
        assert_eq!(response.agent_id, harness.worker_client.invoked_agent_id());
        assert!(phantom_id(&response.agent_id).is_some());
    }

    #[test]
    async fn durable_rest_paths_keep_non_phantom_agent_ids() {
        let harness = RestHarness::new(AgentMode::Durable);

        let create_response = harness
            .worker_service
            .create_agent_rest(harness.create_request(), AuthCtx::system())
            .await
            .unwrap();
        let invoke_response = harness
            .worker_service
            .invoke_agent_rest(harness.invoke_request(), AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(
            create_response.agent_id,
            harness.worker_client.created_agent_id()
        );
        assert_eq!(
            invoke_response.agent_id,
            harness.worker_client.invoked_agent_id()
        );
        assert!(phantom_id(&create_response.agent_id).is_none());
        assert!(phantom_id(&invoke_response.agent_id).is_none());
    }
}
