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
use crate::service::auth::AuthService;
use crate::service::component::ComponentService;
use crate::service::limit::LimitService;
use bytes::Bytes;
use futures::Stream;
use golem_api_grpc::proto::golem::worker::InvocationContext;
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentId, ComponentRevision, PluginPriority,
};
use golem_common::model::oplog::OplogCursor;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::WorkerUpdateMode;
use golem_common::model::worker::{RevertWorkerTarget, WorkerMetadataDto};
use golem_common::model::{IdempotencyKey, ScanCursor, WorkerFilter, WorkerId};
use golem_service_base::model::auth::{AuthCtx, EnvironmentAction};
use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
use golem_wasm::protobuf::Val as ProtoVal;
use golem_wasm::protobuf::Val;
use golem_wasm::ValueAndType;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};

pub struct WorkerService {
    component_service: Arc<dyn ComponentService>,
    auth_service: Arc<dyn AuthService>,
    limit_service: Arc<dyn LimitService>,
    worker_client: Arc<dyn WorkerClient>,
}

impl WorkerService {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        auth_service: Arc<dyn AuthService>,
        limit_service: Arc<dyn LimitService>,
        worker_client: Arc<dyn WorkerClient>,
    ) -> Self {
        Self {
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
        wasi_config_vars: BTreeMap<String, String>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<ComponentRevision> {
        let component = self
            .component_service
            .get_latest_by_id_uncached(&worker_id.component_id)
            .await?;

        self.create_with_component(
            worker_id,
            component,
            environment_variables,
            wasi_config_vars,
            ignore_already_existing,
            auth_ctx,
        )
        .await
    }

    // Like create, but skip fetching the component.
    pub async fn create_with_component(
        &self,
        worker_id: &WorkerId,
        component: ComponentDto,
        environment_variables: HashMap<String, String>,
        wasi_config_vars: BTreeMap<String, String>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<ComponentRevision> {
        assert!(component.id == worker_id.component_id);

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::CreateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .create(
                worker_id,
                component.revision,
                environment_variables,
                wasi_config_vars,
                ignore_already_existing,
                component.account_id,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        self.limit_service
            .update_worker_limit(&component.account_id, worker_id, true)
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::ViewWorker,
                &auth_ctx,
            )
            .await?;

        let stream = self
            .worker_client
            .connect(
                worker_id,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        self.limit_service
            .update_worker_connection_limit(&component.account_id, worker_id, true)
            .await?;

        Ok(ConnectWorkerStream::new(
            stream,
            worker_id.clone(),
            component.account_id,
            self.limit_service.clone(),
        ))
    }

    pub async fn delete(&self, worker_id: &WorkerId, auth_ctx: AuthCtx) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::DeleteWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .delete(worker_id, component.environment_id, auth_ctx)
            .await?;

        self.limit_service
            .update_worker_limit(&component.account_id, worker_id, false)
            .await?;

        Ok(())
    }

    fn validate_typed_parameters(&self, params: Vec<ValueAndType>) -> WorkerResult<Vec<ProtoVal>> {
        let mut result = Vec::new();
        for param in params {
            let val = param.value;
            result.push(golem_wasm::protobuf::Val::from(val));
        }
        Ok(result)
    }

    pub async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<Val>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>> {
        let component = self
            .component_service
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .invoke_and_await_typed(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ValueAndType>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>> {
        let params = self.validate_typed_parameters(params)?;
        self.invoke_and_await(
            worker_id,
            idempotency_key,
            function_name,
            params,
            invocation_context,
            auth_ctx,
        )
        .await
    }

    pub async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>> {
        let component = self
            .component_service
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        let result = self
            .worker_client
            .invoke_and_await_json(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn invoke(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<Val>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .invoke(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn invoke_typed(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ValueAndType>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let params = self.validate_typed_parameters(params)?;
        self.invoke(
            worker_id,
            idempotency_key,
            function_name,
            params,
            invocation_context,
            auth_ctx,
        )
        .await
    }

    pub async fn invoke_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .invoke_json(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                component.environment_id,
                component.account_id,
                auth_ctx,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
        component_id: &ComponentId,
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
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
                EnvironmentAction::UpdateWorker,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .update(
                worker_id,
                update_mode,
                target_revision,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
                component.account_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
                component.account_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&source_worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
                component.account_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
            .get_latest_by_id(&worker_id.component_id)
            .await?;

        self.auth_service
            .authorize_environment_actions(
                &component.environment_id,
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
}
