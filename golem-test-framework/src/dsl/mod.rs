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

pub mod benchmark;
pub mod debug_render;

use crate::config::{TestDependencies, TestDependenciesDsl};
use crate::dsl::debug_render::debug_render_oplog_entry;
use crate::model::PluginDefinitionCreation;
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use golem_api_grpc::proto::golem::component::v1::GetLatestComponentRequest;
use golem_api_grpc::proto::golem::worker::update_record::Update;
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;
use golem_api_grpc::proto::golem::worker::v1::{
    cancel_invocation_response, fork_worker_response, get_file_system_node_response,
    get_oplog_response, get_worker_metadata_response, get_workers_metadata_response,
    interrupt_worker_response, invoke_and_await_json_response, invoke_and_await_response,
    invoke_and_await_typed_response, invoke_response, launch_new_worker_response,
    resume_worker_response, revert_worker_response, search_oplog_response, update_worker_response,
    worker_execution_error, CancelInvocationRequest, ConnectWorkerRequest, DeleteWorkerRequest,
    ForkWorkerRequest, ForkWorkerResponse, GetFileContentsRequest, GetFileSystemNodeRequest,
    GetOplogRequest, GetWorkerMetadataRequest, GetWorkersMetadataRequest,
    GetWorkersMetadataSuccessResponse, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitJsonRequest, LaunchNewWorkerRequest, ResumeWorkerRequest, RevertWorkerRequest,
    SearchOplogRequest, UpdateWorkerRequest, UpdateWorkerResponse, WorkerError,
    WorkerExecutionError,
};
use golem_api_grpc::proto::golem::worker::{log_event, LogEvent, StdErrLog, StdOutLog, UpdateMode};
use golem_client::model::Account;
use golem_common::model::component_metadata::{
    ComponentMetadata, DynamicLinkedInstance, RawComponentMetadata,
};
use golem_common::model::oplog::{
    OplogIndex, TimestampedUpdateDescription, UpdateDescription, WorkerResourceId,
};
use golem_common::model::plugin::PluginWasmFileKey;
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    AccountId, ComponentFilePermissions, PluginInstallationId, ProjectId,
    WorkerResourceDescription, WorkerStatus,
};
use golem_common::model::{
    ComponentFileSystemNode, ComponentId, ComponentType, ComponentVersion, FailedUpdateRecord,
    IdempotencyKey, InitialComponentFile, InitialComponentFileKey, ScanCursor,
    SuccessfulUpdateRecord, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatusRecord,
};
use golem_common::widen_infallible;
use golem_service_base::model::{ComponentName, PublicOplogEntryWithIndex, RevertWorkerTarget};
use golem_service_base::replayable_stream::ReplayableStream;
use golem_wasm::{Value, ValueAndType};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::Builder;
use tokio::select;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot::Sender;
use tracing::{debug, info, Instrument};
use uuid::Uuid;
use wasm_metadata::{AddMetadata, AddMetadataField};

pub struct StoreComponentBuilder<'a, DSL: TestDsl + ?Sized> {
    dsl: &'a DSL,
    name: String,
    wasm_name: String,
    component_type: ComponentType,
    unique: bool,
    unverified: bool,
    files: Vec<(PathBuf, InitialComponentFile)>,
    dynamic_linking: Vec<(&'static str, DynamicLinkedInstance)>,
    env: HashMap<String, String>,
    project_id: Option<ProjectId>,
}

impl<'a, DSL: TestDsl> StoreComponentBuilder<'a, DSL> {
    pub fn new(dsl: &'a DSL, name: impl AsRef<str>) -> Self {
        Self {
            dsl,
            name: name.as_ref().to_string(),
            wasm_name: name.as_ref().to_string(),
            component_type: ComponentType::Durable,
            unique: false,
            unverified: false,
            files: vec![],
            dynamic_linking: vec![],
            env: HashMap::new(),
            project_id: None,
        }
    }

    /// Set the name of the component.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Set the component type to ephemeral.
    pub fn ephemeral(mut self) -> Self {
        self.component_type = ComponentType::Ephemeral;
        self
    }

    /// Set the component type to durable.
    pub fn durable(mut self) -> Self {
        self.component_type = ComponentType::Durable;
        self
    }

    /// Always create as a new component - otherwise, if the same component was already uploaded, it will be reused
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Reuse an existing component of the same WASM if it exists
    pub fn reused(mut self) -> Self {
        self.unique = false;
        self
    }

    /// Local filesystem mode only - do not try to parse the component
    pub fn unverified(mut self) -> Self {
        self.unverified = true;
        self
    }

    /// Local filesystem mode only - parse the component before storing
    pub fn verified(mut self) -> Self {
        self.unverified = false;
        self
    }

    /// Set the initial files for the component
    pub fn with_files(mut self, files: &[(PathBuf, InitialComponentFile)]) -> Self {
        self.files = files.to_vec();
        self
    }

    /// Adds an initial file to the component
    pub fn add_file(mut self, source: PathBuf, file: InitialComponentFile) -> Self {
        self.files.push((source, file));
        self
    }

    /// Set the dynamic linking for the component
    pub fn with_dynamic_linking(
        mut self,
        dynamic_linking: &[(&'static str, DynamicLinkedInstance)],
    ) -> Self {
        self.dynamic_linking = dynamic_linking.to_vec();
        self
    }

    /// Adds a dynamic linked instance to the component
    pub fn add_dynamic_linking(
        mut self,
        name: &'static str,
        instance: DynamicLinkedInstance,
    ) -> Self {
        self.dynamic_linking.push((name, instance));
        self
    }

    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        let map = env.into_iter().collect::<HashMap<_, _>>();
        self.env = map;
        self
    }

    pub fn with_project(mut self, project_id: ProjectId) -> Self {
        let _ = self.project_id.insert(project_id);
        self
    }

    /// Stores the component
    pub async fn store(self) -> ComponentId {
        self.store_and_get_name().await.0
    }

    /// Stores the component and returns the final component name too which is useful when used
    /// together with unique
    pub async fn store_and_get_name(self) -> (ComponentId, ComponentName) {
        self.dsl
            .store_component_with(
                &self.wasm_name,
                &self.name,
                self.component_type,
                self.unique,
                self.unverified,
                &self.files,
                &self.dynamic_linking,
                &self.env,
                self.project_id,
            )
            .await
    }
}

#[async_trait]
pub trait TestDsl {
    fn component(&self, name: &str) -> StoreComponentBuilder<'_, Self>;

    async fn store_component_with(
        &self,
        wasm_name: &str,
        name: &str,
        component_type: ComponentType,
        unique: bool,
        unverified: bool,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &[(&'static str, DynamicLinkedInstance)],
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> (ComponentId, ComponentName);

    async fn store_component_with_id(&self, name: &str, component_id: &ComponentId);

    async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> crate::Result<ComponentMetadata>;

    async fn update_component(&self, component_id: &ComponentId, name: &str) -> ComponentVersion;

    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        name: &str,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
    ) -> ComponentVersion;

    async fn update_component_with_env(
        &self,
        component_id: &ComponentId,
        name: &str,
        files: &[(String, String)],
    ) -> ComponentVersion;

    async fn add_initial_component_file(&self, path: &Path) -> InitialComponentFileKey;

    async fn add_initial_component_files(
        &self,
        files: &[(&str, &str, ComponentFilePermissions)],
    ) -> Vec<(PathBuf, InitialComponentFile)> {
        let mut added_files = Vec::<(PathBuf, InitialComponentFile)>::with_capacity(files.len());
        for (source, target, permissions) in files {
            added_files.push((
                source.into(),
                InitialComponentFile {
                    key: self.add_initial_component_file(Path::new(source)).await,
                    path: (*target).try_into().unwrap(),
                    permissions: *permissions,
                },
            ))
        }
        added_files
    }

    async fn add_plugin_wasm(&self, name: &str) -> crate::Result<PluginWasmFileKey>;

    async fn start_worker(&self, component_id: &ComponentId, name: &str)
        -> crate::Result<WorkerId>;

    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> crate::Result<Result<WorkerId, Error>>;

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> crate::Result<WorkerId>;

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> crate::Result<Result<WorkerId, Error>>;

    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> crate::Result<Option<(WorkerMetadata, Option<String>)>>;

    async fn wait_for_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        timeout: Duration,
    ) -> crate::Result<WorkerMetadata>;

    async fn wait_for_statuses(
        &self,
        worker_id: &WorkerId,
        status: &[WorkerStatus],
        timeout: Duration,
    ) -> crate::Result<WorkerMetadata>;

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> crate::Result<(Option<ScanCursor>, Vec<(WorkerMetadata, Option<String>)>)>;
    async fn delete_worker(&self, worker_id: &WorkerId) -> crate::Result<()>;

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<(), Error>>;
    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<(), Error>>;
    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>>;
    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>>;
    async fn invoke_and_await_custom(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>>;
    async fn invoke_and_await_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>>;
    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>>;
    async fn invoke_and_await_typed_custom(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>>;
    async fn invoke_and_await_typed_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>>;
    async fn invoke_and_await_typed_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>>;
    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> crate::Result<Result<serde_json::Value, Error>>;
    async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent>;
    async fn capture_output_forever(
        &self,
        worker_id: &WorkerId,
    ) -> (UnboundedReceiver<Option<LogEvent>>, Sender<()>);
    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>>;
    async fn log_output(&self, worker_id: &WorkerId);
    async fn resume(&self, worker_id: &WorkerId, force: bool) -> crate::Result<()>;
    async fn interrupt(&self, worker_id: &WorkerId) -> crate::Result<()>;
    async fn simulated_crash(&self, worker_id: &WorkerId) -> crate::Result<()>;
    async fn auto_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentVersion,
    ) -> crate::Result<()>;
    async fn manual_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentVersion,
    ) -> crate::Result<()>;
    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> crate::Result<Vec<PublicOplogEntryWithIndex>>;
    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> crate::Result<Vec<PublicOplogEntryWithIndex>>;

    async fn check_oplog_is_queryable(&self, worker_id: &WorkerId) -> crate::Result<()>;

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> crate::Result<Vec<ComponentFileSystemNode>>;

    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> crate::Result<Bytes>;

    async fn create_plugin(&self, definition: PluginDefinitionCreation) -> crate::Result<()>;

    async fn delete_plugin(
        &self,
        account_id: AccountId,
        name: &str,
        version: &str,
    ) -> crate::Result<()>;

    async fn install_plugin_to_component(
        &self,
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> crate::Result<PluginInstallationId>;

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index: OplogIndex,
    ) -> crate::Result<()>;

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget) -> crate::Result<()>;

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> crate::Result<bool>;

    async fn default_project(&self) -> crate::Result<ProjectId>;

    async fn create_project(&self) -> crate::Result<ProjectId>;

    async fn grant_full_project_access(
        &self,
        project_id: &ProjectId,
        grantee_account_id: &AccountId,
    ) -> crate::Result<()>;

    async fn get_account(&self, account_id: &AccountId) -> crate::Result<Account>;
}

#[async_trait]
impl<Deps: TestDependencies> TestDsl for TestDependenciesDsl<Deps> {
    fn component(&self, name: &str) -> StoreComponentBuilder<'_, Self> {
        StoreComponentBuilder::new(self, name)
    }

    async fn store_component_with(
        &self,
        wasm_name: &str,
        name: &str,
        component_type: ComponentType,
        unique: bool,
        unverified: bool,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &[(&'static str, DynamicLinkedInstance)],
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> (ComponentId, ComponentName) {
        let source_path = self
            .deps
            .component_directory()
            .join(format!("{wasm_name}.wasm"));
        let component_name = if unique {
            let uuid = Uuid::new_v4();
            format!("{name}---{uuid}")
        } else {
            match component_type {
                ComponentType::Durable => name.to_string(),
                ComponentType::Ephemeral => format!("{name}---ephemeral"),
            }
        };
        let dynamic_linking = HashMap::from_iter(
            dynamic_linking
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone())),
        );

        let source_path = if !unverified {
            rename_component_if_needed(
                self.deps.borrow().component_temp_directory(),
                &source_path,
                &component_name,
            )
            .expect("Failed to verify and change component metadata")
        } else {
            source_path
        };

        // Even though the APIs accept `Option<ProjectId>`, applying `self.default_project_id`
        // here makes it possible to force a different default in tests (for example, one per test case)
        let project_id = Some(project_id.unwrap_or_else(|| self.default_project_id.clone()));

        let component = {
            if unique {
                self.deps
                    .component_service()
                    .add_component(
                        &self.token,
                        &source_path,
                        &component_name,
                        component_type,
                        files,
                        &dynamic_linking,
                        unverified,
                        env,
                        project_id,
                    )
                    .await
                    .expect("Failed to add component")
            } else {
                self.deps
                    .component_service()
                    .get_or_add_component(
                        &self.token,
                        &source_path,
                        &component_name,
                        component_type,
                        files,
                        &dynamic_linking,
                        unverified,
                        env,
                        project_id,
                    )
                    .await
            }
        };

        (
            component
                .versioned_component_id
                .unwrap()
                .component_id
                .unwrap()
                .try_into()
                .unwrap(),
            ComponentName(component_name),
        )
    }

    async fn store_component_with_id(&self, name: &str, component_id: &ComponentId) {
        let source_path = self.deps.component_directory().join(format!("{name}.wasm"));
        self.deps
            .component_service()
            .add_component_with_id(
                &source_path,
                component_id,
                name,
                ComponentType::Durable,
                None,
            )
            .await
            .expect("Failed to store component");
    }

    async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> crate::Result<ComponentMetadata> {
        self.deps
            .component_service()
            .get_latest_component_metadata(
                &self.token,
                GetLatestComponentRequest {
                    component_id: Some(component_id.clone().into()),
                },
            )
            .await
            .and_then(|c| {
                c.metadata
                    .ok_or(anyhow!("metadata not found"))
                    .and_then(|cm| cm.try_into().map_err(|e: String| anyhow!(e)))
            })
    }

    async fn update_component(&self, component_id: &ComponentId, name: &str) -> ComponentVersion {
        let source_path = self.deps.component_directory().join(format!("{name}.wasm"));
        let component_env = HashMap::new();
        self.deps
            .component_service()
            .update_component(
                &self.token,
                component_id,
                &source_path,
                ComponentType::Durable,
                None,
                None,
                &component_env,
            )
            .await
            .unwrap()
    }

    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        name: &str,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
    ) -> ComponentVersion {
        let source_path = self.deps.component_directory().join(format!("{name}.wasm"));
        self.deps
            .component_service()
            .update_component(
                &self.token,
                component_id,
                &source_path,
                ComponentType::Durable,
                files,
                None,
                &HashMap::new(),
            )
            .await
            .unwrap()
    }

    async fn update_component_with_env(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: &[(String, String)],
    ) -> ComponentVersion {
        let map = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        let source_path = self.deps.component_directory().join(format!("{name}.wasm"));
        self.deps
            .component_service()
            .update_component(
                &self.token,
                component_id,
                &source_path,
                ComponentType::Durable,
                None,
                None,
                &map,
            )
            .await
            .unwrap()
    }

    async fn add_initial_component_file(&self, path: &Path) -> InitialComponentFileKey {
        let source_path = self.deps.borrow().component_directory().join(path);
        let data = tokio::fs::read(&source_path)
            .await
            .expect("Failed to read file");
        let bytes = Bytes::from(data);

        let stream = bytes
            .map_item(|i| i.map_err(widen_infallible))
            .map_error(widen_infallible);

        let project_id = self.default_project_id.clone();
        self.deps
            .initial_component_files_service()
            .put_if_not_exists(&project_id, stream)
            .await
            .expect("Failed to add initial component file")
    }

    async fn add_plugin_wasm(&self, name: &str) -> crate::Result<PluginWasmFileKey> {
        let source_path = self.deps.component_directory().join(format!("{name}.wasm"));
        let data = tokio::fs::read(&source_path)
            .await
            .map_err(|e| anyhow!("Failed to read file: {e}"))?;

        let bytes = Bytes::from(data);

        let stream = bytes
            .map_item(|i| i.map_err(widen_infallible))
            .map_error(widen_infallible);

        let key = self
            .deps
            .plugin_wasm_files_service()
            .put_if_not_exists(&self.account_id, stream)
            .await
            .map_err(|e| anyhow!("Failed to store plugin wasm: {e}"))?;

        Ok(key)
    }

    async fn start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> crate::Result<WorkerId> {
        TestDsl::start_worker_with(self, component_id, name, vec![], HashMap::new(), vec![]).await
    }

    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> crate::Result<Result<WorkerId, Error>> {
        TestDsl::try_start_worker_with(self, component_id, name, vec![], HashMap::new(), vec![])
            .await
    }

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> crate::Result<WorkerId> {
        let result =
            TestDsl::try_start_worker_with(self, component_id, name, args, env, wasi_config_vars)
                .await?;
        Ok(result.map_err(|err| anyhow!("Failed to start worker: {err:?}"))?)
    }

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> crate::Result<Result<WorkerId, Error>> {
        let response = self
            .deps
            .worker_service()
            .create_worker(
                &self.token,
                LaunchNewWorkerRequest {
                    component_id: Some(component_id.clone().into()),
                    name: name.to_string(),
                    args,
                    env,
                    wasi_config_vars: Some(BTreeMap::from_iter(wasi_config_vars).into()),
                    ignore_already_existing: false,
                },
            )
            .await?;

        match response.result {
            None => panic!("No response from create_worker"),
            Some(launch_new_worker_response::Result::Success(response)) => Ok(Ok(response
                .worker_id
                .ok_or(anyhow!("worker_id is missing"))?
                .try_into()
                .map_err(|err: String| anyhow!(err))?)),
            Some(launch_new_worker_response::Result::Error(WorkerError { error: Some(error) })) => {
                Ok(Err(error))
            }
            Some(launch_new_worker_response::Result::Error(_)) => {
                Err(anyhow!("Error response without any details"))
            }
        }
    }

    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> crate::Result<Option<(WorkerMetadata, Option<String>)>> {
        let worker_id: golem_api_grpc::proto::golem::worker::WorkerId = worker_id.clone().into();
        let response = self
            .deps
            .worker_service()
            .get_worker_metadata(
                &self.token,
                GetWorkerMetadataRequest {
                    worker_id: Some(worker_id),
                },
            )
            .await?;

        debug!("Received worker metadata: {:?}", response);

        match response.result {
            None => Err(anyhow!("No response from connect_worker")),
            Some(get_worker_metadata_response::Result::Success(metadata)) => {
                Ok(Some(to_worker_metadata(&metadata)))
            }
            Some(get_worker_metadata_response::Result::Error(WorkerError {
                error: Some(Error::NotFound { .. }),
            })) => Ok(None),
            Some(get_worker_metadata_response::Result::Error(WorkerError {
                error:
                    Some(Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::WorkerNotFound(_)),
                    })),
            })) => Ok(None),
            Some(get_worker_metadata_response::Result::Error(error)) => {
                Err(anyhow!("Failed to get worker metadata: {error:?}"))
            }
        }
    }

    async fn wait_for_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        timeout: Duration,
    ) -> crate::Result<WorkerMetadata> {
        TestDsl::wait_for_statuses(self, worker_id, &[status], timeout).await
    }

    async fn wait_for_statuses(
        &self,
        worker_id: &WorkerId,
        statuses: &[WorkerStatus],
        timeout: Duration,
    ) -> crate::Result<WorkerMetadata> {
        let start = Instant::now();
        let mut last_known = None;
        while start.elapsed() < timeout {
            let (metadata, _) = TestDsl::get_worker_metadata(self, worker_id)
                .await?
                .ok_or(anyhow!("Worker not found"))?;
            if statuses
                .iter()
                .any(|s| s == &metadata.last_known_status.status)
            {
                return Ok(metadata);
            }

            last_known = Some(metadata.last_known_status.status.clone());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(anyhow!(
            "Timeout waiting for worker status {} (last known: {last_known:?})",
            statuses
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> crate::Result<(Option<ScanCursor>, Vec<(WorkerMetadata, Option<String>)>)> {
        let component_id: golem_api_grpc::proto::golem::component::ComponentId =
            component_id.clone().into();
        let response = self
            .deps
            .worker_service()
            .get_workers_metadata(
                &self.token,
                GetWorkersMetadataRequest {
                    component_id: Some(component_id),
                    filter: filter.map(|f| f.into()),
                    cursor: Some(cursor.into()),
                    count,
                    precise,
                },
            )
            .await?;
        match response.result {
            None => Err(anyhow!("No response from get_workers_metadata")),
            Some(get_workers_metadata_response::Result::Success(
                GetWorkersMetadataSuccessResponse { workers, cursor },
            )) => Ok((
                cursor.map(|c| c.into()),
                workers.iter().map(to_worker_metadata).collect(),
            )),
            Some(get_workers_metadata_response::Result::Error(error)) => {
                Err(anyhow!("Failed to get workers metadata: {error:?}"))
            }
        }
    }

    async fn delete_worker(&self, worker_id: &WorkerId) -> crate::Result<()> {
        let _ = self
            .deps
            .worker_service()
            .delete_worker(
                &self.token,
                DeleteWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                },
            )
            .await?;
        Ok(())
    }

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<(), Error>> {
        let invoke_response = self
            .deps
            .worker_service()
            .invoke(
                &self.token,
                worker_id.clone().into(),
                None,
                function_name.to_string(),
                params,
                None,
            )
            .await?;

        match invoke_response.result {
            None => Err(anyhow!("No response from invoke_worker")),
            Some(invoke_response::Result::Success(_)) => Ok(Ok(())),
            Some(invoke_response::Result::Error(WorkerError { error: Some(error) })) => {
                Ok(Err(error))
            }
            Some(invoke_response::Result::Error(_)) => {
                Err(anyhow!("Empty error response from invoke_worker"))
            }
        }
    }

    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<(), Error>> {
        let invoke_response = self
            .deps
            .worker_service()
            .invoke(
                &self.token,
                worker_id.clone().into(),
                Some(idempotency_key.clone().into()),
                function_name.to_string(),
                params,
                None,
            )
            .await?;

        match invoke_response.result {
            None => Err(anyhow!("No response from invoke_worker")),
            Some(invoke_response::Result::Success(_)) => Ok(Ok(())),
            Some(invoke_response::Result::Error(WorkerError { error: Some(error) })) => {
                Ok(Err(error))
            }
            Some(invoke_response::Result::Error(_)) => {
                Err(anyhow!("Empty error response from invoke_worker"))
            }
        }
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>> {
        TestDsl::invoke_and_await_custom(self, worker_id, function_name, params).await
    }

    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>> {
        TestDsl::invoke_and_await_custom_with_key(
            self,
            worker_id,
            idempotency_key,
            function_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_custom(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>> {
        let idempotency_key = IdempotencyKey::fresh();
        TestDsl::invoke_and_await_custom_with_key(
            self,
            worker_id,
            &idempotency_key,
            function_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, Error>> {
        let invoke_response = self
            .deps
            .worker_service()
            .invoke_and_await(
                &self.token,
                worker_id.clone().into(),
                Some(idempotency_key.clone().into()),
                function_name.to_string(),
                params,
                None,
            )
            .await?;

        match invoke_response.result {
            None => Err(anyhow!("No response from invoke_and_await")),
            Some(invoke_and_await_response::Result::Success(response)) => Ok(Ok(response
                .result
                .into_iter()
                .map(|v| v.try_into())
                .collect::<Result<Vec<Value>, String>>()
                .map_err(|err| anyhow!("Invocation result had unexpected format: {err}"))?)),
            Some(invoke_and_await_response::Result::Error(WorkerError { error: Some(error) })) => {
                Ok(Err(error))
            }
            Some(invoke_and_await_response::Result::Error(_)) => {
                Err(anyhow!("Empty error response from invoke_and_await"))
            }
        }
    }

    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>> {
        TestDsl::invoke_and_await_typed_custom(self, worker_id, function_name, params).await
    }

    async fn invoke_and_await_typed_custom(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>> {
        let idempotency_key = IdempotencyKey::fresh();
        TestDsl::invoke_and_await_typed_custom_with_key(
            self,
            worker_id,
            &idempotency_key,
            function_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_typed_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>> {
        TestDsl::invoke_and_await_typed_custom_with_key(
            self,
            worker_id,
            idempotency_key,
            function_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_typed_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Option<ValueAndType>, Error>> {
        let invoke_response = self
            .deps
            .worker_service()
            .invoke_and_await_typed(
                &self.token,
                worker_id.clone().into(),
                Some(idempotency_key.clone().into()),
                function_name.to_string(),
                params,
                None,
            )
            .await?;

        match invoke_response.result {
            None => Err(anyhow!("No response from invoke_and_await_typed")),
            Some(invoke_and_await_typed_response::Result::Success(response)) => {
                match response.result {
                    None => Ok(Ok(None)),
                    Some(response) => {
                        let response: ValueAndType = response.try_into().map_err(|err| {
                            anyhow!("Invocation result had unexpected format: {err}")
                        })?;
                        Ok(Ok(Some(response)))
                    }
                }
            }
            Some(invoke_and_await_typed_response::Result::Error(WorkerError {
                error: Some(error),
            })) => Ok(Err(error)),
            Some(invoke_and_await_typed_response::Result::Error(_)) => {
                Err(anyhow!("Empty error response from invoke_and_await_typed"))
            }
        }
    }

    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> crate::Result<Result<serde_json::Value, Error>> {
        let params = params.into_iter().map(|p| p.to_string()).collect();
        let invoke_response = self
            .deps
            .worker_service()
            .invoke_and_await_json(
                &self.token,
                InvokeAndAwaitJsonRequest {
                    worker_id: Some(worker_id.clone().into()),
                    idempotency_key: Some(IdempotencyKey::fresh().into()),
                    function: function_name.to_string(),
                    invoke_parameters: params,
                    context: None,
                },
            )
            .await?;

        match invoke_response.result {
            None => Err(anyhow!("No response from invoke_and_await_json")),
            Some(invoke_and_await_json_response::Result::Success(response)) => {
                let response = serde_json::from_str(&response).map_err(|err| anyhow!(err))?;
                Ok(Ok(response))
            }
            Some(invoke_and_await_json_response::Result::Error(WorkerError {
                error: Some(error),
            })) => Ok(Err(error)),
            Some(invoke_and_await_json_response::Result::Error(_)) => {
                Err(anyhow!("Empty error response from invoke_and_await"))
            }
        }
    }

    async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cloned_service = self.deps.borrow().worker_service().clone();
        let worker_id = worker_id.clone();
        let token = self.token;
        tokio::spawn(
            async move {
                let mut response = cloned_service
                    .connect_worker(
                        &token,
                        ConnectWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                        },
                    )
                    .await
                    .expect("Failed to connect worker");

                while let Some(event) = response.message().await.expect("Failed to get message") {
                    debug!("Received event: {:?}", event);
                    tx.send(event).expect("Failed to send event");
                }

                debug!("Finished receiving events");
            }
            .in_current_span(),
        );

        rx
    }

    async fn capture_output_forever(
        &self,
        worker_id: &WorkerId,
    ) -> (UnboundedReceiver<Option<LogEvent>>, Sender<()>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cloned_service = self.deps.borrow().worker_service().clone();
        let worker_id = worker_id.clone();
        let token = self.token;
        let (abort_tx, mut abort_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(
            async move {
                let mut abort = false;
                while !abort {
                    let mut response = cloned_service
                        .connect_worker(
                            &token,
                            ConnectWorkerRequest {
                                worker_id: Some(worker_id.clone().into()),
                            },
                        )
                        .await
                        .expect("Failed to connect worker");

                    loop {
                        select! {
                            msg = response.message() => {
                                match msg {
                                    Ok(Some(event)) =>  {
                                        debug!("Received event: {:?}", event);
                                        tx.send(Some(event)).expect("Failed to send event");
                                    }
                                    Ok(None) => {
                                        break;
                                    }
                                    Err(e) => {
                                        panic!("Failed to get message: {e:?}");
                                    }
                                }
                            }
                            _ = (&mut abort_rx) => {
                                abort = true;
                                break;
                            }
                        }
                    }
                }

                tx.send(None).expect("Failed to send event");
                debug!("Finished receiving events");
            }
            .in_current_span(),
        );

        (rx, abort_tx)
    }

    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cloned_service = self.deps.borrow().worker_service().clone();
        let worker_id = worker_id.clone();
        let token = self.token;
        tokio::spawn(
            async move {
                let mut response = cloned_service
                    .connect_worker(
                        &token,
                        ConnectWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                        },
                    )
                    .await
                    .expect("Failed to connect to worker");

                while let Some(event) = response.message().await.expect("Failed to get message") {
                    debug!("Received event: {:?}", event);
                    tx.send(Some(event)).expect("Failed to send event");
                }

                debug!("Finished receiving events");
                tx.send(None).expect("Failed to send termination event");
            }
            .in_current_span(),
        );

        rx
    }

    async fn log_output(&self, worker_id: &WorkerId) {
        let cloned_service = self.deps.borrow().worker_service().clone();
        let worker_id = worker_id.clone();
        let token = self.token;
        tokio::spawn(
            async move {
                let mut response = cloned_service
                    .connect_worker(
                        &token,
                        ConnectWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                        },
                    )
                    .await
                    .expect("Failed to connect worker");

                while let Some(event) = response.message().await.expect("Failed to get message") {
                    info!("Received event: {:?}", event);
                }
            }
            .in_current_span(),
        );
    }

    async fn resume(&self, worker_id: &WorkerId, force: bool) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .resume_worker(
                &self.token,
                ResumeWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    force: Some(force),
                },
            )
            .await?;

        match response.result {
            None => Err(anyhow!("No response from connect_worker")),
            Some(resume_worker_response::Result::Success(_)) => Ok(()),
            Some(resume_worker_response::Result::Error(error)) => {
                Err(anyhow!("Failed to connect worker: {error:?}"))
            }
        }
    }

    async fn interrupt(&self, worker_id: &WorkerId) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .interrupt_worker(
                &self.token,
                InterruptWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    recover_immediately: false,
                },
            )
            .await?;

        match response {
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Success(_)),
            } => Ok(()),
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Error(error)),
            } => panic!("Failed to interrupt worker: {error:?}"),
            _ => panic!("Failed to interrupt worker: unknown error"),
        }
    }

    async fn simulated_crash(&self, worker_id: &WorkerId) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .interrupt_worker(
                &self.token,
                InterruptWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    recover_immediately: true,
                },
            )
            .await?;

        match response {
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Success(_)),
            } => Ok(()),
            InterruptWorkerResponse {
                result: Some(interrupt_worker_response::Result::Error(error)),
            } => Err(anyhow!("Failed to crash worker: {error:?}")),
            _ => Err(anyhow!("Failed to crash worker: unknown error")),
        }
    }

    async fn auto_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentVersion,
    ) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .update_worker(
                &self.token,
                UpdateWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    target_version,
                    mode: UpdateMode::Automatic.into(),
                },
            )
            .await?;

        match response {
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Success(_)),
            } => Ok(()),
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Error(error)),
            } => Err(anyhow!("Failed to update worker: {error:?}")),
            _ => Err(anyhow!("Failed to update worker: unknown error")),
        }
    }

    async fn manual_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentVersion,
    ) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .update_worker(
                &self.token,
                UpdateWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    target_version,
                    mode: UpdateMode::Manual.into(),
                },
            )
            .await?;

        match response {
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Success(_)),
            } => Ok(()),
            UpdateWorkerResponse {
                result: Some(update_worker_response::Result::Error(error)),
            } => Err(anyhow!("Failed to update worker: {error:?}")),
            _ => Err(anyhow!("Failed to update worker: unknown error")),
        }
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> crate::Result<Vec<PublicOplogEntryWithIndex>> {
        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let chunk = self
                .deps
                .worker_service()
                .get_oplog(
                    &self.token,
                    GetOplogRequest {
                        worker_id: Some(worker_id.clone().into()),
                        from_oplog_index: from.into(),
                        cursor,
                        count: 100,
                    },
                )
                .await?;

            if let Some(chunk) = chunk.result {
                match chunk {
                    get_oplog_response::Result::Success(chunk) => {
                        if chunk.entries.is_empty() {
                            break;
                        } else {
                            result.extend(
                                chunk
                                    .entries
                                    .into_iter()
                                    .enumerate()
                                    .map(|(chunk_idx, entry)| {
                                        PublicOplogEntry::try_from(entry).map(
                                            |public_oplog_entry| PublicOplogEntryWithIndex {
                                                entry: public_oplog_entry,
                                                oplog_index: OplogIndex::from_u64(
                                                    chunk.first_index_in_chunk + chunk_idx as u64,
                                                ),
                                            },
                                        )
                                    })
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|err| {
                                        anyhow!("Failed to convert oplog entry: {err}")
                                    })?,
                            );
                            cursor = chunk.next;
                        }
                    }
                    get_oplog_response::Result::Error(error) => {
                        return Err(anyhow!("Failed to get oplog: {error:?}"));
                    }
                }
            } else {
                break;
            }
        }

        Ok(result)
    }

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> crate::Result<Vec<PublicOplogEntryWithIndex>> {
        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let chunk = self
                .deps
                .worker_service()
                .search_oplog(
                    &self.token,
                    SearchOplogRequest {
                        worker_id: Some(worker_id.clone().into()),
                        cursor,
                        count: 100,
                        query: query.to_string(),
                    },
                )
                .await?;

            if let Some(chunk) = chunk.result {
                match chunk {
                    search_oplog_response::Result::Success(chunk) => {
                        if chunk.entries.is_empty() {
                            break;
                        } else {
                            result.extend(
                                chunk
                                    .entries
                                    .into_iter()
                                    .map(|entry| entry.try_into())
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|err| {
                                        anyhow!("Failed to convert oplog entry: {err}")
                                    })?,
                            );
                            cursor = chunk.next;
                        }
                    }
                    search_oplog_response::Result::Error(error) => {
                        return Err(anyhow!("Failed to search oplog: {error:?}"));
                    }
                }
            } else {
                break;
            }
        }

        Ok(result)
    }

    async fn check_oplog_is_queryable(&self, worker_id: &WorkerId) -> crate::Result<()> {
        let oplog = TestDsl::get_oplog(self, worker_id, OplogIndex::INITIAL).await?;

        for entry in oplog.iter() {
            debug!(
                "#{}:\n{}",
                entry.oplog_index,
                debug_render_oplog_entry(&entry.entry)
            );
        }

        Ok(())
    }

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> crate::Result<Vec<ComponentFileSystemNode>> {
        let response = self
            .deps
            .worker_service()
            .get_file_system_node(
                &self.token,
                GetFileSystemNodeRequest {
                    worker_id: Some(worker_id.clone().into()),
                    path: path.to_string(),
                },
            )
            .await?;

        match response.result {
            Some(get_file_system_node_response::Result::Success(response)) => {
                let converted = response
                    .nodes
                    .into_iter()
                    .map(|node| node.try_into())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|err| anyhow!("Failed to convert node: {err}"))?;
                Ok(converted)
            }
            _ => Err(anyhow!("Failed to list directory")),
        }
    }

    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> crate::Result<Bytes> {
        self.deps
            .worker_service()
            .get_file_contents(
                &self.token,
                GetFileContentsRequest {
                    worker_id: Some(worker_id.clone().into()),
                    file_path: path.to_string(),
                },
            )
            .await
    }

    async fn create_plugin(&self, definition: PluginDefinitionCreation) -> crate::Result<()> {
        self.deps
            .component_service()
            .create_plugin(&self.token, &self.account_id, definition)
            .await
    }

    async fn delete_plugin(
        &self,
        account_id: AccountId,
        name: &str,
        version: &str,
    ) -> crate::Result<()> {
        self.deps
            .component_service()
            .delete_plugin(&self.token, account_id, name, version)
            .await
    }

    async fn install_plugin_to_component(
        &self,
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> crate::Result<PluginInstallationId> {
        self.deps
            .component_service()
            .install_plugin_to_component(
                &self.token,
                component_id,
                plugin_name,
                plugin_version,
                priority,
                parameters,
            )
            .await
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index: OplogIndex,
    ) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .fork_worker(
                &self.token,
                ForkWorkerRequest {
                    source_worker_id: Some(source_worker_id.clone().into()),
                    target_worker_id: Some(target_worker_id.clone().into()),
                    oplog_index_cutoff: oplog_index.into(),
                },
            )
            .await?;

        match response {
            ForkWorkerResponse {
                result: Some(fork_worker_response::Result::Success(_)),
            } => Ok(()),
            ForkWorkerResponse {
                result: Some(fork_worker_response::Result::Error(error)),
            } => Err(anyhow!("Failed to fork worker: {error:?}")),
            _ => Err(anyhow!("Failed to fork worker: unknown error")),
        }
    }

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget) -> crate::Result<()> {
        let response = self
            .deps
            .worker_service()
            .revert_worker(
                &self.token,
                RevertWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    target: Some(target.into()),
                },
            )
            .await?;

        match response.result {
            Some(revert_worker_response::Result::Success(_)) => Ok(()),
            Some(revert_worker_response::Result::Error(error)) => {
                Err(anyhow!("Failed to fork worker: {error:?}"))
            }
            _ => Err(anyhow!("Failed to revert worker: unknown error")),
        }
    }

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> crate::Result<bool> {
        let response = self
            .deps
            .worker_service()
            .cancel_invocation(
                &self.token,
                CancelInvocationRequest {
                    worker_id: Some(worker_id.clone().into()),
                    idempotency_key: Some(idempotency_key.clone().into()),
                },
            )
            .await?;

        match response.result {
            Some(cancel_invocation_response::Result::Success(canceled)) => Ok(canceled),
            Some(cancel_invocation_response::Result::Error(error)) => {
                Err(anyhow!("Failed to cancel invocation: {error:?}"))
            }
            _ => Err(anyhow!("Failed to cancel invocation: unknown error")),
        }
    }

    async fn default_project(&self) -> crate::Result<ProjectId> {
        self.deps
            .cloud_service()
            .get_default_project(&self.token)
            .await
    }

    async fn create_project(&self) -> crate::Result<ProjectId> {
        let name = Uuid::new_v4().to_string();
        let description = Uuid::new_v4().to_string();

        self.deps
            .cloud_service()
            .create_project(&self.token, name, self.account_id.clone(), description)
            .await
    }

    async fn grant_full_project_access(
        &self,
        project_id: &ProjectId,
        grantee_account_id: &AccountId,
    ) -> crate::Result<()> {
        self.deps
            .cloud_service()
            .grant_full_project_access(&self.token, project_id, grantee_account_id)
            .await?;
        Ok(())
    }

    async fn get_account(&self, account_id: &AccountId) -> crate::Result<Account> {
        self.deps
            .cloud_service()
            .get_account_by_id(&self.token, account_id)
            .await
    }
}

pub fn stdout_events(events: impl Iterator<Item = LogEvent>) -> Vec<String> {
    events
        .flat_map(|event| match event {
            LogEvent {
                event: Some(log_event::Event::Stdout(StdOutLog { message, .. })),
            } => Some(message),
            _ => None,
        })
        .collect()
}

pub fn stdout_event_matching(event: &LogEvent, s: &str) -> bool {
    if let LogEvent {
        event: Some(log_event::Event::Stdout(StdOutLog { message, .. })),
    } = event
    {
        message == s
    } else {
        false
    }
}

pub fn stdout_event_starting_with(event: &LogEvent, s: &str) -> bool {
    if let LogEvent {
        event: Some(log_event::Event::Stdout(StdOutLog { message, .. })),
    } = event
    {
        message.starts_with(s)
    } else {
        false
    }
}

pub fn stderr_events(events: impl Iterator<Item = LogEvent>) -> Vec<String> {
    events
        .flat_map(|event| match event {
            LogEvent {
                event: Some(log_event::Event::Stderr(StdErrLog { message, .. })),
            } => Some(message),
            _ => None,
        })
        .collect()
}

pub fn log_event_to_string(event: &LogEvent) -> String {
    match &event.event {
        Some(log_event::Event::Stdout(stdout)) => stdout.message.clone(),
        Some(log_event::Event::Stderr(stderr)) => stderr.message.clone(),
        Some(log_event::Event::Log(log)) => log.message.clone(),
        Some(log_event::Event::InvocationFinished(_)) => "".to_string(),
        Some(log_event::Event::InvocationStarted(_)) => "".to_string(),
        Some(log_event::Event::ClientLagged { .. }) => "".to_string(),
        None => std::panic!("Unexpected event type"),
    }
}

pub async fn drain_connection(rx: UnboundedReceiver<Option<LogEvent>>) -> Vec<Option<LogEvent>> {
    let mut rx = rx;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    if !events.contains(&None) {
        loop {
            match rx.recv().await {
                Some(Some(event)) => events.push(Some(event)),
                Some(None) => break,
                None => break,
            }
        }
    }
    events
}

pub async fn events_to_lines(rx: &mut UnboundedReceiver<LogEvent>) -> Vec<String> {
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;
    let full_output = events
        .iter()
        .map(log_event_to_string)
        .collect::<Vec<_>>()
        .join("");
    let lines = full_output
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    lines
}

pub fn is_worker_execution_error(got: &Error, expected: &worker_execution_error::Error) -> bool {
    matches!(got, Error::InternalError(error) if error.error.as_ref() == Some(expected))
}

pub fn worker_error_message(error: &Error) -> String {
    match error {
        Error::BadRequest(errors) => errors.errors.join(", "),
        Error::Unauthorized(error) => error.error.clone(),
        Error::LimitExceeded(error) => error.error.clone(),
        Error::NotFound(error) => error.error.clone(),
        Error::AlreadyExists(error) => error.error.clone(),
        Error::InternalError(error) => match &error.error {
            None => "Internal error".to_string(),
            Some(error) => match error {
                worker_execution_error::Error::InvalidRequest(error) => error.details.clone(),
                worker_execution_error::Error::WorkerAlreadyExists(error) => {
                    format!("Worker already exists: {:?}", error.worker_id)
                }
                worker_execution_error::Error::WorkerCreationFailed(error) => format!(
                    "Worker creation failed: {:?}: {}",
                    error.worker_id, error.details
                ),
                worker_execution_error::Error::FailedToResumeWorker(error) => {
                    format!("Failed to resume worker: {:?}", error.worker_id)
                }
                worker_execution_error::Error::ComponentDownloadFailed(error) => format!(
                    "Failed to download component: {:?} version {}: {}",
                    error.component_id, error.component_version, error.reason
                ),
                worker_execution_error::Error::ComponentParseFailed(error) => format!(
                    "Failed to parse component: {:?} version {}: {}",
                    error.component_id, error.component_version, error.reason
                ),
                worker_execution_error::Error::GetLatestVersionOfComponentFailed(error) => format!(
                    "Failed to get latest version of component: {:?}: {}",
                    error.component_id, error.reason
                ),
                worker_execution_error::Error::PromiseNotFound(error) => {
                    format!("Promise not found: {:?}", error.promise_id)
                }
                worker_execution_error::Error::PromiseDropped(error) => {
                    format!("Promise dropped: {:?}", error.promise_id)
                }
                worker_execution_error::Error::PromiseAlreadyCompleted(error) => {
                    format!("Promise already completed: {:?}", error.promise_id)
                }
                worker_execution_error::Error::Interrupted(error) => {
                    if error.recover_immediately {
                        "Simulated crash".to_string()
                    } else {
                        "Interrupted via the Golem API".to_string()
                    }
                }
                worker_execution_error::Error::ParamTypeMismatch(_error) => {
                    "Parameter type mismatch".to_string()
                }
                worker_execution_error::Error::NoValueInMessage(_error) => {
                    "No value in message".to_string()
                }
                worker_execution_error::Error::ValueMismatch(error) => {
                    format!("Value mismatch: {}", error.details)
                }
                worker_execution_error::Error::UnexpectedOplogEntry(error) => format!(
                    "Unexpected oplog entry; Expected: {}, got: {}",
                    error.expected, error.got
                ),
                worker_execution_error::Error::RuntimeError(error) => {
                    format!("Runtime error: {}", error.details)
                }
                worker_execution_error::Error::InvalidShardId(error) => format!(
                    "Invalid shard id: {:?}; ids: {:?}",
                    error.shard_id, error.shard_ids
                ),
                worker_execution_error::Error::PreviousInvocationFailed(_) => {
                    "Previous invocation failed".to_string()
                }
                worker_execution_error::Error::Unknown(error) => {
                    format!("Unknown error: {}", error.details)
                }
                worker_execution_error::Error::PreviousInvocationExited(_error) => {
                    "Previous invocation exited".to_string()
                }
                worker_execution_error::Error::InvalidAccount(_error) => {
                    "Invalid account id".to_string()
                }
                worker_execution_error::Error::WorkerNotFound(error) => {
                    format!("Worker not found: {:?}", error.worker_id)
                }
                worker_execution_error::Error::ShardingNotReady(_error) => {
                    "Sharing not ready".to_string()
                }
                worker_execution_error::Error::InitialComponentFileDownloadFailed(error) => {
                    format!("Initial File download failed: {}", error.reason)
                }
                worker_execution_error::Error::FileSystemError(error) => {
                    format!("File system error: {}", error.reason)
                }
                worker_execution_error::Error::InvocationFailed(_) => {
                    "Invocation failed".to_string()
                }
            },
        },
    }
}

pub fn worker_error_underlying_error(
    error: &Error,
) -> Option<golem_common::model::oplog::WorkerError> {
    match error {
        Error::InternalError(error) => match &error.error {
            Some(worker_execution_error::Error::InvocationFailed(error)) => {
                Some(error.error.clone().unwrap().try_into().unwrap())
            }
            Some(worker_execution_error::Error::PreviousInvocationFailed(error)) => {
                Some(error.error.clone().unwrap().try_into().unwrap())
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn worker_error_logs(error: &Error) -> Option<String> {
    match error {
        Error::InternalError(error) => match &error.error {
            Some(worker_execution_error::Error::InvocationFailed(error)) => {
                Some(error.stderr.clone())
            }
            Some(worker_execution_error::Error::PreviousInvocationFailed(error)) => {
                Some(error.stderr.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn to_worker_metadata(
    metadata: &golem_api_grpc::proto::golem::worker::WorkerMetadata,
) -> (WorkerMetadata, Option<String>) {
    (
        WorkerMetadata {
            worker_id: metadata
                .worker_id
                .clone()
                .expect("no worker_id")
                .clone()
                .try_into()
                .expect("invalid worker_id"),
            args: metadata.args.clone(),
            env: metadata
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            wasi_config_vars: metadata
                .wasi_config_vars
                .clone()
                .expect("no wasi_config_vars_field")
                .into(),
            project_id: metadata
                .project_id
                .expect("no project_id")
                .try_into()
                .expect("invalid project_id"),
            created_by: metadata
                .created_by
                .clone()
                .expect("no account_id")
                .clone()
                .into(),
            created_at: (*metadata.created_at.as_ref().expect("no created_at")).into(),
            last_known_status: WorkerStatusRecord {
                oplog_idx: OplogIndex::default(),
                status: metadata.status.try_into().expect("invalid status"),
                overridden_retry_config: None, // not passed through gRPC
                skipped_regions: DeletedRegions::new(),
                pending_invocations: vec![],
                pending_updates: metadata
                    .updates
                    .iter()
                    .filter_map(|u| match &u.update {
                        Some(Update::Pending(_)) => Some(TimestampedUpdateDescription {
                            timestamp: (*u
                                .timestamp
                                .as_ref()
                                .expect("no timestamp on update record"))
                            .into(),
                            oplog_index: OplogIndex::from_u64(0),
                            description: UpdateDescription::Automatic {
                                target_version: u.target_version,
                            },
                        }),
                        _ => None,
                    })
                    .collect(),
                failed_updates: metadata
                    .updates
                    .iter()
                    .filter_map(|u| match &u.update {
                        Some(Update::Failed(failed_update)) => Some(FailedUpdateRecord {
                            timestamp: (*u
                                .timestamp
                                .as_ref()
                                .expect("no timestamp on update record"))
                            .into(),
                            target_version: u.target_version,
                            details: failed_update.details.clone(),
                        }),
                        _ => None,
                    })
                    .collect(),
                successful_updates: metadata
                    .updates
                    .iter()
                    .filter_map(|u| match &u.update {
                        Some(Update::Successful(_)) => Some(SuccessfulUpdateRecord {
                            timestamp: (*u
                                .timestamp
                                .as_ref()
                                .expect("no timestamp on update record"))
                            .into(),
                            target_version: u.target_version,
                        }),
                        _ => None,
                    })
                    .collect(),
                invocation_results: HashMap::new(),
                current_idempotency_key: None,
                component_version: metadata.component_version,
                component_size: metadata.component_size,
                total_linear_memory_size: metadata.total_linear_memory_size,
                owned_resources: metadata
                    .owned_resources
                    .iter()
                    .map(|desc| {
                        (
                            WorkerResourceId(desc.resource_id),
                            WorkerResourceDescription {
                                created_at: desc.created_at.expect("Missing created_at").into(),
                                resource_name: desc.resource_name.clone(),
                                resource_owner: desc.resource_owner.clone(),
                            },
                        )
                    })
                    .collect(),
                active_plugins: HashSet::from_iter(
                    metadata
                        .active_plugins
                        .iter()
                        .cloned()
                        .map(|id| id.try_into().expect("invalid plugin installation id")),
                ),
                deleted_regions: DeletedRegions::new(),
                current_retry_count: HashMap::new(),
                component_version_for_replay: metadata.component_version,
            },
            parent: None,
        },
        metadata.last_error.clone(),
    )
}

#[async_trait]
pub trait TestDslUnsafe {
    type Safe: TestDsl;

    fn component(&self, name: &str) -> StoreComponentBuilder<'_, Self::Safe>;

    async fn store_component_with(
        &self,
        name: &str,
        component_type: ComponentType,
        unique: bool,
        unverified: bool,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &[(&'static str, DynamicLinkedInstance)],
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> (ComponentId, ComponentName);

    async fn store_component_with_id(&self, name: &str, component_id: &ComponentId);

    async fn get_latest_component_metadata(&self, component_id: &ComponentId) -> ComponentMetadata;

    async fn update_component(&self, component_id: &ComponentId, name: &str) -> ComponentVersion;
    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        name: &str,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
    ) -> ComponentVersion;

    async fn update_component_with_env(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: &[(String, String)],
    ) -> ComponentVersion;

    async fn add_initial_component_file(&self, path: &Path) -> InitialComponentFileKey;

    async fn add_initial_component_files(
        &self,
        files: &[(&str, &str, ComponentFilePermissions)],
    ) -> Vec<(PathBuf, InitialComponentFile)>;

    async fn add_plugin_wasm(&self, name: &str) -> PluginWasmFileKey;

    async fn start_worker(&self, component_id: &ComponentId, name: &str) -> WorkerId;

    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> Result<WorkerId, Error>;
    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> WorkerId;
    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> Result<WorkerId, Error>;
    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> Option<(WorkerMetadata, Option<String>)>;

    async fn wait_for_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        timeout: Duration,
    ) -> WorkerMetadata;

    async fn wait_for_statuses(
        &self,
        worker_id: &WorkerId,
        statuses: &[WorkerStatus],
        timeout: Duration,
    ) -> WorkerMetadata;

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> (Option<ScanCursor>, Vec<(WorkerMetadata, Option<String>)>);
    async fn delete_worker(&self, worker_id: &WorkerId) -> ();

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<(), Error>;
    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<(), Error>;
    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Vec<Value>, Error>;
    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Vec<Value>, Error>;
    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Option<ValueAndType>, Error>;
    async fn invoke_and_await_typed_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Option<ValueAndType>, Error>;
    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, Error>;
    async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent>;
    async fn capture_output_forever(
        &self,
        worker_id: &WorkerId,
    ) -> (UnboundedReceiver<Option<LogEvent>>, Sender<()>);
    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>>;
    async fn log_output(&self, worker_id: &WorkerId);
    async fn resume(&self, worker_id: &WorkerId, force: bool);
    async fn interrupt(&self, worker_id: &WorkerId);
    async fn simulated_crash(&self, worker_id: &WorkerId);
    async fn auto_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion);
    async fn manual_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion);
    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> Vec<PublicOplogEntryWithIndex>;
    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> Vec<PublicOplogEntryWithIndex>;

    async fn check_oplog_is_queryable(&self, worker_id: &WorkerId);

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> Vec<ComponentFileSystemNode>;
    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> Bytes;

    async fn create_plugin(&self, definition: PluginDefinitionCreation);

    async fn delete_plugin(&self, account_id: AccountId, name: &str, version: &str);

    async fn install_plugin_to_component(
        &self,
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> PluginInstallationId;

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index: OplogIndex,
    );

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget);

    async fn cancel_invocation(&self, worker_id: &WorkerId, idempotency_key: &IdempotencyKey);
    async fn try_cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> crate::Result<bool>;

    async fn default_project(&self) -> ProjectId;

    async fn create_project(&self) -> ProjectId;

    async fn grant_full_project_access(
        &self,
        project_id: &ProjectId,
        grantee_account_id: &AccountId,
    );

    async fn get_account(&self, account_id: &AccountId) -> Account;
}

#[async_trait]
impl<T: TestDsl + Sync> TestDslUnsafe for T {
    type Safe = T;

    fn component(&self, name: &str) -> StoreComponentBuilder<'_, T> {
        StoreComponentBuilder::new(self, name)
    }

    async fn store_component_with(
        &self,
        name: &str,
        component_type: ComponentType,
        unique: bool,
        unverified: bool,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &[(&'static str, DynamicLinkedInstance)],
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> (ComponentId, ComponentName) {
        <T as TestDsl>::store_component_with(
            self,
            name,
            name,
            component_type,
            unique,
            unverified,
            files,
            dynamic_linking,
            env,
            project_id,
        )
        .await
    }

    async fn store_component_with_id(&self, name: &str, component_id: &ComponentId) {
        <T as TestDsl>::store_component_with_id(self, name, component_id).await
    }

    async fn get_latest_component_metadata(&self, component_id: &ComponentId) -> ComponentMetadata {
        <T as TestDsl>::get_latest_component_metadata(self, component_id)
            .await
            .expect("Failed to get latest component metadata")
    }

    async fn update_component(&self, component_id: &ComponentId, name: &str) -> ComponentVersion {
        <T as TestDsl>::update_component(self, component_id, name).await
    }

    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        name: &str,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
    ) -> ComponentVersion {
        <T as TestDsl>::update_component_with_files(self, component_id, name, files).await
    }

    async fn update_component_with_env(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: &[(String, String)],
    ) -> ComponentVersion {
        <T as TestDsl>::update_component_with_env(self, component_id, name, env).await
    }

    async fn add_initial_component_file(&self, path: &Path) -> InitialComponentFileKey {
        <T as TestDsl>::add_initial_component_file(self, path).await
    }

    async fn add_initial_component_files(
        &self,
        files: &[(&str, &str, ComponentFilePermissions)],
    ) -> Vec<(PathBuf, InitialComponentFile)> {
        <T as TestDsl>::add_initial_component_files(self, files).await
    }

    async fn add_plugin_wasm(&self, name: &str) -> PluginWasmFileKey {
        <T as TestDsl>::add_plugin_wasm(self, name)
            .await
            .expect("Failed to add plugin wasm")
    }

    async fn start_worker(&self, component_id: &ComponentId, name: &str) -> WorkerId {
        <T as TestDsl>::start_worker(self, component_id, name)
            .await
            .expect("Failed to start worker")
    }

    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> Result<WorkerId, Error> {
        <T as TestDsl>::try_start_worker(self, component_id, name)
            .await
            .expect("Failed to start worker")
    }

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> WorkerId {
        <T as TestDsl>::start_worker_with(self, component_id, name, args, env, wasi_config_vars)
            .await
            .expect("Failed to start worker")
    }

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> Result<WorkerId, Error> {
        <T as TestDsl>::try_start_worker_with(self, component_id, name, args, env, wasi_config_vars)
            .await
            .expect("Failed to start worker")
    }

    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> Option<(WorkerMetadata, Option<String>)> {
        <T as TestDsl>::get_worker_metadata(self, worker_id)
            .await
            .expect("Failed to get worker metadata")
    }

    async fn wait_for_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        timeout: Duration,
    ) -> WorkerMetadata {
        <T as TestDsl>::wait_for_status(self, worker_id, status, timeout)
            .await
            .expect("Failed to wait for status")
    }

    async fn wait_for_statuses(
        &self,
        worker_id: &WorkerId,
        statuses: &[WorkerStatus],
        timeout: Duration,
    ) -> WorkerMetadata {
        <T as TestDsl>::wait_for_statuses(self, worker_id, statuses, timeout)
            .await
            .expect("Failed to wait for status")
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> (Option<ScanCursor>, Vec<(WorkerMetadata, Option<String>)>) {
        <T as TestDsl>::get_workers_metadata(self, component_id, filter, cursor, count, precise)
            .await
            .expect("Failed to get workers metadata")
    }

    async fn delete_worker(&self, worker_id: &WorkerId) -> () {
        <T as TestDsl>::delete_worker(self, worker_id)
            .await
            .expect("Failed to delete worker")
    }

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<(), Error> {
        <T as TestDsl>::invoke(self, worker_id, function_name, params)
            .await
            .expect("Failed to invoke function")
    }

    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<(), Error> {
        <T as TestDsl>::invoke_with_key(self, worker_id, idempotency_key, function_name, params)
            .await
            .expect("Failed to invoke function")
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Vec<Value>, Error> {
        <T as TestDsl>::invoke_and_await(self, worker_id, function_name, params)
            .await
            .expect("Failed to invoke function")
    }
    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Vec<Value>, Error> {
        <T as TestDsl>::invoke_and_await_with_key(
            self,
            worker_id,
            idempotency_key,
            function_name,
            params,
        )
        .await
        .expect("Failed to invoke function")
    }
    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Option<ValueAndType>, Error> {
        <T as TestDsl>::invoke_and_await_typed(self, worker_id, function_name, params)
            .await
            .expect("Failed to invoke function")
    }
    async fn invoke_and_await_typed_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Result<Option<ValueAndType>, Error> {
        <T as TestDsl>::invoke_and_await_typed_with_key(
            self,
            worker_id,
            idempotency_key,
            function_name,
            params,
        )
        .await
        .expect("Failed to invoke function")
    }

    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, Error> {
        <T as TestDsl>::invoke_and_await_json(self, worker_id, function_name, params)
            .await
            .expect("Failed to invoke function")
    }

    async fn capture_output(&self, worker_id: &WorkerId) -> UnboundedReceiver<LogEvent> {
        <T as TestDsl>::capture_output(self, worker_id).await
    }

    async fn capture_output_forever(
        &self,
        worker_id: &WorkerId,
    ) -> (UnboundedReceiver<Option<LogEvent>>, Sender<()>) {
        <T as TestDsl>::capture_output_forever(self, worker_id).await
    }

    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> UnboundedReceiver<Option<LogEvent>> {
        <T as TestDsl>::capture_output_with_termination(self, worker_id).await
    }

    async fn log_output(&self, worker_id: &WorkerId) {
        <T as TestDsl>::log_output(self, worker_id).await
    }

    async fn resume(&self, worker_id: &WorkerId, force: bool) {
        <T as TestDsl>::resume(self, worker_id, force)
            .await
            .expect("Failed to resume worker")
    }

    async fn interrupt(&self, worker_id: &WorkerId) {
        <T as TestDsl>::interrupt(self, worker_id)
            .await
            .expect("Failed to interrupt worker")
    }

    async fn simulated_crash(&self, worker_id: &WorkerId) {
        <T as TestDsl>::simulated_crash(self, worker_id)
            .await
            .expect("Failed to crash worker")
    }

    async fn auto_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion) {
        <T as TestDsl>::auto_update_worker(self, worker_id, target_version)
            .await
            .expect("Failed to update worker")
    }

    async fn manual_update_worker(&self, worker_id: &WorkerId, target_version: ComponentVersion) {
        <T as TestDsl>::manual_update_worker(self, worker_id, target_version)
            .await
            .expect("Failed to update worker")
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> Vec<PublicOplogEntryWithIndex> {
        <T as TestDsl>::get_oplog(self, worker_id, from)
            .await
            .expect("Failed to get oplog")
    }

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> Vec<PublicOplogEntryWithIndex> {
        <T as TestDsl>::search_oplog(self, worker_id, query)
            .await
            .expect("Failed to search oplog")
    }
    async fn check_oplog_is_queryable(&self, worker_id: &WorkerId) -> () {
        <T as TestDsl>::check_oplog_is_queryable(self, worker_id)
            .await
            .expect("Oplog check failed")
    }

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> Vec<ComponentFileSystemNode> {
        <T as TestDsl>::get_file_system_node(self, worker_id, path)
            .await
            .expect("Failed to get file system node")
    }

    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> Bytes {
        <T as TestDsl>::get_file_contents(self, worker_id, path)
            .await
            .expect("Failed to get file contents")
    }

    async fn create_plugin(&self, definition: PluginDefinitionCreation) {
        <T as TestDsl>::create_plugin(self, definition)
            .await
            .expect("Failed to create plugin")
    }

    async fn delete_plugin(&self, account_id: AccountId, name: &str, version: &str) {
        <T as TestDsl>::delete_plugin(self, account_id, name, version)
            .await
            .expect("Failed to delete plugin")
    }

    async fn install_plugin_to_component(
        &self,
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> PluginInstallationId {
        <T as TestDsl>::install_plugin_to_component(
            self,
            component_id,
            plugin_name,
            plugin_version,
            priority,
            parameters,
        )
        .await
        .expect("Failed to install plugin")
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index: OplogIndex,
    ) {
        <T as TestDsl>::fork_worker(self, source_worker_id, target_worker_id, oplog_index)
            .await
            .expect("Failed to fork worker")
    }

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget) {
        <T as TestDsl>::revert(self, worker_id, target)
            .await
            .expect("Failed to revert worker")
    }

    async fn cancel_invocation(&self, worker_id: &WorkerId, idempotency_key: &IdempotencyKey) {
        <T as TestDsl>::cancel_invocation(self, worker_id, idempotency_key)
            .await
            .expect("Failed to cancel invocation");
    }

    async fn try_cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> crate::Result<bool> {
        <T as TestDsl>::cancel_invocation(self, worker_id, idempotency_key).await
    }

    async fn default_project(&self) -> ProjectId {
        <T as TestDsl>::default_project(self)
            .await
            .expect("failed to get default project")
    }

    async fn create_project(&self) -> ProjectId {
        <T as TestDsl>::create_project(self)
            .await
            .expect("failed to create project")
    }

    async fn grant_full_project_access(
        &self,
        project_id: &ProjectId,
        grantee_account_id: &AccountId,
    ) {
        <T as TestDsl>::grant_full_project_access(self, project_id, grantee_account_id)
            .await
            .expect("failed to grant full project access")
    }

    async fn get_account(&self, account_id: &AccountId) -> Account {
        <T as TestDsl>::get_account(self, account_id)
            .await
            .expect("failed to get account")
    }
}

fn rename_component_if_needed(temp_dir: &Path, path: &Path, name: &str) -> anyhow::Result<PathBuf> {
    // Check metadata
    let source = std::fs::read(path)?;
    let metadata = RawComponentMetadata::analyse_component(&source)?;
    if metadata.root_package_name.is_none() || metadata.root_package_name == Some(name.to_string())
    {
        info!(
            "Name in metadata is {:?}, used component name is {}, using the original WASM: {:?}",
            metadata.root_package_name, name, path
        );
        Ok(path.to_path_buf())
    } else {
        let new_path = Builder::new().disable_cleanup(true).tempfile_in(temp_dir)?;
        let mut add_metadata = AddMetadata::default();
        add_metadata.name = AddMetadataField::Set(name.to_string());
        add_metadata.version = if let Some(v) = &metadata.root_package_version {
            AddMetadataField::Set(wasm_metadata::Version::new(v.to_string()))
        } else {
            AddMetadataField::Clear
        };

        info!(
            "Name in metadata is {:?}, used component name is {}, using an updated WASM: {:?}",
            metadata.root_package_name, name, new_path
        );

        let updated_wasm = add_metadata.to_wasm(&source)?;
        std::fs::write(&new_path, updated_wasm)?;
        Ok(new_path.path().to_path_buf())
    }
}
