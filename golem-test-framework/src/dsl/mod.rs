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

pub mod debug_render;

use self::debug_render::debug_render_oplog_entry;
use crate::components::redis::Redis;
use crate::model::IFSEntry;
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use bytes::Bytes;
use golem_api_grpc::proto::golem::worker::v1::agent_error::Error as WorkerGrpcError;
use golem_api_grpc::proto::golem::worker::v1::worker_execution_error;
use golem_api_grpc::proto::golem::worker::{LogEvent, StdErrLog, StdOutLog, log_event};
use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
use golem_common::base_model::{AgentId, PromiseId};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::{DataValue, ParsedAgentId};
use golem_common::model::application::{Application, ApplicationId};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::component::{
    AgentFilePermissions, AgentTypeProvisionConfigCreation, AgentTypeProvisionConfigUpdate,
    CanonicalFilePath, ComponentDto, ComponentId, ComponentRevision, PluginInstallation,
    PluginPriority,
};
use golem_common::model::component_metadata::RawComponentMetadata;
use golem_common::model::deployment::{CurrentDeployment, DeploymentCreation, DeploymentRevision};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::environment_share::{EnvironmentShare, EnvironmentShareCreation};
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use golem_common::model::worker::{
    AgentConfigEntryDto, AgentFileSystemNode, AgentMetadataDto, RevertWorkerTarget, UpdateRecord,
};
use golem_common::model::{AgentFilter, AgentStatus, IdempotencyKey, OplogIndex, ScanCursor};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};
use tokio::fs::File;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot::Sender;
use tracing::{Instrument, debug, info};
use uuid::Uuid;
use wasm_metadata::{AddMetadata, AddMetadataField};

/// Represents a test component whose analysis cache has been pre-warmed
/// during test-r dependency initialization. Tests that depend on a
/// `PrecompiledComponent` are guaranteed that the expensive `extract_agent_types`
/// and metadata analysis have already been performed.
#[derive(Clone, Debug)]
pub struct PrecompiledComponent {
    /// The WASM file name (without .wasm extension) in the test-components directory
    pub wasm_name: String,
    /// The WIT package name used as the component name (passed to `.name()`)
    pub package_name: String,
}

impl PrecompiledComponent {
    pub fn new(wasm_name: &str, package_name: &str) -> Self {
        Self {
            wasm_name: wasm_name.to_string(),
            package_name: package_name.to_string(),
        }
    }
}

pub struct EnvironmentOptions {
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

pub struct LogOutputGuard {
    abort_tx: Option<Sender<()>>,
}

impl LogOutputGuard {
    fn new(abort_tx: Sender<()>) -> Self {
        Self {
            abort_tx: Some(abort_tx),
        }
    }
}

impl Drop for LogOutputGuard {
    fn drop(&mut self) {
        if let Some(abort_tx) = self.abort_tx.take() {
            let _ = abort_tx.send(());
        }
    }
}

#[async_trait]
// TestDsl for everything needed by the worker-executor tests
pub trait TestDsl {
    type WorkerError: std::error::Error + Sync + Send + 'static;

    fn redis(&self) -> Arc<dyn Redis>;

    fn component(
        &self,
        environment_id: &EnvironmentId,
        name: &str,
    ) -> StoreComponentBuilder<'_, Self> {
        StoreComponentBuilder::new(self, *environment_id, name.to_string())
    }

    /// Creates a `StoreComponentBuilder` from a `PrecompiledComponent`, automatically
    /// setting both the WASM file name and the package name.
    fn component_dep(
        &self,
        environment_id: &EnvironmentId,
        precompiled: &PrecompiledComponent,
    ) -> StoreComponentBuilder<'_, Self> {
        StoreComponentBuilder::new(self, *environment_id, precompiled.wasm_name.clone())
            .name(&precompiled.package_name)
    }

    async fn store_component_with(
        &self,
        wasm_name: &str,
        environment_id: EnvironmentId,
        name: &str,
        unique: bool,
        unverified: bool,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfigCreation>,
        files_for_archive: Vec<IFSEntry>,
    ) -> anyhow::Result<ComponentDto>;

    async fn get_latest_component_revision(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto>;

    async fn get_component_at_revision(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
    ) -> anyhow::Result<ComponentDto>;

    async fn update_component(
        &self,
        component_id: &ComponentId,
        name: &str,
    ) -> anyhow::Result<ComponentDto> {
        let latest_revision = self.get_latest_component_revision(component_id).await?;
        self.update_component_with(
            component_id,
            latest_revision.revision,
            Some(name),
            None,
            Vec::new(),
        )
        .await
    }

    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        agent_type: &str,
        name: &str,
        files: Vec<IFSEntry>,
    ) -> anyhow::Result<ComponentDto> {
        use golem_common::model::component::{AgentFileOptions, AgentFilePath, ArchiveFilePath};
        let latest_revision = self.get_latest_component_revision(component_id).await?;
        // Collect all existing file paths for this agent type to remove them first
        let files_to_remove = latest_revision
            .metadata
            .agent_type_provision_configs()
            .get(&AgentTypeName(agent_type.to_string()))
            .map(|c| c.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        let files_to_add_or_update = files
            .iter()
            .map(|f| {
                (
                    ArchiveFilePath(f.target_path.clone()),
                    AgentFileOptions {
                        target_path: AgentFilePath(f.target_path.clone()),
                        permissions: f.permissions,
                    },
                )
            })
            .collect();
        let update = AgentTypeProvisionConfigUpdate {
            files_to_remove,
            files_to_add_or_update,
            ..Default::default()
        };
        self.update_component_with(
            component_id,
            latest_revision.revision,
            Some(name),
            Some(BTreeMap::from([(
                AgentTypeName(agent_type.to_string()),
                update,
            )])),
            files,
        )
        .await
    }

    async fn update_component_with_env(
        &self,
        component_id: &ComponentId,
        agent_type: &str,
        name: &str,
        env: &[(String, String)],
    ) -> anyhow::Result<ComponentDto> {
        let latest_revision = self.get_latest_component_revision(component_id).await?;
        let update = AgentTypeProvisionConfigUpdate {
            env: Some(BTreeMap::from_iter(env.iter().cloned())),
            ..Default::default()
        };
        self.update_component_with(
            component_id,
            latest_revision.revision,
            Some(name),
            Some(BTreeMap::from([(
                AgentTypeName(agent_type.to_string()),
                update,
            )])),
            Vec::new(),
        )
        .await
    }

    async fn update_component_with(
        &self,
        component_id: &ComponentId,
        previous_revision: ComponentRevision,
        wasm_name: Option<&str>,
        agent_type_provision_config_updates: Option<
            BTreeMap<AgentTypeName, AgentTypeProvisionConfigUpdate>,
        >,
        files_for_archive: Vec<IFSEntry>,
    ) -> anyhow::Result<ComponentDto>;

    async fn try_start_agent(
        &self,
        component_id: &ComponentId,
        id: ParsedAgentId,
    ) -> anyhow::Result<Result<AgentId, Self::WorkerError>> {
        self.try_start_agent_with(
            component_id,
            id,
            HashMap::new(),
            Vec::<AgentConfigEntryDto>::new(),
        )
        .await
    }

    async fn try_start_agent_with(
        &self,
        component_id: &ComponentId,
        id: ParsedAgentId,
        env: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
    ) -> anyhow::Result<Result<AgentId, Self::WorkerError>>;

    async fn start_agent(
        &self,
        component_id: &ComponentId,
        id: ParsedAgentId,
    ) -> anyhow::Result<AgentId> {
        self.start_agent_with(
            component_id,
            id,
            HashMap::new(),
            Vec::<AgentConfigEntryDto>::new(),
        )
        .await
    }

    async fn start_agent_with(
        &self,
        component_id: &ComponentId,
        id: ParsedAgentId,
        env: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
    ) -> anyhow::Result<AgentId> {
        let result = self
            .try_start_agent_with(component_id, id, env, config)
            .await?;
        Ok(result?)
    }

    async fn invoke_agent(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<()> {
        self.invoke_agent_with_key(
            component,
            agent_id,
            &IdempotencyKey::fresh(),
            method_name,
            params,
        )
        .await
    }

    async fn invoke_agent_with_key(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        idempotency_key: &IdempotencyKey,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<()>;

    async fn invoke_and_await_agent(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<DataValue> {
        self.invoke_and_await_agent_impl(component, agent_id, None, None, method_name, params)
            .await
    }

    async fn invoke_and_await_agent_at_deployment(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        deployment_revision: DeploymentRevision,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<DataValue> {
        self.invoke_and_await_agent_impl(
            component,
            agent_id,
            None,
            Some(deployment_revision),
            method_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_agent_with_key(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        idempotency_key: &IdempotencyKey,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<DataValue> {
        self.invoke_and_await_agent_impl(
            component,
            agent_id,
            Some(idempotency_key),
            None,
            method_name,
            params,
        )
        .await
    }

    async fn invoke_and_await_agent_impl(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        idempotency_key: Option<&IdempotencyKey>,
        deployment_revision: Option<DeploymentRevision>,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<DataValue>;

    async fn revert(&self, agent_id: &AgentId, target: RevertWorkerTarget) -> anyhow::Result<()>;

    async fn get_oplog(
        &self,
        agent_id: &AgentId,
        from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>>;

    async fn search_oplog(
        &self,
        agent_id: &AgentId,
        query: &str,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>>;

    async fn check_oplog_is_queryable(&self, agent_id: &AgentId) -> crate::Result<()> {
        let entries = self.get_oplog(agent_id, OplogIndex::INITIAL).await?;

        for entry in entries {
            debug!(
                "#{}:\n{}",
                entry.oplog_index,
                debug_render_oplog_entry(&entry.entry)
            );
        }

        Ok(())
    }

    async fn interrupt_with_optional_recovery(
        &self,
        agent_id: &AgentId,
        recover_immediately: bool,
    ) -> anyhow::Result<()>;

    async fn interrupt(&self, agent_id: &AgentId) -> anyhow::Result<()> {
        self.interrupt_with_optional_recovery(agent_id, false).await
    }

    async fn simulated_crash(&self, agent_id: &AgentId) -> anyhow::Result<()> {
        self.interrupt_with_optional_recovery(agent_id, true).await
    }

    async fn resume(&self, agent_id: &AgentId, force: bool) -> anyhow::Result<()>;

    async fn complete_promise(&self, promise_id: &PromiseId, data: Vec<u8>) -> anyhow::Result<()>;

    async fn make_worker_log_event_stream(
        &self,
        agent_id: &AgentId,
    ) -> anyhow::Result<impl WorkerLogEventStream>;

    async fn capture_output(
        &self,
        agent_id: &AgentId,
    ) -> anyhow::Result<UnboundedReceiver<LogEvent>> {
        let mut stream = self.make_worker_log_event_stream(agent_id).await?;
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
        agent_id: &AgentId,
    ) -> anyhow::Result<(UnboundedReceiver<Option<LogEvent>>, Sender<()>)> {
        let mut stream = self.make_worker_log_event_stream(agent_id).await?;
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

    async fn log_output(&self, agent_id: &AgentId) -> anyhow::Result<()> {
        let mut stream = self.make_worker_log_event_stream(agent_id).await?;
        tokio::spawn(
            async move {
                loop {
                    match stream.message().await {
                        Ok(Some(event)) => {
                            info!("Received event: {:?}", event);
                        }
                        Ok(None) => {
                            debug!("Finished receiving events");
                            break;
                        }
                        Err(err) => {
                            debug!("Log output stream closed: {err}");
                            break;
                        }
                    }
                }
            }
            .in_current_span(),
        );
        Ok(())
    }

    async fn log_output_scoped(&self, agent_id: &AgentId) -> anyhow::Result<LogOutputGuard> {
        let mut stream = self.make_worker_log_event_stream(agent_id).await?;
        let (abort_tx, mut abort_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(
            async move {
                loop {
                    tokio::select! {
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(event)) => {
                                    info!("Received event: {:?}", event);
                                }
                                Ok(None) => {
                                    debug!("Finished receiving events");
                                    break;
                                }
                                Err(err) => {
                                    debug!("Log output stream closed: {err}");
                                    break;
                                }
                            }
                        }
                        _ = (&mut abort_rx) => {
                            debug!("Aborting log output stream");
                            break;
                        }
                    }
                }
            }
            .in_current_span(),
        );

        Ok(LogOutputGuard::new(abort_tx))
    }

    async fn auto_update_worker(
        &self,
        agent_id: &AgentId,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
    ) -> anyhow::Result<()>;

    async fn manual_update_worker(
        &self,
        agent_id: &AgentId,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
    ) -> anyhow::Result<()>;

    async fn delete_worker(&self, agent_id: &AgentId) -> anyhow::Result<()>;

    async fn get_worker_metadata_opt(
        &self,
        agent_id: &AgentId,
    ) -> anyhow::Result<Option<AgentMetadataDto>>;

    async fn get_worker_metadata(&self, agent_id: &AgentId) -> anyhow::Result<AgentMetadataDto> {
        match self.get_worker_metadata_opt(agent_id).await? {
            Some(worker_metadata) => Ok(worker_metadata),
            None => Err(anyhow!("Worker not found: {}", agent_id)),
        }
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> anyhow::Result<(Option<ScanCursor>, Vec<AgentMetadataDto>)>;

    async fn wait_for_status(
        &self,
        agent_id: &AgentId,
        status: AgentStatus,
        timeout: Duration,
    ) -> anyhow::Result<AgentMetadataDto> {
        self.wait_for_statuses(agent_id, &[status], timeout).await
    }

    #[tracing::instrument(level = "info", skip(self, statuses, timeout), fields(%agent_id))]
    async fn wait_for_statuses(
        &self,
        agent_id: &AgentId,
        statuses: &[AgentStatus],
        timeout: Duration,
    ) -> anyhow::Result<AgentMetadataDto> {
        let start = Instant::now();
        let mut last_known = None;
        while start.elapsed() < timeout {
            let metadata = self.get_worker_metadata(agent_id).await?;

            if statuses.contains(&metadata.status) {
                return Ok(metadata);
            }

            last_known = Some(metadata.clone());
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

    #[tracing::instrument(level = "info", skip(self, timeout), fields(%agent_id))]
    async fn wait_for_component_revision(
        &self,
        agent_id: &AgentId,
        target_revision: ComponentRevision,
        timeout: Duration,
    ) -> anyhow::Result<AgentMetadataDto> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            let metadata = self.get_worker_metadata(agent_id).await?;

            if metadata.component_revision >= target_revision {
                return Ok(metadata);
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Err(anyhow!(
            "Timeout waiting for agent {agent_id} to reach component revision {target_revision}"
        ))
    }

    async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
    ) -> anyhow::Result<bool>;

    async fn get_file_system_node(
        &self,
        agent_id: &AgentId,
        path: &str,
    ) -> anyhow::Result<Vec<AgentFileSystemNode>>;

    async fn get_file_contents(&self, agent_id: &AgentId, path: &str) -> anyhow::Result<Bytes>;

    async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_name: &str,
        oplog_index: OplogIndex,
    ) -> anyhow::Result<()>;
}

#[async_trait]
// TestDsl for multi service tests and benchmarks
pub trait TestDslExtended: TestDsl {
    fn account_id(&self) -> &AccountId;
    fn custom_request_port(&self) -> u16;

    async fn registry_service_client(&self) -> RegistryServiceClientLive;

    async fn app(&self) -> anyhow::Result<Application>;

    async fn env(&self, application_id: &ApplicationId) -> anyhow::Result<Environment>;

    async fn app_and_env(&self) -> anyhow::Result<(Application, Environment)>;

    async fn app_and_env_custom(
        &self,
        environment_options: &EnvironmentOptions,
    ) -> anyhow::Result<(Application, Environment)>;

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
        environment_id: EnvironmentId,
    ) -> anyhow::Result<CurrentDeployment> {
        self.deploy_environment_with(environment_id, |_| {}).await
    }

    async fn deploy_environment_with(
        &self,
        environment_id: EnvironmentId,
        modify_deployment: impl for<'a> FnOnce(&'a mut DeploymentCreation) + Send,
    ) -> anyhow::Result<CurrentDeployment>;

    fn get_last_deployment_revision(
        &self,
        environment_id: &EnvironmentId,
    ) -> anyhow::Result<DeploymentRevision>;
}

pub struct StoreComponentBuilder<'a, Dsl: TestDsl + ?Sized> {
    dsl: &'a Dsl,
    environment_id: EnvironmentId,
    name: String,
    wasm_name: String,
    unique: bool,
    unverified: bool,
    agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfigCreation>,
    files_for_archive: Vec<IFSEntry>,
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
            agent_type_provision_configs: BTreeMap::new(),
            files_for_archive: Vec::new(),
        }
    }

    /// Set the name of the component.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Always create as a new component - otherwise, if the same component was already uploaded, it will be reused
    // TODO: CHECK IF WE CAN GET RID OF THIS FEATURE COMPLETELY IN THE FIRST CLASS AGENTS EPIC
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

    fn provision_config_entry_mut(
        &mut self,
        agent_type: &str,
    ) -> &mut AgentTypeProvisionConfigCreation {
        self.agent_type_provision_configs
            .entry(AgentTypeName(agent_type.to_string()))
            .or_default()
    }

    /// Set the initial files for an agent type.
    /// Populates both the provision config files map and the archive source list.
    pub fn with_files(mut self, agent_type: &str, files: &[IFSEntry]) -> Self {
        use golem_common::model::component::{AgentFileOptions, AgentFilePath, ArchiveFilePath};
        // Populate provision config: archive_path -> options
        let entry = self.provision_config_entry_mut(agent_type);
        for f in files {
            entry.files.insert(
                ArchiveFilePath(f.target_path.clone()),
                AgentFileOptions {
                    target_path: AgentFilePath(f.target_path.clone()),
                    permissions: f.permissions,
                },
            );
        }
        // Also accumulate source files for archive building
        self.files_for_archive.extend_from_slice(files);
        self
    }

    /// Adds an initial file to the component for a specific agent type.
    /// Populates both the provision config files map and the archive source list.
    pub fn add_file(
        mut self,
        agent_type: &str,
        target: &str,
        source: &str,
        permissions: AgentFilePermissions,
    ) -> anyhow::Result<Self> {
        use golem_common::model::component::{AgentFileOptions, AgentFilePath, ArchiveFilePath};
        let target_path = CanonicalFilePath::from_abs_str(target).map_err(|e| anyhow!(e))?;
        let entry = self.provision_config_entry_mut(agent_type);
        entry.files.insert(
            ArchiveFilePath(target_path.clone()),
            AgentFileOptions {
                target_path: AgentFilePath(target_path.clone()),
                permissions,
            },
        );
        self.files_for_archive.push(IFSEntry {
            source_path: std::path::PathBuf::from(source),
            target_path,
            permissions,
        });
        Ok(self)
    }

    pub fn add_ro_file(self, agent_type: &str, target: &str, source: &str) -> anyhow::Result<Self> {
        self.add_file(agent_type, target, source, AgentFilePermissions::ReadOnly)
    }

    pub fn add_rw_file(self, agent_type: &str, target: &str, source: &str) -> anyhow::Result<Self> {
        self.add_file(agent_type, target, source, AgentFilePermissions::ReadWrite)
    }

    pub fn with_env(mut self, agent_type: &str, env: Vec<(String, String)>) -> Self {
        let entry = self.provision_config_entry_mut(agent_type);
        entry.env = env.into_iter().collect();
        self
    }

    pub fn with_agent_config(mut self, agent_type: &str, config: Vec<AgentConfigEntryDto>) -> Self {
        let entry = self.provision_config_entry_mut(agent_type);
        entry.config = config;
        self
    }

    pub fn add_agent_config(mut self, agent_type: &str, config_entry: AgentConfigEntryDto) -> Self {
        let entry = self.provision_config_entry_mut(agent_type);
        entry.config.push(config_entry);
        self
    }

    pub fn with_plugin(
        self,
        agent_type: &str,
        environment_plugin_id: &EnvironmentPluginGrantId,
        priority: i32,
    ) -> Self {
        self.with_parametrized_plugin(agent_type, environment_plugin_id, priority, BTreeMap::new())
    }

    pub fn with_parametrized_plugin(
        mut self,
        agent_type: &str,
        environment_plugin_id: &EnvironmentPluginGrantId,
        priority: i32,
        parameters: BTreeMap<String, String>,
    ) -> Self {
        let entry = self.provision_config_entry_mut(agent_type);
        entry.plugin_installations.push(PluginInstallation {
            environment_plugin_grant_id: *environment_plugin_id,
            priority: PluginPriority(priority),
            parameters,
        });
        self
    }

    pub fn update_agent_provision_config(
        mut self,
        agent_type: &str,
        f: impl FnOnce(&mut AgentTypeProvisionConfigCreation),
    ) -> Self {
        f(self.provision_config_entry_mut(agent_type));
        self
    }

    pub fn try_update_agent_provision_config<E>(
        mut self,
        agent_type: &str,
        f: impl FnOnce(&mut AgentTypeProvisionConfigCreation) -> Result<(), E>,
    ) -> Result<Self, E> {
        f(self.provision_config_entry_mut(agent_type))?;
        Ok(self)
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
                self.agent_type_provision_configs,
                self.files_for_archive,
            )
            .await
    }
}

pub fn update_counts(metadata: &AgentMetadataDto) -> (usize, usize, usize) {
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
        Some(log_event::Event::PluginError(err)) => err.message.clone(),
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

    full_output
        .lines()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
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
        WorkerExecutorError::AgentAlreadyExists { agent_id } => {
            format!("Worker already exists: {:?}", agent_id)
        }
        WorkerExecutorError::AgentCreationFailed { agent_id, details } => {
            format!("Worker creation failed: {:?}: {}", agent_id, details)
        }
        WorkerExecutorError::FailedToResumeAgent { agent_id, .. } => {
            format!("Failed to resume worker: {:?}", agent_id)
        }
        WorkerExecutorError::ComponentDownloadFailed {
            component_id,
            component_revision,
            reason,
        } => format!(
            "Failed to download component: {:?} revision {}: {}",
            component_id, component_revision, reason
        ),
        WorkerExecutorError::ComponentParseFailed {
            component_id,
            component_revision,
            reason,
        } => format!(
            "Failed to parse component: {:?} revision {}: {}",
            component_id, component_revision, reason
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
        WorkerExecutorError::AgentNotFound { agent_id } => {
            format!("Worker not found: {:?}", agent_id)
        }
        WorkerExecutorError::ShardingNotReady => "Sharing not ready".to_string(),
        WorkerExecutorError::InitialAgentFileDownloadFailed { reason, .. } => {
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
) -> Option<golem_common::model::oplog::AgentError> {
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
