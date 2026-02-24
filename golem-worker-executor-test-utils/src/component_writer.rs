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

use anyhow::{anyhow, Context};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_agent_types;
use golem_common::model::agent::AgentType;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentId, ComponentName, ComponentRevision,
    InitialComponentFile,
};
use golem_common::model::component_metadata::{
    ComponentMetadata, LinearMemory, RawComponentMetadata,
};
use golem_common::model::diff::{Hash, Hashable};
use golem_common::model::environment::EnvironmentId;
use golem_wasm::analysis::AnalysedExport;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::{debug, info};
use uuid::Uuid;

const WASMS_DIRNAME: &str = "wasms";

#[derive(Clone)]
struct CachedAnalysis {
    memories: Vec<LinearMemory>,
    exports: Vec<AnalysedExport>,
    agent_types: Vec<AgentType>,
    root_package_name: Option<String>,
    root_package_version: Option<String>,
}

pub struct FileSystemComponentWriter {
    root: PathBuf,
    analysis_cache: Cache<blake3::Hash, (), CachedAnalysis, String>,
    component_cache: Cache<(ComponentId, ComponentRevision), (), ComponentDto, String>,
    latest_revisions: Mutex<HashMap<ComponentId, ComponentRevision>>,
}

impl FileSystemComponentWriter {
    pub async fn new(root: &Path) -> Self {
        info!("Using a directory for storing components: {root:?}");

        // If we keep metadata around for multiple runs invariants like unique name
        // might be violated.
        // Ignore the error as this will fail if the directory does not exist.
        let _ = tokio::fs::remove_dir_all(root).await;

        Self {
            root: root.to_path_buf(),
            analysis_cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_analysis",
            ),
            component_cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_metadata",
            ),
            latest_revisions: Mutex::new(HashMap::new()),
        }
    }

    async fn write_component_to_filesystem(
        &self,
        source_path: &Path,
        component_name: &str,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
        files: Vec<InitialComponentFile>,
        skip_analysis: bool,
        env: BTreeMap<String, String>,
        config_vars: BTreeMap<String, String>,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
        original_source_hash: Option<blake3::Hash>,
    ) -> anyhow::Result<ComponentDto> {
        let target_dir = &self.root;

        debug!("Local component store: {target_dir:?}");
        {
            let wasm_dir = target_dir.join(WASMS_DIRNAME);
            if !wasm_dir.exists() {
                tokio::fs::create_dir_all(wasm_dir)
                    .await
                    .map_err(|err| anyhow!("Failed to create component store directory: {err}"))?;
            }
        }

        if !source_path.exists() {
            return Err(anyhow!("Source file does not exist: {source_path:?}"));
        }

        let wasm_filename = format!("{WASMS_DIRNAME}/{component_id}-{component_revision}.wasm");
        let target_path = target_dir.join(&wasm_filename);

        let content = tokio::fs::read(source_path).await?;
        let blake3_hash = blake3::hash(&content);
        let analysis_cache_key = original_source_hash.unwrap_or(blake3_hash);
        let wasm_hash = golem_common::model::diff::Hash::from(blake3_hash);

        tokio::fs::copy(source_path, &target_path)
            .await
            .map_err(|err| anyhow!("Failed to copy WASM to the local component store: {err:#}"))?;

        let CachedAnalysis {
            memories,
            exports,
            agent_types,
            root_package_name,
            root_package_version,
        } = if skip_analysis {
            CachedAnalysis {
                memories: vec![],
                exports: vec![],
                agent_types: vec![],
                root_package_name: None,
                root_package_version: None,
            }
        } else {
            let target_path_clone = target_path.clone();
            self.analysis_cache
                .get_or_insert_simple(&analysis_cache_key, async || {
                    debug!("Analyzing component {component_id} (hash {blake3_hash})");

                    let (raw_component_metadata, memories, exports) =
                        Self::analyze_memories_and_exports(&target_path_clone)
                            .await
                            .map_err(|err| format!("Failed to analyze component: {err:#}"))?;

                    let agent_types = extract_agent_types(&target_path_clone, false, true)
                        .await
                        .map_err(|err| format!("Failed analyzing component: {err}"))?;

                    Ok(CachedAnalysis {
                        memories,
                        exports,
                        agent_types,
                        root_package_name: raw_component_metadata.root_package_name,
                        root_package_version: raw_component_metadata.root_package_version,
                    })
                })
                .await
                .map_err(|err| anyhow!("{err}"))?
        };

        let size = tokio::fs::metadata(&target_path)
            .await
            .map_err(|err| anyhow!("Failed to read component size: {err:#}"))?
            .len();

        let metadata = LocalFileSystemComponentMetadata {
            account_id,
            environment_id,
            application_id,
            component_id: *component_id,
            component_name: component_name.to_string(),
            revision: component_revision,
            files,
            size,
            memories,
            exports,
            wasm_filename,
            env,
            config_vars,
            agent_types,
            target_path,
            root_package_name,
            root_package_version,
            wasm_hash,
            environment_roles_from_shares,
            final_hash: Hash::empty(),
        }
        .with_updated_hash();

        write_metadata_to_file(
            &metadata,
            &target_dir.join(metadata_filename(component_id, component_revision)),
        )
        .await?;

        self.latest_revisions
            .lock()
            .unwrap()
            .entry(*component_id)
            .and_modify(|rev| {
                if component_revision > *rev {
                    *rev = component_revision;
                }
            })
            .or_insert(component_revision);

        tracing::info!(
            "Wrote component {} with revision {} to local component service",
            metadata.component_id,
            metadata.revision
        );

        Ok(metadata.into())
    }

    async fn analyze_memories_and_exports(
        path: &Path,
    ) -> anyhow::Result<(RawComponentMetadata, Vec<LinearMemory>, Vec<AnalysedExport>)> {
        let component_bytes = &tokio::fs::read(path).await?;
        let raw_component_metadata = RawComponentMetadata::analyse_component(component_bytes)?;

        let exports = raw_component_metadata.exports.to_vec();

        let linear_memories: Vec<LinearMemory> = raw_component_metadata.memories.clone();
        Ok((raw_component_metadata, linear_memories, exports))
    }

    async fn load_metadata(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
    ) -> anyhow::Result<LocalFileSystemComponentMetadata> {
        load_metadata_from(&self.root, component_id, component_revision).await
    }

    pub async fn get_or_add_component(
        &self,
        local_path: &Path,
        name: &str,
        files: Vec<InitialComponentFile>,
        unverified: bool,
        env: BTreeMap<String, String>,
        config_vars: BTreeMap<String, String>,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
        original_source_hash: Option<blake3::Hash>,
    ) -> ComponentDto {
        self.add_component(
            local_path,
            name,
            files,
            unverified,
            env,
            config_vars,
            environment_id,
            application_id,
            account_id,
            environment_roles_from_shares,
            original_source_hash,
        )
        .await
        .expect("Failed to add component")
    }

    pub async fn add_component(
        &self,
        local_path: &Path,
        name: &str,
        files: Vec<InitialComponentFile>,
        unverified: bool,
        env: BTreeMap<String, String>,
        config_vars: BTreeMap<String, String>,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
        original_source_hash: Option<blake3::Hash>,
    ) -> anyhow::Result<ComponentDto> {
        self.write_component_to_filesystem(
            local_path,
            name,
            &ComponentId(Uuid::new_v4()),
            ComponentRevision::INITIAL,
            files,
            unverified,
            env,
            config_vars,
            environment_id,
            application_id,
            account_id,
            environment_roles_from_shares,
            original_source_hash,
        )
        .await
    }

    pub async fn add_component_with_id(
        &self,
        local_path: &Path,
        component_id: &ComponentId,
        component_name: &str,
        environment_id: EnvironmentId,
        application_id: ApplicationId,
        account_id: AccountId,
        environment_roles_from_shares: HashSet<EnvironmentRole>,
    ) -> anyhow::Result<ComponentDto> {
        self.write_component_to_filesystem(
            local_path,
            component_name,
            component_id,
            ComponentRevision::INITIAL,
            Vec::new(),
            false,
            BTreeMap::new(),
            BTreeMap::new(),
            environment_id,
            application_id,
            account_id,
            environment_roles_from_shares,
            None,
        )
        .await
    }

    pub async fn update_component(
        &self,
        component_id: &ComponentId,
        local_path: Option<&Path>,
        new_files: Vec<InitialComponentFile>,
        removed_files: Vec<ComponentFilePath>,
        env: Option<BTreeMap<String, String>>,
        config_vars: Option<BTreeMap<String, String>>,
        original_source_hash: Option<blake3::Hash>,
    ) -> anyhow::Result<ComponentDto> {
        let target_dir = &self.root;

        debug!("Local component store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir)
                .expect("Failed to create component store directory");
        }

        let last_revision = self.get_latest_revision(component_id).await;

        let new_revision = last_revision.next().unwrap();

        let old_metadata = self
            .load_metadata(component_id, last_revision)
            .await
            .expect("failed to read metadata");

        let files = {
            let mut files = old_metadata.files;
            let removed_files = removed_files.into_iter().collect::<HashSet<_>>();
            files.retain(|f| !removed_files.contains(&f.path));
            for f in new_files {
                files.push(f);
            }
            files
        };

        let component = self
            .write_component_to_filesystem(
                local_path.unwrap_or(old_metadata.target_path.as_path()),
                &old_metadata.component_name,
                component_id,
                new_revision,
                files,
                false,
                env.unwrap_or(old_metadata.env),
                config_vars.unwrap_or(old_metadata.config_vars),
                old_metadata.environment_id,
                old_metadata.application_id,
                old_metadata.account_id,
                old_metadata.environment_roles_from_shares,
                original_source_hash,
            )
            .await
            .expect("Failed to write component to filesystem");

        Ok(component)
    }

    pub async fn get_latest_revision(&self, component_id: &ComponentId) -> ComponentRevision {
        if let Some(rev) = self.latest_revisions.lock().unwrap().get(component_id) {
            return *rev;
        }

        let target_dir = &self.root;

        let component_id_str = component_id.to_string();
        let mut revisions = std::fs::read_dir(target_dir)
            .expect("Failed to read component store directory")
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path();
                let file_name = path.file_name().unwrap().to_str().unwrap();

                if file_name.starts_with(&component_id_str) && file_name.ends_with(".json") {
                    let version_part = file_name.split('-').next_back().unwrap();
                    let version_part = version_part[..version_part.len() - 5].to_string();
                    version_part.parse::<u64>().ok()
                } else {
                    None
                }
            })
            .collect::<Vec<u64>>();
        revisions.sort();
        let rev = ComponentRevision::new(*revisions.last().unwrap_or(&0)).unwrap();
        self.latest_revisions
            .lock()
            .unwrap()
            .insert(*component_id, rev);
        rev
    }

    pub async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto> {
        let revision = self.get_latest_revision(component_id).await;
        self.get_component_metadata_at_revision(component_id, revision)
            .await
    }

    pub async fn get_component_metadata_at_revision(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
    ) -> anyhow::Result<ComponentDto> {
        let key = (*component_id, revision);
        let root = self.root.clone();
        self.component_cache
            .get_or_insert_simple(&key, async || {
                let metadata = load_metadata_from(&root, &key.0, key.1)
                    .await
                    .map_err(|err| format!("Failed to load component metadata: {err:#}"))?;
                let component: ComponentDto = metadata.into();
                Ok(component)
            })
            .await
            .map_err(|err| anyhow!("{err}"))
    }
}

async fn write_metadata_to_file(
    metadata: &LocalFileSystemComponentMetadata,
    path: &Path,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(metadata)
        .map_err(|_| anyhow!("Failed to serialize component file properties".to_string()))?;
    tokio::fs::write(path, json)
        .await
        .map_err(|_| anyhow!("Failed to write component file properties".to_string()))
}

async fn load_metadata_from(
    root: &Path,
    component_id: &ComponentId,
    component_revision: ComponentRevision,
) -> anyhow::Result<LocalFileSystemComponentMetadata> {
    let path = root.join(metadata_filename(component_id, component_revision));

    let content = tokio::fs::read_to_string(path)
        .await
        .context("failed to read old metadata")?;

    let result = serde_json::from_str(&content)?;
    Ok(result)
}

fn metadata_filename(component_id: &ComponentId, component_revision: ComponentRevision) -> String {
    format!("{component_id}-{component_revision}.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LocalFileSystemComponentMetadata {
    pub component_id: ComponentId,
    pub revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub application_id: ApplicationId,
    pub account_id: AccountId,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub files: Vec<InitialComponentFile>,
    pub component_name: String,
    pub wasm_filename: String,
    pub env: BTreeMap<String, String>,
    pub config_vars: BTreeMap<String, String>,
    pub wasm_hash: golem_common::model::diff::Hash,
    pub agent_types: Vec<AgentType>,
    pub environment_roles_from_shares: HashSet<EnvironmentRole>,
    pub target_path: PathBuf,

    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,

    pub final_hash: Hash,
}

impl LocalFileSystemComponentMetadata {
    pub fn with_updated_hash(self) -> Self {
        let diffable = ComponentDto::from(self.clone()).to_diffable();
        Self {
            final_hash: diffable.hash(),
            ..self
        }
    }
}

impl From<LocalFileSystemComponentMetadata> for ComponentDto {
    fn from(value: LocalFileSystemComponentMetadata) -> Self {
        Self {
            id: value.component_id,
            revision: value.revision,
            environment_id: value.environment_id,
            application_id: value.application_id,
            account_id: value.account_id,
            component_name: ComponentName(value.component_name),
            component_size: value.size,
            metadata: ComponentMetadata::from_parts(
                value.exports,
                value.memories,
                value.root_package_name,
                value.root_package_version,
                value.agent_types,
            ),
            created_at: Default::default(),
            original_files: value.files.clone(),
            files: value.files,
            installed_plugins: vec![],
            original_env: value.env.clone(),
            env: value.env,
            original_config_vars: value.config_vars.clone(),
            config_vars: value.config_vars,
            wasm_hash: value.wasm_hash,
            hash: value.final_hash,
        }
    }
}
