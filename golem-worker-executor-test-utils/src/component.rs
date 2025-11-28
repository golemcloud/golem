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

use crate::services::component::ComponentService;
use async_lock::RwLock;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentName, ComponentType, InitialComponentFile
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::service::compiled_component::CompiledComponentService;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::spawn_blocking;
use tracing::{debug, info, warn};
use wasmtime::{component::Component, Engine};
use golem_common::model::component_metadata::{
    ComponentMetadata, DynamicLinkedInstance, LinearMemory, RawComponentMetadata,
};
use golem_common::model::auth::EnvironmentRole;
use anyhow::{anyhow, Context};
use golem_common::model::agent::extraction::extract_agent_types;
use golem_wasm::analysis::AnalysedExport;
use golem_common::model::agent::AgentType;
use uuid::Uuid;

pub struct InMemoryComponentService {
    compiled_component_service: Arc<dyn CompiledComponentService>,
    registry: RwLock<ComponentRegistry>,
}

impl InMemoryComponentService {
    pub fn new(
        compiled_component_service: Arc<dyn CompiledComponentService>,
    ) -> Self {
        Self {
            compiled_component_service,
            registry: RwLock::new(ComponentRegistry::new()),
        }
    }

    async fn load_or_compile_component(
        &self,
        wasm_path: &Path,
        engine: &Engine,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        component_version: ComponentRevision,
    ) -> Result<Component, WorkerExecutorError> {
        let key = CacheKey {
            component_id: component_id.clone(),
            component_version,
        };
        let component_id_cloned = component_id.clone();
        let engine_cloned = engine.clone();
        let compiled_service = self.compiled_component_service.clone();
        let path = wasm_path.to_path_buf();

        let remote_res = compiled_service
            .get(environment_id, &component_id_cloned, component_version, &engine_cloned)
            .await;

        let remote_component = match remote_res {
            Ok(comp) => comp,
            Err(err) => {
                warn!("Failed to download compiled component {:?}: {}", key, err);
                None
            }
        };

        if let Some(c) = remote_component {
            return Ok(c);
        }

        // compile locally
        let bytes = tokio::fs::read(path).await?;
        let start = Instant::now();

        let compiled = spawn_blocking({
            let engine = engine_cloned.clone();
            let comp_id = component_id_cloned.clone();
            move || {
                Component::from_binary(&engine, &bytes).map_err(|e| {
                    WorkerExecutorError::ComponentParseFailed {
                        component_id: comp_id.clone(),
                        component_version,
                        reason: format!("{e}"),
                    }
                })
            }
        })
        .await
        .map_err(|join_err| WorkerExecutorError::unknown(join_err.to_string()))??;

        let elapsed = Instant::now().duration_since(start);
        debug!("Compiled {} in {}ms", component_id_cloned, elapsed.as_millis());

        // attempt to upload compiled component, but ignore if putting fails
        match compiled_service.put(environment_id, &component_id_cloned, component_version, &compiled).await {
            Ok(_) => (),
            Err(e) => warn!("Failed to upload compiled component {:?}: {}", key, e),
        }

        Ok(compiled)
    }

    async fn create_and_store_metadata(
        &self,
        source_path: &Path,
        component_name: &str,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        component_type: ComponentType,
        files: Vec<InitialComponentFile>,
        skip_analysis: bool,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        env: BTreeMap<String, String>,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
    ) -> anyhow::Result<ComponentDto> {
        if !source_path.exists() {
            return Err(anyhow!("Source file does not exist: {source_path:?}"));
        }

        // read wasm bytes (for hashing and analysis)
        let content = load_component_bytes(source_path).await?;
        let wasm_hash = {
            let hash = blake3::hash(&content);
            golem_common::model::diff::Hash::from(hash)
        };

        // analyze memories/exports if requested
        let (raw_metadata, memories, exports) = if skip_analysis {
            (RawComponentMetadata::default(), vec![], vec![])
        } else {
            analyze_component_bytes(&content)?
        };

        // extract agent types if requested
        let agent_types = if skip_analysis {
            vec![]
        } else {
            extract_agent_types(source_path, false, true)
                .await
                .map_err(|e| anyhow!("Failed analyzing component: {e}"))?
        };

        let size = content.len() as u64;

        let metadata = FullComponentMetadata {
            component_id: component_id.clone(),
            version: component_version,
            environment_id: environment_id.clone(),
            application_id: application_id.clone(),
            account_id: account_id.clone(),
            size,
            memories,
            exports,
            component_type,
            files,
            component_name: component_name.to_string(),
            dynamic_linking,
            env,
            wasm_hash,
            agent_types,
            environment_roles_from_shares,
            target_path: source_path.to_path_buf(),
            root_package_name: raw_metadata.root_package_name.clone(),
            root_package_version: raw_metadata.root_package_version.clone(),
        };

        {
            let mut reg = self.registry.write().await;
            reg.insert(metadata.clone());
        }

        info!(
            "Stored component {} version {} in memory",
            metadata.component_id, metadata.version
        );

        Ok(metadata.into())
    }

    async fn metadata_for_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        let key = CacheKey {
            component_id: component_id.clone(),
            component_version: version,
        };
        let reg = self.registry.read().await;
        reg.metadata
            .get(&key)
            .cloned()
            .map(|m| m.into())
            .ok_or(WorkerExecutorError::unknown(format!(
                "No such component found: {component_id}/{version}"
            )))
    }

    async fn latest_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        let reg = self.registry.read().await;
        let version = reg.latest_versions.get(component_id).cloned();
        match version {
            Some(v) => {
                let key = CacheKey {
                    component_id: component_id.clone(),
                    component_version: v,
                };
                reg.metadata
                    .get(&key)
                    .cloned()
                    .map(|m| m.into())
                    .ok_or(WorkerExecutorError::unknown(format!(
                        "No such component found: {component_id}/{v}"
                    )))
            }
            None => Err(WorkerExecutorError::unknown(
                "Could not find any component with the given id",
            )),
        }
    }

    pub async fn add_component_with_id(
        &self,
        local_path: &Path,
        component_id: &ComponentId,
        component_name: &str,
        component_type: ComponentType,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
    ) -> anyhow::Result<ComponentDto> {
        let component_version = ComponentRevision::INITIAL;

        let files = Vec::new();
        let env = BTreeMap::new();
        let dynamic_linking = HashMap::new();
        let skip_analysis = false;

        self.create_and_store_metadata(
            local_path,
            component_name,
            component_id,
            component_version,
            component_type,
            files,
            skip_analysis,
            dynamic_linking,
            env,
            environment_id,
            application_id,
            account_id,
            environment_roles_from_shares,
        )
        .await
    }

    pub async fn add_component(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: Vec<InitialComponentFile>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
        env: BTreeMap<String, String>,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
    ) -> anyhow::Result<ComponentDto> {
        let component_id = ComponentId(Uuid::new_v4());
        let version = ComponentRevision(0);

        let (raw, memories, exports) = if unverified {
            (RawComponentMetadata::default(), vec![], vec![])
        } else {
            Self::analyze_memories_and_exports(local_path).await?
        };

        let agent_types = if unverified {
            vec![]
        } else {
            extract_agent_types(local_path, false, true).await?
        };

        let metadata = FullComponentMetadata {
            component_id: component_id.clone(),
            version,
            environment_id: environment_id.clone(),
            application_id: application_id.clone(),
            account_id: account_id.clone(),
            component_name: name.to_string(),
            component_type,
            files,
            dynamic_linking,
            env,
            environment_roles_from_shares,
            target_path: local_path.to_path_buf(),
            size: tokio::fs::metadata(local_path).await?.len(),
            memories,
            exports,
            wasm_hash: {
                let bytes = tokio::fs::read(local_path).await?;
                golem_common::model::diff::Hash::from(blake3::hash(&bytes))
            },
            agent_types,
            root_package_name: raw.root_package_name.clone(),
            root_package_version: raw.root_package_version.clone(),
        };

        // Insert metadata into memory registry
        {
            let mut reg = self.registry.write().await;
            reg.insert(metadata);
        }

        Ok(self
            .latest_metadata(&component_id)
            .await
            .expect("just inserted, should exist"))
    }

    pub async fn get_or_add_component(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: Vec<InitialComponentFile>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
        env: BTreeMap<String, String>,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
    ) -> anyhow::Result<ComponentDto> {
        if let Some(existing_id) = self
            .registry
            .read()
            .await
            .id_by_name
            .get(&(environment_id.clone(), name.to_string()))
            .cloned()
        {
            return self.latest_metadata(&existing_id).await.map_err(|e| e.into());
        }

        self.add_component(
            local_path,
            &name,
            component_type,
            files,
            dynamic_linking,
            unverified,
            env,
            environment_id,
            application_id,
            account_id,
            environment_roles_from_shares,
        )
        .await
    }

    pub async fn update_component(
        &self,
        component_id: &ComponentId,
        local_path: Option<&Path>,
        component_type: Option<ComponentType>,
        new_files: Vec<InitialComponentFile>,
        removed_files: Vec<ComponentFilePath>,
        dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        env: Option<BTreeMap<String, String>>,
    ) -> anyhow::Result<ComponentDto> {
        // Find last version from registry
        let last_version = {
            let reg = self.registry.read().await;
            reg.latest_versions
                .get(component_id)
                .cloned()
                .unwrap_or(ComponentRevision(0))
        };

        // Compute new version (match previous behavior which used `.next().unwrap()`)
        let new_version = last_version.next().unwrap();

        // Load old metadata
        let old_metadata = {
            let reg = self.registry.read().await;
            reg.metadata
                .get(&CacheKey {
                    component_id: component_id.clone(),
                    component_version: last_version,
                })
                .cloned()
                .ok_or_else(|| anyhow!("failed to read existing metadata for update"))?
        };

        // Merge files: remove removed_files, add new_files
        let mut files = old_metadata.files;
        let removed_set = removed_files.into_iter().collect::<HashSet<_>>();
        files.retain(|f| !removed_set.contains(&f.path));
        for f in new_files {
            files.push(f);
        }

        // Choose resulting values (preserve old ones when None)
        let resulting_component_type = component_type.unwrap_or(old_metadata.component_type);
        let resulting_dynamic_linking = dynamic_linking.unwrap_or(old_metadata.dynamic_linking);
        let resulting_env = env.unwrap_or(old_metadata.env);
        let resulting_path = local_path.unwrap_or(old_metadata.target_path.as_path());

        // Create and store metadata for the new version (this will insert into registry)
        let dto = self
            .create_and_store_metadata(
                resulting_path,
                &old_metadata.component_name,
                component_id,
                new_version,
                resulting_component_type,
                files,
                false, // do not skip analysis on updates by default (matches previous behavior)
                resulting_dynamic_linking,
                resulting_env,
                old_metadata.environment_id,
                old_metadata.application_id,
                old_metadata.account_id,
                old_metadata.environment_roles_from_shares,
            )
            .await?;

        Ok(dto)
    }

    async fn analyze_memories_and_exports( path: &Path, ) -> anyhow::Result<(RawComponentMetadata, Vec<LinearMemory>, Vec<AnalysedExport>)> { let component_bytes = &tokio::fs::read(path).await?; let raw_component_metadata = RawComponentMetadata::analyse_component(component_bytes)?; let exports = raw_component_metadata.exports.to_vec(); let linear_memories: Vec<LinearMemory> = raw_component_metadata.memories.clone(); Ok((raw_component_metadata, linear_memories, exports)) }
}

async fn load_component_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read wasm bytes from path: {path:?}"))?;
    Ok(bytes)
}

fn analyze_component_bytes(content: &[u8]) -> anyhow::Result<(RawComponentMetadata, Vec<LinearMemory>, Vec<AnalysedExport>)> {
    let raw = RawComponentMetadata::analyse_component(content)?;
    let exports = raw.exports.to_vec();
    let memories = raw.memories.clone();
    Ok((raw, memories, exports))
}

#[async_trait]
impl ComponentService for InMemoryComponentService {
    async fn get(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: ComponentRevision,
    ) -> Result<(Component, ComponentDto), WorkerExecutorError> {
        let key = CacheKey {
            component_id: component_id.clone(),
            component_version,
        };

        let metadata = {
            let reg = self.registry.read().await;
            reg.metadata.get(&key).cloned()
        }
        .ok_or(WorkerExecutorError::unknown(format!(
            "No such component found: {component_id}/{component_version}"
        )))?;

        // use the original source path (we do not copy)
        let wasm_path = metadata.target_path.clone();

        let component = self
            .load_or_compile_component(&wasm_path, engine, &metadata.environment_id, component_id, component_version)
            .await?;

        Ok((component, metadata.into()))
    }

    async fn get_metadata(
        &self,
        component_id: &ComponentId,
        forced_version: Option<ComponentRevision>,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        match forced_version {
            Some(v) => self.metadata_for_version(component_id, v).await,
            None => self.latest_metadata(component_id).await,
        }
    }

    async fn get_caller_specific_latest_metadata(
        &self,
        component_id: &ComponentId,
        _auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        self.latest_metadata(component_id).await
    }

    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        Ok(self
            .registry
            .read()
            .await
            .id_by_name
            .get(&(resolving_environment, component_reference))
            .cloned())
    }

    async fn all_cached_metadata(&self) -> Vec<golem_common::model::component::ComponentDto> {
        self.registry
            .read()
            .await
            .metadata
            .values()
            .map(|local_metadata| golem_common::model::component::ComponentDto::from(local_metadata.clone()))
            .collect()
    }
}

/// Internal registry that holds all metadata in memory.
struct ComponentRegistry {
    metadata: HashMap<CacheKey, FullComponentMetadata>,
    latest_versions: HashMap<ComponentId, ComponentRevision>,
    id_by_name: HashMap<(EnvironmentId, String), ComponentId>,
}

impl ComponentRegistry {
    fn new() -> Self {
        Self {
            metadata: HashMap::new(),
            latest_versions: HashMap::new(),
            id_by_name: HashMap::new(),
        }
    }

    fn insert(&mut self, metadata: FullComponentMetadata) {
        let key = CacheKey {
            component_id: metadata.component_id.clone(),
            component_version: metadata.version,
        };

        self.latest_versions
            .entry(metadata.component_id.clone())
            .and_modify(|e| *e = (*e).max(metadata.version))
            .or_insert(metadata.version);

        self.id_by_name
            .entry((metadata.environment_id.clone(), metadata.component_name.clone()))
            .or_insert(metadata.component_id.clone());

        self.metadata.insert(key, metadata);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    component_id: ComponentId,
    component_version: ComponentRevision,
}

#[derive(Debug, Clone)]
struct FullComponentMetadata {
    pub component_id: ComponentId,
    pub version: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub application_id: ApplicationId,
    pub account_id: AccountId,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub component_name: String,
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
    pub env: BTreeMap<String, String>,
    pub wasm_hash: golem_common::model::diff::Hash,
    pub agent_types: Vec<AgentType>,
    pub environment_roles_from_shares: HashSet<EnvironmentRole>,
    pub target_path: PathBuf,

    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl From<FullComponentMetadata> for ComponentDto {
    fn from(value: FullComponentMetadata) -> Self {
        Self {
            id: value.component_id,
            revision: value.version,
            environment_id: value.environment_id,
            application_id: value.application_id,
            account_id: value.account_id,
            component_name: ComponentName(value.component_name),
            component_size: value.size,
            metadata: ComponentMetadata::from_parts(
                value.exports,
                value.memories,
                value.dynamic_linking,
                value.root_package_name,
                value.root_package_version,
                value.agent_types,
            ),
            created_at: Default::default(),
            component_type: value.component_type,
            files: value.files,
            installed_plugins: vec![],
            env: value.env,
            wasm_hash: value.wasm_hash,
            environment_roles_from_shares: value.environment_roles_from_shares,
        }
    }
}
