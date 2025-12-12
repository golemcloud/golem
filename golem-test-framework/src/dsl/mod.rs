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

pub mod debug_render;

use self::debug_render::debug_render_oplog_entry;
use crate::components::redis::Redis;
use crate::model::IFSEntry;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use bytes::Bytes;
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error as WorkerGrpcError;
use golem_api_grpc::proto::golem::worker::v1::worker_execution_error;
use golem_api_grpc::proto::golem::worker::{log_event, LogEvent, StdErrLog, StdOutLog};
use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
use golem_common::base_model::{PromiseId, WorkerId};
use golem_common::model::account::AccountId;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName,
};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::component::PluginPriority;
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentFilePermissions, ComponentId, ComponentRevision,
    PluginInstallation,
};
use golem_common::model::component_metadata::{DynamicLinkedInstance, RawComponentMetadata};
use golem_common::model::deployment::{CurrentDeployment, DeploymentCreation, DeploymentVersion};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName,
};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::environment_share::{EnvironmentShare, EnvironmentShareCreation};
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use golem_common::model::worker::{
    FlatComponentFileSystemNode, RevertWorkerTarget, UpdateRecord, WorkerMetadataDto,
};
use golem_common::model::{IdempotencyKey, OplogIndex, ScanCursor, WorkerFilter, WorkerStatus};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_wasm::{Value, ValueAndType};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};
use tokio::fs::File;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot::Sender;
use tracing::{debug, info, Instrument};
use uuid::Uuid;
use wasm_metadata::{AddMetadata, AddMetadataField};

pub struct EnvironmentOptions {
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

#[async_trait]
// TestDsl for everything needed by the worker-executor tests
pub trait TestDsl {
    type WorkerInvocationResult<T>;

    fn redis(&self) -> Arc<dyn Redis>;

    fn component(
        &self,
        environment_id: &EnvironmentId,
        name: &str,
    ) -> StoreComponentBuilder<'_, Self> {
        StoreComponentBuilder::new(self, *environment_id, name.to_string())
    }

    async fn store_component_with(
        &self,
        wasm_name: &str,
        environment_id: EnvironmentId,
        name: &str,
        unique: bool,
        unverified: bool,
        files: Vec<IFSEntry>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        env: BTreeMap<String, String>,
        plugins: Vec<PluginInstallation>,
    ) -> anyhow::Result<ComponentDto>;

    async fn get_latest_component_version(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto>;

    async fn update_component(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> anyhow::Result<ComponentDto> {
        let latest_version = self.get_latest_component_version(component_id).await?;
        self.update_component_with(
            component_id,
            latest_version.revision,
            Some(name),
            Vec::new(),
            Vec::new(),
            None,
            None,
        )
        .await
    }

    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        name: &str,
        files: Vec<IFSEntry>,
    ) -> anyhow::Result<ComponentDto> {
        let latest_version = self.get_latest_component_version(component_id).await?;
        self.update_component_with(
            component_id,
            latest_version.revision,
            Some(name),
            files,
            latest_version.files.into_iter().map(|f| f.path).collect(),
            None,
            None,
        )
        .await
    }

    async fn update_component_with_env(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: &[(String, String)],
    ) -> anyhow::Result<ComponentDto> {
        let latest_version = self.get_latest_component_version(component_id).await?;
        self.update_component_with(
            component_id,
            latest_version.revision,
            Some(name),
            Vec::new(),
            Vec::new(),
            None,
            Some(BTreeMap::from_iter(env.to_vec())),
        )
        .await
    }

    async fn update_component_with(
        &self,
        component_id: &ComponentId,
        previous_version: ComponentRevision,
        wasm_name: Option<&str>,
        new_files: Vec<IFSEntry>,
        removed_files: Vec<ComponentFilePath>,
        dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        env: Option<BTreeMap<String, String>>,
    ) -> anyhow::Result<ComponentDto>;

    async fn try_start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> Self::WorkerInvocationResult<WorkerId> {
        self.try_start_worker_with(component_id, name, HashMap::new(), vec![])
            .await
    }

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> Self::WorkerInvocationResult<WorkerId>;

    async fn start_worker(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> anyhow::Result<WorkerId> {
        self.start_worker_with(component_id, name, HashMap::new(), vec![])
            .await
    }

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<WorkerId>;

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Self::WorkerInvocationResult<()> {
        self.invoke_with_key(worker_id, &IdempotencyKey::fresh(), function_name, params)
            .await
    }

    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Self::WorkerInvocationResult<()>;

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Self::WorkerInvocationResult<Vec<Value>> {
        self.invoke_and_await_with_key(worker_id, &IdempotencyKey::fresh(), function_name, params)
            .await
    }

    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Self::WorkerInvocationResult<Vec<Value>>;

    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> Self::WorkerInvocationResult<Option<ValueAndType>> {
        self.invoke_and_await_typed_with_key(
            worker_id,
            &IdempotencyKey::fresh(),
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
    ) -> Self::WorkerInvocationResult<Option<ValueAndType>>;

    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> Self::WorkerInvocationResult<Option<ValueAndType>> {
        self.invoke_and_await_json_with_key(
            worker_id,
            &IdempotencyKey::fresh(),
            function_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_json_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> Self::WorkerInvocationResult<Option<ValueAndType>>;

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget) -> anyhow::Result<()>;

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>>;

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>>;

    async fn check_oplog_is_queryable(&self, worker_id: &WorkerId) -> crate::Result<()> {
        let entries = self.get_oplog(worker_id, OplogIndex::INITIAL).await?;

        for entry in entries {
            debug_render_oplog_entry(&entry.entry);
        }

        Ok(())
    }

    async fn interrupt_with_optional_recovery(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
    ) -> anyhow::Result<()>;

    async fn interrupt(&self, worker_id: &WorkerId) -> anyhow::Result<()> {
        self.interrupt_with_optional_recovery(worker_id, false)
            .await
    }

    async fn simulated_crash(&self, worker_id: &WorkerId) -> anyhow::Result<()> {
        self.interrupt_with_optional_recovery(worker_id, true).await
    }

    async fn resume(&self, worker_id: &WorkerId, force: bool) -> anyhow::Result<()>;

    async fn complete_promise(&self, promise_id: &PromiseId, data: Vec<u8>) -> anyhow::Result<()>;

    async fn make_worker_log_event_stream(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<impl WorkerLogEventStream>;

    async fn capture_output(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<UnboundedReceiver<LogEvent>> {
        let mut stream = self.make_worker_log_event_stream(worker_id).await?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(
            async move {
                while let Some(event) = stream.message().await.expect("Failed to get message") {
                    debug!("Received event: {:?}", event);
                    let _ = tx.send(event);
                }

                debug!("Finished receiving events");
            }
            .in_current_span(),
        );
        Ok(rx)
    }

    async fn capture_output_with_termination(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<(UnboundedReceiver<Option<LogEvent>>, Sender<()>)> {
        let mut stream = self.make_worker_log_event_stream(worker_id).await?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (abort_tx, mut abort_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        msg = stream.message() => {
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
                            break;
                        }
                    }
                }

                debug!("Finished receiving events");

                let _ = tx.send(None);
            }
            .in_current_span(),
        );

        Ok((rx, abort_tx))
    }

    async fn log_output(&self, worker_id: &WorkerId) -> anyhow::Result<()> {
        let mut stream = self.make_worker_log_event_stream(worker_id).await?;
        tokio::spawn(
            async move {
                while let Some(event) = stream.message().await.expect("Failed to get message") {
                    info!("Received event: {:?}", event);
                }
                debug!("Finished receiving events");
            }
            .in_current_span(),
        );
        Ok(())
    }

    async fn auto_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentRevision,
    ) -> anyhow::Result<()>;

    async fn manual_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentRevision,
    ) -> anyhow::Result<()>;

    async fn delete_worker(&self, worker_id: &WorkerId) -> anyhow::Result<()>;

    async fn get_worker_metadata_opt(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadataDto>>;

    async fn get_worker_metadata(&self, worker_id: &WorkerId) -> anyhow::Result<WorkerMetadataDto> {
        match self.get_worker_metadata_opt(worker_id).await? {
            Some(worker_metadata) => Ok(worker_metadata),
            None => Err(anyhow!("Worker not found: {}", worker_id)),
        }
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> anyhow::Result<(Option<ScanCursor>, Vec<WorkerMetadataDto>)>;

    async fn wait_for_status(
        &self,
        worker_id: &WorkerId,
        status: WorkerStatus,
        timeout: Duration,
    ) -> anyhow::Result<WorkerMetadataDto> {
        self.wait_for_statuses(worker_id, &[status], timeout).await
    }

    async fn wait_for_statuses(
        &self,
        worker_id: &WorkerId,
        statuses: &[WorkerStatus],
        timeout: Duration,
    ) -> anyhow::Result<WorkerMetadataDto> {
        let start = Instant::now();
        let mut last_known = None;
        while start.elapsed() < timeout {
            let metadata = self.get_worker_metadata(worker_id).await?;

            if statuses.contains(&metadata.status) {
                return Ok(metadata);
            }

            last_known = Some(metadata.status.clone());
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

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> anyhow::Result<bool>;

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> anyhow::Result<Vec<FlatComponentFileSystemNode>>;

    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> anyhow::Result<Bytes>;

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_name: &str,
        oplog_index: OplogIndex,
    ) -> anyhow::Result<()>;
}

#[async_trait]
// TestDsl for multi service tests and benchmarks
pub trait TestDslExtended: TestDsl {
    fn account_id(&self) -> &AccountId;
    fn custom_request_port(&self) -> u16;

    async fn registry_service_client(&self) -> RegistryServiceClientLive;

    async fn app(&self) -> anyhow::Result<Application> {
        let client = self.registry_service_client().await;
        let app_name = ApplicationName(format!("app-{}", Uuid::new_v4()));

        let application = client
            .create_application(
                &self.account_id().0,
                &ApplicationCreation { name: app_name },
            )
            .await?;

        Ok(application)
    }

    async fn env(&self, application_id: &ApplicationId) -> anyhow::Result<Environment> {
        let client = self.registry_service_client().await;
        let env_name = EnvironmentName(format!("env-{}", Uuid::new_v4()));

        let environment = client
            .create_environment(
                &application_id.0,
                &EnvironmentCreation {
                    name: env_name,
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                },
            )
            .await?;

        Ok(environment)
    }

    async fn app_and_env(&self) -> anyhow::Result<(Application, Environment)> {
        self.app_and_env_custom(&EnvironmentOptions {
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
        })
        .await
    }

    async fn app_and_env_custom(
        &self,
        environment_options: &EnvironmentOptions,
    ) -> anyhow::Result<(Application, Environment)> {
        let client = self.registry_service_client().await;
        let app_name = ApplicationName(format!("app-{}", Uuid::new_v4()));
        let env_name = EnvironmentName(format!("env-{}", Uuid::new_v4()));

        let application = client
            .create_application(
                &self.account_id().0,
                &ApplicationCreation { name: app_name },
            )
            .await?;

        let environment = client
            .create_environment(
                &application.id.0,
                &EnvironmentCreation {
                    name: env_name,
                    compatibility_check: environment_options.compatibility_check,
                    version_check: environment_options.version_check,
                    security_overrides: environment_options.security_overrides,
                },
            )
            .await?;

        Ok((application, environment))
    }

    async fn share_environment(
        &self,
        environment_id: &EnvironmentId,
        grantee_account_id: &AccountId,
        roles: &[EnvironmentRole],
    ) -> anyhow::Result<EnvironmentShare> {
        let client = self.registry_service_client().await;

        let environment_share = client
            .create_environment_share(
                &environment_id.0,
                &EnvironmentShareCreation {
                    grantee_account_id: *grantee_account_id,
                    roles: BTreeSet::from_iter(roles.iter().copied()),
                },
            )
            .await?;

        Ok(environment_share)
    }

    async fn register_domain(&self, environment_id: &EnvironmentId) -> anyhow::Result<Domain> {
        let client = self.registry_service_client().await;

        let domain = Domain(format!(
            "{}.api.golem.cloud",
            Uuid::new_v4().to_string().to_lowercase()
        ));

        let domain_registration = client
            .create_domain_registration(&environment_id.0, &DomainRegistrationCreation { domain })
            .await?;

        Ok(domain_registration.domain)
    }

    async fn get_environment(&self, environment_id: &EnvironmentId) -> anyhow::Result<Environment> {
        let client = self.registry_service_client().await;
        let environment = client.get_environment(&environment_id.0).await?;
        Ok(environment)
    }

    async fn deploy_environment(
        &self,
        environment_id: &EnvironmentId,
    ) -> anyhow::Result<CurrentDeployment> {
        let client = self.registry_service_client().await;

        let plan = client
            .get_environment_deployment_plan(&environment_id.0)
            .await?;

        let deployment = client
            .deploy_environment(
                &environment_id.0,
                &DeploymentCreation {
                    current_revision: plan.current_revision,
                    expected_deployment_hash: plan.deployment_hash,
                    version: DeploymentVersion(Uuid::new_v4().to_string()),
                },
            )
            .await?;

        Ok(deployment)
    }
}

pub struct StoreComponentBuilder<'a, Dsl: TestDsl + ?Sized> {
    dsl: &'a Dsl,
    environment_id: EnvironmentId,
    name: String,
    wasm_name: String,
    unique: bool,
    unverified: bool,
    files: Vec<IFSEntry>,
    dynamic_linking: HashMap<String, DynamicLinkedInstance>,
    env: BTreeMap<String, String>,
    plugins: Vec<PluginInstallation>,
}

impl<'a, Dsl: TestDsl + ?Sized> StoreComponentBuilder<'a, Dsl> {
    pub fn new(dsl: &'a Dsl, environment_id: EnvironmentId, name: String) -> Self {
        Self {
            dsl,
            environment_id,
            wasm_name: name.clone(),
            name,
            unique: false,
            unverified: false,
            files: Vec::new(),
            dynamic_linking: HashMap::new(),
            env: BTreeMap::new(),
            plugins: Vec::new(),
        }
    }

    /// Set the name of the component.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
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
    pub fn with_files(mut self, files: &[IFSEntry]) -> Self {
        self.files = files.to_vec();
        self
    }

    /// Adds an initial file to the component
    pub fn add_file(
        mut self,
        target: &str,
        source: &str,
        permissions: ComponentFilePermissions,
    ) -> anyhow::Result<Self> {
        let source_path = PathBuf::from(source);
        let target_path = ComponentFilePath::from_abs_str(target).map_err(|e| anyhow!(e))?;
        let ifs_entry = IFSEntry {
            source_path,
            target_path,
            permissions,
        };

        self.files.push(ifs_entry);
        Ok(self)
    }

    pub fn add_ro_file(self, target: &str, source: &str) -> anyhow::Result<Self> {
        self.add_file(target, source, ComponentFilePermissions::ReadOnly)
    }

    pub fn add_rw_file(self, target: &str, source: &str) -> anyhow::Result<Self> {
        self.add_file(target, source, ComponentFilePermissions::ReadWrite)
    }

    /// Set the dynamic linking for the component
    pub fn with_dynamic_linking(
        mut self,
        dynamic_linking: &[(&str, DynamicLinkedInstance)],
    ) -> Self {
        self.dynamic_linking = dynamic_linking
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        self
    }

    /// Adds a dynamic linked instance to the component
    pub fn add_dynamic_linking(mut self, name: &str, instance: DynamicLinkedInstance) -> Self {
        self.dynamic_linking.insert(name.to_string(), instance);
        self
    }

    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        let map = env.into_iter().collect::<BTreeMap<_, _>>();
        self.env = map;
        self
    }

    pub fn with_plugin(
        self,
        environment_plugin_id: &EnvironmentPluginGrantId,
        priority: i32,
    ) -> Self {
        self.with_parametrized_plugin(environment_plugin_id, priority, BTreeMap::new())
    }

    pub fn with_parametrized_plugin(
        mut self,
        environment_plugin_id: &EnvironmentPluginGrantId,
        priority: i32,
        parameters: BTreeMap<String, String>,
    ) -> Self {
        self.plugins.push(PluginInstallation {
            environment_plugin_grant_id: *environment_plugin_id,
            priority: PluginPriority(priority),
            parameters,
        });
        self
    }

    /// Stores the component and returns the final component name too which is useful when used
    /// together with unique
    pub async fn store(self) -> anyhow::Result<ComponentDto> {
        self.dsl
            .store_component_with(
                &self.wasm_name,
                self.environment_id,
                &self.name,
                self.unique,
                self.unverified,
                self.files,
                self.dynamic_linking,
                self.env,
                self.plugins,
            )
            .await
    }
}

pub fn update_counts(metadata: &WorkerMetadataDto) -> (usize, usize, usize) {
    let mut pending_updates = 0;
    let mut successful_updates = 0;
    let mut failed_updates = 0;

    for update in &metadata.updates {
        match update {
            UpdateRecord::PendingUpdate(_) => pending_updates += 1,
            UpdateRecord::SuccessfulUpdate(_) => successful_updates += 1,
            UpdateRecord::FailedUpdate(_) => failed_updates += 1,
        }
    }

    (pending_updates, successful_updates, failed_updates)
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

pub fn is_worker_execution_error(
    got: &WorkerGrpcError,
    expected: &worker_execution_error::Error,
) -> bool {
    matches!(got, WorkerGrpcError::InternalError(error) if error.error.as_ref() == Some(expected))
}

pub fn worker_error_message(error: &WorkerExecutorError) -> String {
    match error {
        WorkerExecutorError::InvalidRequest { details } => details.clone(),
        WorkerExecutorError::WorkerAlreadyExists { worker_id } => {
            format!("Worker already exists: {:?}", worker_id)
        }
        WorkerExecutorError::WorkerCreationFailed { worker_id, details } => {
            format!("Worker creation failed: {:?}: {}", worker_id, details)
        }
        WorkerExecutorError::FailedToResumeWorker { worker_id, .. } => {
            format!("Failed to resume worker: {:?}", worker_id)
        }
        WorkerExecutorError::ComponentDownloadFailed {
            component_id,
            component_version,
            reason,
        } => format!(
            "Failed to download component: {:?} version {}: {}",
            component_id, component_version, reason
        ),
        WorkerExecutorError::ComponentParseFailed {
            component_id,
            component_version,
            reason,
        } => format!(
            "Failed to parse component: {:?} version {}: {}",
            component_id, component_version, reason
        ),
        WorkerExecutorError::GetLatestVersionOfComponentFailed {
            component_id,
            reason,
        } => format!(
            "Failed to get latest version of component: {:?}: {}",
            component_id, reason
        ),
        WorkerExecutorError::PromiseNotFound { promise_id } => {
            format!("Promise not found: {:?}", promise_id)
        }
        WorkerExecutorError::PromiseDropped { promise_id } => {
            format!("Promise dropped: {:?}", promise_id)
        }
        WorkerExecutorError::PromiseAlreadyCompleted { promise_id } => {
            format!("Promise already completed: {:?}", promise_id)
        }
        WorkerExecutorError::Interrupted { kind } => {
            if *kind == InterruptKind::Restart {
                "Simulated crash".to_string()
            } else {
                "Interrupted via the Golem API".to_string()
            }
        }
        WorkerExecutorError::ParamTypeMismatch { .. } => "Parameter type mismatch".to_string(),
        WorkerExecutorError::NoValueInMessage => "No value in message".to_string(),
        WorkerExecutorError::ValueMismatch { details } => {
            format!("Value mismatch: {}", details)
        }
        WorkerExecutorError::UnexpectedOplogEntry { expected, got } => format!(
            "Unexpected oplog entry; Expected: {}, got: {}",
            expected, got
        ),
        WorkerExecutorError::Runtime { details } => {
            format!("Runtime error: {}", details)
        }
        WorkerExecutorError::InvalidShardId {
            shard_id,
            shard_ids,
        } => format!("Invalid shard id: {:?}; ids: {:?}", shard_id, shard_ids),
        WorkerExecutorError::PreviousInvocationFailed { .. } => {
            "Previous invocation failed".to_string()
        }
        WorkerExecutorError::Unknown { details } => {
            format!("Unknown error: {}", details)
        }
        WorkerExecutorError::PreviousInvocationExited => "Previous invocation exited".to_string(),
        WorkerExecutorError::InvalidAccount => "Invalid account id".to_string(),
        WorkerExecutorError::WorkerNotFound { worker_id } => {
            format!("Worker not found: {:?}", worker_id)
        }
        WorkerExecutorError::ShardingNotReady => "Sharing not ready".to_string(),
        WorkerExecutorError::InitialComponentFileDownloadFailed { reason, .. } => {
            format!("Initial File download failed: {}", reason)
        }
        WorkerExecutorError::FileSystemError { reason, .. } => {
            format!("File system error: {}", reason)
        }
        WorkerExecutorError::InvocationFailed { .. } => "Invocation failed".to_string(),
    }
}

pub fn worker_error_underlying_error(
    error: &WorkerExecutorError,
) -> Option<golem_common::model::oplog::WorkerError> {
    match error {
        WorkerExecutorError::InvocationFailed { error, .. } => Some(error.clone()),
        WorkerExecutorError::PreviousInvocationFailed { error, .. } => Some(error.clone()),
        _ => None,
    }
}

pub fn worker_error_logs(error: &WorkerExecutorError) -> Option<String> {
    match error {
        WorkerExecutorError::InvocationFailed { stderr, .. } => Some(stderr.clone()),
        WorkerExecutorError::PreviousInvocationFailed { stderr, .. } => Some(stderr.clone()),
        _ => None,
    }
}

pub fn rename_component_if_needed(
    temp_dir: &Path,
    path: &Path,
    name: &str,
) -> anyhow::Result<PathBuf> {
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

pub async fn build_ifs_archive(
    component_directory: &Path,
    ifs_files: &[IFSEntry],
) -> anyhow::Result<(TempDir, PathBuf)> {
    static ARCHIVE_NAME: &str = "ifs.zip";

    let temp_dir = tempfile::Builder::new()
        .prefix("golem-test-framework-ifs-zip")
        .tempdir()?;
    let temp_file = File::create(temp_dir.path().join(ARCHIVE_NAME)).await?;
    let mut zip_writer = ZipFileWriter::with_tokio(temp_file);

    for ifs_file in ifs_files {
        zip_writer
            .write_entry_whole(
                ZipEntryBuilder::new(
                    ifs_file.target_path.to_string().into(),
                    Compression::Deflate,
                ),
                &(tokio::fs::read(&component_directory.join(&ifs_file.source_path))
                    .await
                    .with_context(|| {
                        format!("source file path: {}", ifs_file.source_path.display())
                    })?),
            )
            .await?;
    }

    zip_writer.close().await?;
    let file_path = temp_dir.path().join(ARCHIVE_NAME);
    Ok((temp_dir, file_path))
}

#[async_trait]
pub trait WorkerLogEventStream: 'static + Send {
    async fn message(&mut self) -> anyhow::Result<Option<LogEvent>>;
}
