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
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_agent_types;
use golem_common::model::agent::AgentType;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentId, ComponentName, ComponentRevision, ComponentType,
    InitialComponentFile,
};
use golem_common::model::component_metadata::{
    ComponentMetadata, DynamicLinkedInstance, LinearMemory, RawComponentMetadata,
};
use golem_common::model::environment::EnvironmentId;
use golem_wasm::analysis::AnalysedExport;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::{debug, info};
use uuid::Uuid;

const WASMS_DIRNAME: &str = "wasms";

pub struct FileSystemComponentWriter {
    root: PathBuf,
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
        }
    }

    async fn write_component_to_filesystem(
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

        let wasm_filename = format!("{WASMS_DIRNAME}/{component_id}-{component_version}.wasm");
        let target_path = target_dir.join(&wasm_filename);

        let wasm_hash = {
            let content = tokio::fs::read(source_path).await?;
            let hash = blake3::hash(&content);
            golem_common::model::diff::Hash::from(hash)
        };

        tokio::fs::copy(source_path, &target_path)
            .await
            .map_err(|err| anyhow!("Failed to copy WASM to the local component store: {err:#}"))?;

        let (raw_component_metadata, memories, exports) = if skip_analysis {
            (RawComponentMetadata::default(), vec![], vec![])
        } else {
            Self::analyze_memories_and_exports(&target_path)
                .await
                .map_err(|err| anyhow!("Failed to analyze component: {err:#}"))?
        };

        let agent_types = if skip_analysis {
            vec![]
        } else {
            extract_agent_types(&target_path, false, true)
                .await
                .map_err(|err| anyhow!("Failed analyzing component: {err}"))?
        };

        let size = tokio::fs::metadata(&target_path)
            .await
            .map_err(|err| anyhow!("Failed to read component size: {err:#}"))?
            .len();

        let metadata = LocalFileSystemComponentMetadata {
            account_id: account_id.clone(),
            environment_id: environment_id.clone(),
            application_id: application_id.clone(),
            component_id: component_id.clone(),
            component_name: component_name.to_string(),
            version: component_version,
            component_type,
            files,
            size,
            memories: memories.clone(),
            exports: exports.clone(),
            dynamic_linking,
            wasm_filename,
            env,
            agent_types,
            target_path,
            root_package_name: raw_component_metadata.root_package_name.clone(),
            root_package_version: raw_component_metadata.root_package_version.clone(),
            wasm_hash,
            environment_roles_from_shares,
        };

        write_metadata_to_file(
            &metadata,
            &target_dir.join(metadata_filename(component_id, component_version)),
        )
        .await?;

        tracing::info!(
            "Wrote component {} with version {} to local component service",
            metadata.component_id,
            metadata.version
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
        component_version: ComponentRevision,
    ) -> anyhow::Result<LocalFileSystemComponentMetadata> {
        let path = self
            .root
            .join(metadata_filename(component_id, component_version));

        let content = tokio::fs::read_to_string(path)
            .await
            .context("failed to read old metadata")?;

        let result = serde_json::from_str(&content)?;
        Ok(result)
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
    ) -> ComponentDto {
        self.add_component(
            local_path,
            name,
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
        .expect("Failed to add component")
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
        self.write_component_to_filesystem(
            local_path,
            name,
            &ComponentId(Uuid::new_v4()),
            ComponentRevision(0),
            component_type,
            files,
            unverified,
            dynamic_linking,
            env,
            environment_id,
            application_id,
            account_id,
            environment_roles_from_shares,
        )
        .await
    }

    // pub async fn add_component_with_id(
    //     &self,
    //     local_path: &Path,
    //     component_id: &ComponentId,
    //     component_name: &str,
    //     component_type: ComponentType,
    //     environment_id: EnvironmentId,
    //     application_id: ApplicationId,
    //     account_id: AccountId,
    //     environment_roles_from_shares: HashSet<EnvironmentRole>,
    // ) -> anyhow::Result<()> {
    //     self.write_component_to_filesystem(
    //         local_path,
    //         component_name,
    //         component_id,
    //         ComponentRevision(0),
    //         component_type,
    //         Vec::new(),
    //         false,
    //         HashMap::new(),
    //         BTreeMap::new(),
    //         environment_id,
    //         application_id,
    //         account_id,
    //         environment_roles_from_shares,
    //     )
    //     .await?;
    //     Ok(())
    // }

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
        let target_dir = &self.root;

        debug!("Local component store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir)
                .expect("Failed to create component store directory");
        }

        let last_version = self.get_latest_version(component_id).await;

        let new_version = last_version.next().unwrap();

        let old_metadata = self
            .load_metadata(component_id, last_version)
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
                new_version,
                component_type.unwrap_or(old_metadata.component_type),
                files,
                false,
                dynamic_linking.unwrap_or(old_metadata.dynamic_linking),
                env.unwrap_or(old_metadata.env),
                old_metadata.environment_id,
                old_metadata.application_id,
                old_metadata.account_id,
                old_metadata.environment_roles_from_shares,
            )
            .await
            .expect("Failed to write component to filesystem");

        Ok(component)
    }

    pub async fn get_latest_version(&self, component_id: &ComponentId) -> ComponentRevision {
        let target_dir = &self.root;

        let component_id_str = component_id.to_string();
        let mut versions = std::fs::read_dir(target_dir)
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
        versions.sort();
        ComponentRevision(*versions.last().unwrap_or(&0))
    }

    pub async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto> {
        let version = self.get_latest_version(component_id).await;
        let metadata = self.load_metadata(component_id, version).await?;
        let component: golem_common::model::component::ComponentDto = metadata.into();
        Ok(component)
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

fn metadata_filename(component_id: &ComponentId, component_version: ComponentRevision) -> String {
    format!("{component_id}-{component_version}.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LocalFileSystemComponentMetadata {
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
    pub wasm_filename: String,
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
    pub env: BTreeMap<String, String>,
    pub wasm_hash: golem_common::model::diff::Hash,
    pub agent_types: Vec<AgentType>,
    pub environment_roles_from_shares: HashSet<EnvironmentRole>,
    pub target_path: PathBuf,

    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl From<LocalFileSystemComponentMetadata> for ComponentDto {
    fn from(value: LocalFileSystemComponentMetadata) -> Self {
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
