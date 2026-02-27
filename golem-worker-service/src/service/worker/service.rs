// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use crate::api::agents::{AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult};
use crate::service::auth::AuthService;
use crate::service::component::ComponentService;
use crate::service::limit::LimitService;
use bytes::Bytes;
use futures::Stream;
use golem_api_grpc::proto::golem::worker::InvocationContext;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::agent::{
    AgentId, DataValue, GolemUserPrincipal, Principal, UntypedDataValue,
};
use golem_common::model::component::{
    ComponentFilePath, ComponentId, ComponentRevision, PluginPriority,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::oplog::OplogCursor;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::WorkerUpdateMode;
use golem_common::model::worker::{RevertWorkerTarget, WorkerMetadataDto};
use golem_common::model::{IdempotencyKey, ScanCursor, WorkerFilter, WorkerId};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::model::auth::{AuthCtx, EnvironmentAction};
use golem_service_base::model::{Component, ComponentFileSystemNode, GetOplogResponse};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};

pub struct WorkerService {
    registry_service: Arc<dyn RegistryService>,
    component_service: Arc<dyn ComponentService>,
    auth_service: Arc<dyn AuthService>,
    limit_service: Arc<dyn LimitService>,
    worker_client: Arc<dyn WorkerClient>,
}

impl WorkerService {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        component_service: Arc<dyn ComponentService>,
        auth_service: Arc<dyn AuthService>,
        limit_service: Arc<dyn LimitService>,
        worker_client: Arc<dyn WorkerClient>,
    ) -> Self {
        Self {
            registry_service,
            component_service,
            auth_service,
            limit_service,
            worker_client,
        }
    }

    pub async fn create(
        &self,
        worker_id: &WorkerId,
        environment_variables: HashMap<String, String>,
        config_vars: BTreeMap<String, String>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<ComponentRevision> {
        let component = self
            .component_service
            .get_latest_by_id_uncached(worker_id.component_id)
            .await?;

        self.create_with_component(
            worker_id,
            component,
            environment_variables,
            config_vars,
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
        worker_id: &WorkerId,
        component: Component,
        environment_variables: HashMap<String, String>,
        config_vars: BTreeMap<String, String>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<ComponentRevision> {
        assert!(component.id == worker_id.component_id);

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
                worker_id,
                environment_variables,
                config_vars,
                ignore_already_existing,
                environment_auth_details.account_id_owning_environment,
                component.environment_id,
                auth_ctx,
                invocation_context,
                principal,
            )
            .await?;

        self.limit_service
            .update_worker_limit(
                environment_auth_details.account_id_owning_environment,
                worker_id,
                true,
            )
            .await?;

        Ok(component.revision)
    }

    pub async fn connect(
        &self,
        worker_id: &WorkerId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
                component.environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
            )
            .await?;

        self.limit_service
            .update_worker_connection_limit(
                environment_auth_details.account_id_owning_environment,
                worker_id,
                true,
            )
            .await?;

        Ok(ConnectWorkerStream::new(
            stream,
            worker_id.clone(),
            environment_auth_details.account_id_owning_environment,
            self.limit_service.clone(),
        ))
    }

    pub async fn delete(&self, worker_id: &WorkerId, auth_ctx: AuthCtx) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
            .await?;

        let environment_auth_details = self
            .auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::DeleteWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .delete(worker_id, component.environment_id, auth_ctx)
            .await?;

        self.limit_service
            .update_worker_limit(
                environment_auth_details.account_id_owning_environment,
                worker_id,
                false,
            )
            .await?;

        Ok(())
    }

    pub async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
            .complete_promise(
                worker_id,
                oplog_id,
                data,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
                recover_immediately,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerMetadataDto> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
            .get_metadata(worker_id, component.environment_id, auth_ctx)
            .await?;

        Ok(result)
    }

    pub async fn find_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadataDto>)> {
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
        worker_id: &WorkerId,
        force: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .resume(worker_id, force, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: WorkerUpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
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
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
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
        worker_id: &WorkerId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
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
        worker_id: &WorkerId,
        path: ComponentFilePath,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
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
        worker_id: &WorkerId,
        path: ComponentFilePath,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
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
        worker_id: &WorkerId,
        plugin_priority: PluginPriority,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
                plugin_priority,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn deactivate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_priority: PluginPriority,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
                plugin_priority,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(source_worker_id.component_id)
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
                source_worker_id,
                target_worker_id,
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
        worker_id: &WorkerId,
        target: RevertWorkerTarget,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .revert_worker(worker_id, target, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
                worker_id,
                idempotency_key,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(canceled)
    }

    pub async fn invoke_agent(
        &self,
        worker_id: &WorkerId,
        method_name: String,
        method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
    ) -> WorkerResult<AgentInvocationOutput> {
        let component = self
            .component_service
            .get_latest_by_id(worker_id.component_id)
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
            .invoke_agent(
                worker_id,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                idempotency_key,
                invocation_context,
                component.environment_id,
                environment_auth_details.account_id_owning_environment,
                auth_ctx,
                principal,
            )
            .await
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

        let registered_agent_type = self
            .registry_service
            .resolve_agent_type_by_names(
                &request.app_name,
                &request.env_name,
                &request.agent_type_name,
                deployment_revision,
                request.owner_account_email.as_deref(),
                &auth,
            )
            .await?;

        let component_id = registered_agent_type.implemented_by.component_id;
        let component_metadata = self
            .component_service
            .get_revision(
                component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;

        let agent_type = component_metadata
            .metadata
            .find_agent_type_by_wrapper_name(&request.agent_type_name)
            .map_err(|err| {
                WorkerServiceError::Internal(format!(
                    "Cannot get agent type {} from component metadata: {err}",
                    request.agent_type_name
                ))
            })?
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent type {} not found in component metadata",
                    request.agent_type_name
                ))
            })?;

        let constructor_parameters: DataValue = DataValue::try_from_untyped_json(
            request.parameters.clone(),
            agent_type.constructor.input_schema.clone(),
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;

        let agent_id = AgentId::new(
            request.agent_type_name.clone(),
            constructor_parameters,
            request.phantom_id,
        );

        let worker_id = WorkerId {
            component_id: component_metadata.id,
            worker_name: agent_id.to_string(),
        };

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
                golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Await as i32
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Schedule
                    as i32
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
                &worker_id,
                request.method_name,
                proto_method_parameters,
                proto_mode,
                proto_schedule_at,
                request.idempotency_key,
                None,
                auth,
                principal,
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
                    .get_revision(component_metadata.id, decode_revision)
                    .await?;
                let decode_agent_type = component_metadata_for_decode
                    .metadata
                    .find_agent_type_by_wrapper_name(&agent_type_name)
                    .map_err(|err| {
                        WorkerServiceError::Internal(format!(
                            "Cannot get agent type {agent_type_name} from component metadata at revision {decode_revision}: {err}",
                        ))
                    })?
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
                    result: Some(typed_data_value.into()),
                })
            }
            _ => Ok(AgentInvocationResult { result: None }),
        }
    }
}
