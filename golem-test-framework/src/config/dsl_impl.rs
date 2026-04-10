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

use crate::components::redis::Redis;
use crate::config::TestDependencies;
use crate::dsl::{
    EnvironmentOptions, TestDsl, TestDslExtended, WorkerLogEventStream, build_ifs_archive,
    rename_component_if_needed,
};
use crate::model::IFSEntry;
use anyhow::{Context, anyhow};
use applying::Apply;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_client::api::{
    AgentClient, RegistryServiceClient, RegistryServiceClientLive, WorkerClient, WorkerClientLive,
    WorkerError,
};
use golem_client::model::{CompleteParameters, UpdateWorkerRequest, WorkersMetadataRequest};
use golem_common::base_model::agent::{DataValue, ParsedAgentId};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::extraction::extract_agent_types;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName,
};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{AgentConfigEntry, ComponentCreation, ComponentUpdate};
use golem_common::model::component::{
    ComponentDto, ComponentFileOptions, ComponentFilePath, ComponentId, ComponentName,
    ComponentRevision, PluginInstallation, PluginInstallationAction,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::deployment::{CurrentDeployment, DeploymentCreation, DeploymentVersion};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName,
};
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use golem_common::model::worker::{
    AgentMetadataDto, AgentUpdateMode, FlatComponentFileSystemNode, RevertWorkerTarget,
    WorkerAgentConfigEntry,
};
use golem_common::model::{
    AgentEvent, AgentFilter, AgentId, IdempotencyKey, OplogIndex, PromiseId, ScanCursor,
};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::File;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::Payload;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tracing::{debug, trace};
use uuid::Uuid;

pub struct NameResolutionCache {
    app_names: Cache<ApplicationId, (), ApplicationName, String>,
    env_names: Cache<EnvironmentId, (), EnvironmentName, String>,
    component_revisions: Cache<(ComponentId, ComponentRevision), (), ComponentDto, String>,
}

impl Default for NameResolutionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl NameResolutionCache {
    pub fn new() -> Self {
        Self {
            app_names: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "app_names",
            ),
            env_names: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "env_names",
            ),
            component_revisions: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_revisions",
            ),
        }
    }

    pub async fn resolve_app_name(
        &self,
        id: &ApplicationId,
        client: &RegistryServiceClientLive,
    ) -> anyhow::Result<ApplicationName> {
        self.app_names
            .get_or_insert_simple(id, async || {
                let app = client
                    .get_application(&id.0)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(app.name)
            })
            .await
            .map_err(|e| anyhow!(e))
    }

    pub async fn resolve_env_name(
        &self,
        id: &EnvironmentId,
        client: &RegistryServiceClientLive,
    ) -> anyhow::Result<EnvironmentName> {
        self.env_names
            .get_or_insert_simple(id, async || {
                let env = client
                    .get_environment(&id.0)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(env.name)
            })
            .await
            .map_err(|e| anyhow!(e))
    }

    pub async fn resolve_component_at_revision(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
        client: &RegistryServiceClientLive,
    ) -> anyhow::Result<ComponentDto> {
        let key = (*component_id, revision);
        self.component_revisions
            .get_or_insert_simple(&key, async || {
                let component = client
                    .get_component_revision(&component_id.0, revision.get())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(component)
            })
            .await
            .map_err(|e| anyhow!(e))
    }

    pub async fn pre_fill_app(&self, id: ApplicationId, name: ApplicationName) {
        let _ = self
            .app_names
            .get_or_insert_simple(&id, async || Ok(name))
            .await;
    }

    pub async fn pre_fill_env(&self, id: EnvironmentId, name: EnvironmentName) {
        let _ = self
            .env_names
            .get_or_insert_simple(&id, async || Ok(name))
            .await;
    }
}

#[derive(Clone)]
pub struct TestUserContext<Deps> {
    pub deps: Deps,
    pub account_id: AccountId,
    pub account_email: AccountEmail,
    pub token: TokenSecret,
    pub auto_deploy_enabled: bool,
    pub name_cache: Arc<NameResolutionCache>,
    pub last_deployments: Arc<std::sync::RwLock<HashMap<EnvironmentId, DeploymentRevision>>>,
}

impl<Deps: TestDependencies> TestUserContext<Deps> {
    pub fn with_auto_deploy(self, enabled: bool) -> Self {
        Self {
            auto_deploy_enabled: enabled,
            ..self
        }
    }
}

#[async_trait]
impl<Deps: TestDependencies> TestDsl for TestUserContext<Deps> {
    type WorkerError = golem_client::Error<golem_client::api::WorkerError>;

    fn redis(&self) -> Arc<dyn Redis> {
        self.deps.redis()
    }

    async fn store_component_with(
        &self,
        wasm_name: &str,
        environment_id: EnvironmentId,
        name: &str,
        unique: bool,
        unverified: bool,
        files: Vec<IFSEntry>,
        env: BTreeMap<String, String>,
        config_vars: BTreeMap<String, String>,
        agent_config: Vec<AgentConfigEntry>,
        plugins: Vec<PluginInstallation>,
    ) -> anyhow::Result<ComponentDto> {
        let component_directory = self.deps.component_directory();
        let source_path = component_directory.join(format!("{wasm_name}.wasm"));
        let component_name = if unique {
            let uuid = Uuid::new_v4();
            ComponentName(format!("{name}---{uuid}"))
        } else {
            ComponentName(name.to_string())
        };

        let source_path = if !unverified {
            rename_component_if_needed(
                self.deps.borrow().temp_directory(),
                &source_path,
                &component_name.0,
            )
            .expect("Failed to verify and change component metadata")
        } else {
            source_path
        };

        let (_tmp_dir, maybe_files_archive) = if !files.is_empty() {
            let (tmp_dir, files_archive) = build_ifs_archive(component_directory, &files).await?;
            (Some(tmp_dir), Some(File::open(files_archive).await?))
        } else {
            (None, None)
        };

        let file_options = files
            .into_iter()
            .map(|f| {
                (
                    f.target_path,
                    ComponentFileOptions {
                        permissions: f.permissions,
                    },
                )
            })
            .apply(BTreeMap::from_iter);

        let client = self.deps.registry_service().client(&self.token).await;

        let agent_types = extract_agent_types(&source_path, false, true).await?;

        trace!("Agent types in component {component_name}:\n{agent_types:#?}");

        let component = client
            .create_component(
                &environment_id.0,
                &ComponentCreation {
                    component_name,
                    file_options,
                    env,
                    config_vars,
                    agent_config,
                    plugins,
                    agent_types,
                },
                File::open(source_path).await?,
                maybe_files_archive,
            )
            .await?;

        if self.auto_deploy_enabled {
            self.deploy_environment(component.environment_id).await?;
        }

        Ok(component)
    }

    async fn get_latest_component_revision(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto> {
        let client = self.deps.registry_service().client(&self.token).await;
        let component = client.get_component(&component_id.0).await?;
        Ok(component)
    }

    async fn get_component_at_revision(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
    ) -> anyhow::Result<ComponentDto> {
        let client = self.deps.registry_service().client(&self.token).await;
        let component = client
            .get_component_revision(&component_id.0, revision.get())
            .await?;
        Ok(component)
    }

    async fn update_component_with(
        &self,
        component_id: &ComponentId,
        previous_revision: ComponentRevision,
        wasm_name: Option<&str>,
        new_files: Vec<IFSEntry>,
        removed_files: Vec<ComponentFilePath>,
        env: Option<BTreeMap<String, String>>,
        config_vars: Option<BTreeMap<String, String>>,
        agent_config: Option<Vec<AgentConfigEntry>>,
        plugin_updates: Vec<PluginInstallationAction>,
    ) -> anyhow::Result<ComponentDto> {
        let component_directory = self.deps.component_directory();
        let client = self.deps.registry_service().client(&self.token).await;

        let updated_wasm = if let Some(wasm_name) = wasm_name {
            let source_path: PathBuf = component_directory.join(format!("{wasm_name}.wasm"));

            let component = client.get_component(&component_id.0).await?;

            let source_path = rename_component_if_needed(
                self.deps.borrow().temp_directory(),
                &source_path,
                &component.component_name.0,
            )?;

            let agent_types = extract_agent_types(&source_path, false, true).await?;

            Some((File::open(source_path).await?, agent_types))
        } else {
            None
        };

        let (_tmp_dir, maybe_new_files_archive) = if !new_files.is_empty() {
            let (tmp_dir, new_files_archive) =
                build_ifs_archive(component_directory, &new_files).await?;
            (Some(tmp_dir), Some(File::open(new_files_archive).await?))
        } else {
            (None, None)
        };

        let new_file_options = new_files
            .into_iter()
            .map(|f| {
                (
                    f.target_path,
                    ComponentFileOptions {
                        permissions: f.permissions,
                    },
                )
            })
            .apply(BTreeMap::from_iter);

        let component = client
            .update_component(
                &component_id.0,
                &ComponentUpdate {
                    current_revision: previous_revision,
                    new_file_options,
                    removed_files,
                    env,
                    config_vars,
                    agent_config,
                    agent_types: updated_wasm
                        .as_ref()
                        .map(|(_wasm, agent_types)| agent_types.clone()),
                    plugin_updates,
                },
                updated_wasm.map(|(wasm, _agent_types)| wasm),
                maybe_new_files_archive,
            )
            .await?;

        if self.auto_deploy_enabled {
            self.deploy_environment(component.environment_id).await?;
        }

        Ok(component)
    }

    async fn try_start_agent_with(
        &self,
        component_id: &ComponentId,
        id: ParsedAgentId,
        env: HashMap<String, String>,
        config_vars: HashMap<String, String>,
        agent_config: Vec<WorkerAgentConfigEntry>,
    ) -> anyhow::Result<Result<AgentId, Self::WorkerError>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let config_vars: BTreeMap<String, String> = config_vars.into_iter().collect();
        let response = client
            .launch_new_worker(
                &component_id.0,
                &golem_client::model::AgentCreationRequest {
                    name: id.to_string(),
                    env,
                    config_vars,
                    agent_config,
                },
            )
            .await;

        Ok(response.map(|r| r.agent_id))
    }

    async fn invoke_agent_with_key(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        idempotency_key: &IdempotencyKey,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<()> {
        let registry_client = self.registry_service_client().await;
        let app_name = self
            .name_cache
            .resolve_app_name(&component.application_id, &registry_client)
            .await?;
        let env_name = self
            .name_cache
            .resolve_env_name(&component.environment_id, &registry_client)
            .await?;

        let client = self
            .deps
            .worker_service()
            .agent_http_client(&self.token)
            .await;
        client
            .invoke_agent(
                Some(&idempotency_key.value),
                &golem_client::model::AgentInvocationRequest {
                    app_name: app_name.0,
                    env_name: env_name.0,
                    agent_type_name: agent_id.agent_type.0.clone(),
                    parameters: agent_id.parameters.clone().into(),
                    phantom_id: agent_id.phantom_id,
                    method_name: method_name.to_string(),
                    method_parameters: params.into(),
                    mode: golem_client::model::AgentInvocationMode::Schedule,
                    schedule_at: None,
                    idempotency_key: None,
                    deployment_revision: None,
                    owner_account_email: None,
                },
            )
            .await
            .map_err(|e| anyhow!("Agent invocation failed: {e}"))?;
        Ok(())
    }

    async fn invoke_and_await_agent_impl(
        &self,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        idempotency_key: Option<&IdempotencyKey>,
        deployment_revision: Option<DeploymentRevision>,
        method_name: &str,
        params: DataValue,
    ) -> anyhow::Result<DataValue> {
        let registry_client = self.registry_service_client().await;
        let app_name = self
            .name_cache
            .resolve_app_name(&component.application_id, &registry_client)
            .await?;
        let env_name = self
            .name_cache
            .resolve_env_name(&component.environment_id, &registry_client)
            .await?;

        let key = idempotency_key
            .cloned()
            .unwrap_or_else(IdempotencyKey::fresh);

        let client = self
            .deps
            .worker_service()
            .agent_http_client(&self.token)
            .await;
        let result = client
            .invoke_agent(
                Some(&key.value),
                &golem_client::model::AgentInvocationRequest {
                    app_name: app_name.0,
                    env_name: env_name.0,
                    agent_type_name: agent_id.agent_type.0.clone(),
                    parameters: agent_id.parameters.clone().into(),
                    phantom_id: agent_id.phantom_id,
                    method_name: method_name.to_string(),
                    method_parameters: params.into(),
                    mode: golem_client::model::AgentInvocationMode::Await,
                    schedule_at: None,
                    idempotency_key: None,
                    deployment_revision: deployment_revision.map(i64::from),
                    owner_account_email: None,
                },
            )
            .await?;

        match result.result {
            Some(untyped_json) => {
                let revision = ComponentRevision::new(
                    result
                        .component_revision
                        .ok_or_else(|| anyhow!("Missing component_revision in response"))?,
                )
                .context("Invalid component_revision in response")?;
                let component_at_rev = self
                    .name_cache
                    .resolve_component_at_revision(&component.id, revision, &registry_client)
                    .await?;
                let agent_type = component_at_rev
                    .metadata
                    .find_agent_type_by_name(&agent_id.agent_type)
                    .ok_or_else(|| anyhow!("Agent type not found: {}", agent_id.agent_type))?;
                let agent_method = agent_type
                    .methods
                    .iter()
                    .find(|method| method.name == method_name)
                    .ok_or_else(|| {
                        debug!("Agent method not found: {}", method_name);
                        debug!("In agent type: {:#?}", agent_type);
                        anyhow!("Agent method not found: {}", method_name)
                    })?;

                DataValue::try_from_untyped_json(untyped_json, agent_method.output_schema.clone())
                    .map_err(|err| anyhow!("DataValue conversion error: {err}"))
            }
            None => Ok(DataValue::Tuple(
                golem_common::base_model::agent::ElementValues { elements: vec![] },
            )),
        }
    }

    async fn revert(&self, agent_id: &AgentId, target: RevertWorkerTarget) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .revert_worker(&agent_id.component_id.0, &agent_id.agent_id, &target)
            .await?;
        Ok(())
    }

    async fn get_oplog(
        &self,
        agent_id: &AgentId,
        from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;

        let mut result = Vec::new();
        let mut cursor: Option<golem_client::model::OplogCursor> = None;

        loop {
            let response = client
                .get_oplog(
                    &agent_id.component_id.0,
                    &agent_id.agent_id,
                    Some(from.as_u64()),
                    100,
                    cursor.as_ref(),
                    None,
                )
                .await
                .map_err(|e| anyhow!("get_oplog failed for agent {agent_id}: {e}"))?;

            result.extend(response.entries);
            match response.next {
                None => break,
                Some(next_cursor) => cursor = Some(next_cursor),
            }
        }

        Ok(result)
    }

    async fn search_oplog(
        &self,
        agent_id: &AgentId,
        query: &str,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;

        let mut result = Vec::new();
        let mut cursor: Option<golem_client::model::OplogCursor> = None;

        loop {
            let response = client
                .get_oplog(
                    &agent_id.component_id.0,
                    &agent_id.agent_id,
                    None,
                    100,
                    cursor.as_ref(),
                    Some(query),
                )
                .await
                .map_err(|e| {
                    anyhow!("search_oplog failed for agent {agent_id}, query={query}: {e}")
                })?;

            result.extend(response.entries);
            match response.next {
                None => break,
                Some(next_cursor) => cursor = Some(next_cursor),
            }
        }

        Ok(result)
    }

    async fn interrupt_with_optional_recovery(
        &self,
        agent_id: &AgentId,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .interrupt_worker(
                &agent_id.component_id.0,
                &agent_id.agent_id,
                Some(recover_immediately),
            )
            .await?;
        Ok(())
    }

    async fn resume(&self, agent_id: &AgentId, _force: bool) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .resume_worker(&agent_id.component_id.0, &agent_id.agent_id)
            .await?;
        Ok(())
    }

    async fn complete_promise(&self, promise_id: &PromiseId, data: Vec<u8>) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .complete_promise(
                &promise_id.agent_id.component_id.0,
                &promise_id.agent_id.agent_id,
                &CompleteParameters {
                    oplog_idx: promise_id.oplog_idx.as_u64(),
                    data,
                },
            )
            .await?;
        Ok(())
    }

    async fn make_worker_log_event_stream(
        &self,
        agent_id: &AgentId,
    ) -> anyhow::Result<impl WorkerLogEventStream> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let stream = HttpWorkerLogEventStream::new(Arc::new(client), agent_id).await?;
        Ok(stream)
    }

    async fn auto_update_worker(
        &self,
        agent_id: &AgentId,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .update_worker(
                &agent_id.component_id.0,
                &agent_id.agent_id,
                &UpdateWorkerRequest {
                    mode: AgentUpdateMode::Automatic,
                    target_revision: target_revision.into(),
                    disable_wakeup: Some(disable_wakeup),
                },
            )
            .await?;
        Ok(())
    }

    async fn manual_update_worker(
        &self,
        agent_id: &AgentId,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .update_worker(
                &agent_id.component_id.0,
                &agent_id.agent_id,
                &UpdateWorkerRequest {
                    mode: AgentUpdateMode::Manual,
                    target_revision: target_revision.into(),
                    disable_wakeup: Some(disable_wakeup),
                },
            )
            .await?;
        Ok(())
    }

    async fn delete_worker(&self, agent_id: &AgentId) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .delete_worker(&agent_id.component_id.0, &agent_id.agent_id)
            .await?;
        Ok(())
    }

    async fn get_worker_metadata_opt(
        &self,
        agent_id: &AgentId,
    ) -> anyhow::Result<Option<AgentMetadataDto>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        match client
            .get_worker_metadata(&agent_id.component_id.0, &agent_id.agent_id)
            .await
        {
            Ok(worker_metadata) => Ok(Some(worker_metadata)),
            Err(golem_client::Error::Item(WorkerError::Error404(_))) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> anyhow::Result<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .find_workers_metadata(
                &component_id.0,
                &WorkersMetadataRequest {
                    filter,
                    cursor: Some(cursor),
                    count: Some(count),
                    precise: Some(precise),
                },
            )
            .await?;
        Ok((result.cursor, result.workers))
    }

    async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
    ) -> anyhow::Result<bool> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .cancel_invocation(
                &agent_id.component_id.0,
                &agent_id.agent_id,
                &idempotency_key.value,
            )
            .await?;
        Ok(result.canceled)
    }

    async fn get_file_system_node(
        &self,
        agent_id: &AgentId,
        path: &str,
    ) -> anyhow::Result<Vec<FlatComponentFileSystemNode>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .get_files(&agent_id.component_id.0, &agent_id.agent_id, path)
            .await?;
        Ok(result.nodes)
    }

    async fn get_file_contents(&self, agent_id: &AgentId, path: &str) -> anyhow::Result<Bytes> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .get_file_content(&agent_id.component_id.0, &agent_id.agent_id, path)
            .await?;
        Ok(result)
    }

    async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_name: &str,
        oplog_index: OplogIndex,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;

        client
            .fork_worker(
                &source_agent_id.component_id.0,
                &source_agent_id.agent_id,
                &golem_client::model::ForkWorkerRequest {
                    target_agent_id: AgentId {
                        component_id: source_agent_id.component_id,
                        agent_id: target_agent_name.to_string(),
                    },
                    oplog_index_cutoff: oplog_index.as_u64(),
                },
            )
            .await?;

        Ok(())
    }
}

#[async_trait]
impl<Deps: TestDependencies> TestDslExtended for TestUserContext<Deps> {
    fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    fn custom_request_port(&self) -> u16 {
        self.deps.worker_service().custom_request_port()
    }

    async fn registry_service_client(&self) -> RegistryServiceClientLive {
        self.deps.registry_service().client(&self.token).await
    }

    async fn app(&self) -> anyhow::Result<Application> {
        let client = self.registry_service_client().await;
        let app_name = ApplicationName(format!("app-{}", Uuid::new_v4()));
        let application = client
            .create_application(
                &self.account_id().0,
                &ApplicationCreation { name: app_name },
            )
            .await?;
        self.name_cache
            .pre_fill_app(application.id, application.name.clone())
            .await;
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
        self.name_cache
            .pre_fill_env(environment.id, environment.name.clone())
            .await;
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

        self.name_cache
            .pre_fill_app(application.id, application.name.clone())
            .await;
        self.name_cache
            .pre_fill_env(environment.id, environment.name.clone())
            .await;

        Ok((application, environment))
    }

    async fn deploy_environment_with(
        &self,
        environment_id: EnvironmentId,
        modify_deployment: impl for<'a> FnOnce(&'a mut DeploymentCreation) + Send,
    ) -> anyhow::Result<CurrentDeployment> {
        let client = self.registry_service_client().await;

        let plan = client
            .get_environment_deployment_plan(&environment_id.0)
            .await?;

        let mut deployment_creation = DeploymentCreation {
            current_revision: plan.current_revision,
            expected_deployment_hash: plan.deployment_hash,
            version: DeploymentVersion(Uuid::new_v4().to_string()),
            agent_secret_defaults: Vec::new(),
            quota_resource_defaults: Vec::new(),
            retry_policy_defaults: Vec::new(),
        };

        modify_deployment(&mut deployment_creation);

        let deployment = client
            .deploy_environment(&environment_id.0, &deployment_creation)
            .await?;

        self.last_deployments
            .write()
            .unwrap()
            .insert(environment_id, deployment.revision);

        Ok(deployment)
    }

    fn get_last_deployment_revision(
        &self,
        environment_id: &EnvironmentId,
    ) -> anyhow::Result<DeploymentRevision> {
        self.last_deployments
            .read()
            .unwrap()
            .get(environment_id)
            .copied()
            .ok_or_else(|| {
                anyhow!("No deployment revision recorded for environment {environment_id}")
            })
    }
}

struct HttpWorkerLogEventStream {
    read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl HttpWorkerLogEventStream {
    async fn new(client: Arc<WorkerClientLive>, agent_id: &AgentId) -> anyhow::Result<Self> {
        let url = format!(
            "ws://{}:{}/v1/components/{}/workers/{}/connect",
            client.context.base_url.host().unwrap(),
            client.context.base_url.port_or_known_default().unwrap(),
            agent_id.component_id.0,
            agent_id.agent_id,
        );

        let mut connection_request = url
            .into_client_request()
            .context("Failed to create request")?;

        {
            let headers = connection_request.headers_mut();

            if let Some(bearer_token) = client.context.bearer_token() {
                headers.insert("Authorization", format!("Bearer {bearer_token}").parse()?);
            }
        }

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            connection_request,
            None,
            false,
            Some(Connector::Plain),
        )
        .await?;
        let (mut write, read) = stream.split();

        static PING_HELLO: &str = "hello";
        tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                match write
                    .send(Message::Ping(Payload::from(PING_HELLO.as_bytes())))
                    .await
                {
                    Ok(_) => {}
                    Err(error) => break error,
                };
            }
        });

        Ok(Self { read })
    }
}

#[async_trait]
impl WorkerLogEventStream for HttpWorkerLogEventStream {
    async fn message(&mut self) -> anyhow::Result<Option<LogEvent>> {
        loop {
            match self.read.next().await {
                Some(Ok(message)) => match message {
                    Message::Text(payload) => {
                        return Ok(Some(
                            serde_json::from_str::<AgentEvent>(payload.as_str())?
                                .try_into()
                                .map_err(|error: String| anyhow!(error))?,
                        ));
                    }
                    Message::Binary(payload) => {
                        return Ok(Some(
                            serde_json::from_slice::<AgentEvent>(payload.as_slice())?
                                .try_into()
                                .map_err(|error: String| anyhow!(error))?,
                        ));
                    }
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(_) => return Ok(None),
                    Message::Frame(_) => {
                        panic!("Raw frames should not be received")
                    }
                },
                Some(Err(error)) => return Err(anyhow!(error)),
                None => return Ok(None),
            }
        }
    }
}
