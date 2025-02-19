// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::components::component_service::{
    AddComponentError, ComponentService, ComponentServiceClient, PluginServiceClient,
};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::{Component, ComponentMetadata, VersionedComponentId};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::{
    component_metadata::{LinearMemory, RawComponentMetadata},
    ComponentId, ComponentType, ComponentVersion, InitialComponentFile,
};
use golem_wasm_ast::analysis::AnalysedExport;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info};
use uuid::Uuid;

pub struct FileSystemComponentService {
    root: PathBuf,
}

impl FileSystemComponentService {
    pub fn new(root: &Path) -> Self {
        info!("Using a directory for storing components: {root:?}");
        Self {
            root: root.to_path_buf(),
        }
    }

    async fn write_component_to_filesystem(
        &self,
        source_path: &Path,
        component_name: &str,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        skip_analysis: bool,
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
    ) -> Result<Component, AddComponentError> {
        let target_dir = &self.root;
        debug!("Local component store: {target_dir:?}");
        if !target_dir.exists() {
            tokio::fs::create_dir_all(target_dir).await.map_err(|err| {
                AddComponentError::Other(format!(
                    "Failed to create component store directory: {err}"
                ))
            })?;
        }

        if !source_path.exists() {
            return Err(AddComponentError::Other(format!(
                "Source file does not exist: {source_path:?}"
            )));
        }

        let target_path = target_dir.join(format!("{component_id}-{component_version}.wasm"));

        tokio::fs::copy(source_path, &target_path)
            .await
            .map_err(|err| {
                AddComponentError::Other(format!(
                    "Failed to copy WASM to the local component store: {err}"
                ))
            })?;

        let (memories, exports) = if skip_analysis {
            (vec![], vec![])
        } else {
            Self::analyze_memories_and_exports(&target_path)
                .await
                .ok_or(AddComponentError::Other(
                    "Failed to analyze component".to_string(),
                ))?
        };

        let size = tokio::fs::metadata(&target_path)
            .await
            .map_err(|e| AddComponentError::Other(format!("Failed to read component size: {}", e)))?
            .len();

        let metadata = FilesystemComponentMetadata {
            version: component_version,
            component_type,
            files: files.iter().map(|(_source, file)| file.clone()).collect(),
            size,
            memories: memories.clone(),
            exports: exports.clone(),
            plugin_installations: vec![],
            dynamic_linking: dynamic_linking.clone(),
        };
        metadata
            .write_to_file(&target_dir.join(format!("{component_id}-{component_version}.json")))
            .await?;

        Ok(Component {
            versioned_component_id: Some(VersionedComponentId {
                component_id: Some(golem_api_grpc::proto::golem::component::ComponentId {
                    value: Some(component_id.0.into()),
                }),
                version: component_version,
            }),
            component_name: component_name.into(),
            component_size: size,
            metadata: Some(ComponentMetadata {
                exports: exports.into_iter().map(|export| export.into()).collect(),
                producers: vec![],
                memories: memories.into_iter().map(|mem| mem.into()).collect(),
                dynamic_linking: dynamic_linking
                    .iter()
                    .map(|(link, instance)| (link.clone(), instance.clone().into()))
                    .collect(),
            }),
            project_id: None,
            created_at: Some(SystemTime::now().into()),
            component_type: Some(component_type as i32),
            files: files
                .iter()
                .map(|(_source, file)| file.clone().into())
                .collect(),
            installed_plugins: vec![],
        })
    }

    async fn analyze_memories_and_exports(
        path: &Path,
    ) -> Option<(Vec<LinearMemory>, Vec<AnalysedExport>)> {
        let component_bytes = &tokio::fs::read(path).await.ok()?;
        let raw_component_metadata =
            RawComponentMetadata::analyse_component(component_bytes).ok()?;

        let exports = raw_component_metadata
            .exports
            .into_iter()
            .collect::<Vec<_>>();

        let linear_memories: Vec<LinearMemory> = raw_component_metadata
            .memories
            .into_iter()
            .map(|mem| LinearMemory {
                initial: mem.mem_type.limits.min * 65536,
                maximum: mem.mem_type.limits.max.map(|m| m * 65536),
            })
            .collect::<Vec<_>>();

        Some((linear_memories, exports))
    }
}

#[async_trait]
impl ComponentService for FileSystemComponentService {
    fn client_protocol(&self) -> GolemClientProtocol {
        panic!("No real component service running")
    }

    fn handles_ifs_upload(&self) -> bool {
        false
    }

    fn component_client(&self) -> ComponentServiceClient {
        panic!("No real component service running")
    }

    fn plugin_client(&self) -> PluginServiceClient {
        panic!("No real component service running")
    }

    async fn get_or_add_component(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
    ) -> Component {
        self.add_component(
            local_path,
            name,
            component_type,
            files,
            dynamic_linking,
            unverified,
        )
        .await
        .expect("Failed to add component")
    }

    async fn add_component(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
    ) -> Result<Component, AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            name,
            &ComponentId(Uuid::new_v4()),
            0,
            component_type,
            files,
            unverified,
            dynamic_linking,
        )
        .await
    }

    async fn add_component_with_id(
        &self,
        local_path: &Path,
        component_id: &ComponentId,
        component_type: ComponentType,
    ) -> Result<(), AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            &Uuid::new_v4().to_string(),
            component_id,
            0,
            component_type,
            &[],
            false,
            &HashMap::new(),
        )
        .await?;
        Ok(())
    }

    async fn update_component(
        &self,
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
        dynamic_linking: Option<&HashMap<String, DynamicLinkedInstance>>,
    ) -> u64 {
        let target_dir = &self.root;

        debug!("Local component store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir)
                .expect("Failed to create component store directory");
        }

        if !local_path.exists() {
            std::panic!("Source file does not exist: {local_path:?}");
        }

        let last_version = self.get_latest_version(component_id).await;
        let new_version = last_version + 1;

        let empty_linking = HashMap::<String, DynamicLinkedInstance>::new();
        self.write_component_to_filesystem(
            local_path,
            &Uuid::new_v4().to_string(),
            component_id,
            new_version,
            component_type,
            files.unwrap_or_default(),
            false,
            dynamic_linking.unwrap_or(&empty_linking),
        )
        .await
        .expect("Failed to write component to filesystem");
        new_version
    }

    async fn get_latest_version(&self, component_id: &ComponentId) -> u64 {
        let target_dir = &self.root;

        let component_id_str = component_id.to_string();
        let mut versions = std::fs::read_dir(target_dir)
            .expect("Failed to read component store directory")
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path();
                let file_name = path.file_name().unwrap().to_str().unwrap();

                if file_name.starts_with(&component_id_str) && file_name.ends_with(".wasm") {
                    let version_part = file_name.split('-').last().unwrap();
                    let version_part = version_part[..version_part.len() - 5].to_string();
                    version_part.parse::<u64>().ok()
                } else {
                    None
                }
            })
            .collect::<Vec<u64>>();
        versions.sort();
        *versions.last().unwrap_or(&0)
    }

    async fn get_component_size(
        &self,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> crate::Result<Option<u64>> {
        let target_dir = &self.root;
        let path = target_dir.join(format!("{component_id}-{component_version}.wasm"));
        let metadata = tokio::fs::metadata(&path).await?;
        Ok(Some(metadata.len()))
    }

    fn component_directory(&self) -> &Path {
        panic!("No real component service running")
    }

    fn private_host(&self) -> String {
        panic!("No real component service running")
    }

    fn private_http_port(&self) -> u16 {
        panic!("No real component service running")
    }

    fn private_grpc_port(&self) -> u16 {
        panic!("No real component service running")
    }

    async fn kill(&self) {}
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemComponentMetadata {
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub plugin_installations: Vec<PluginInstallation>,
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
}

impl FilesystemComponentMetadata {
    async fn write_to_file(&self, path: &Path) -> Result<(), AddComponentError> {
        let json = serde_json::to_string(self).map_err(|_| {
            AddComponentError::Other("Failed to serialize component file properties".to_string())
        })?;
        tokio::fs::write(path, json).await.map_err(|_| {
            AddComponentError::Other("Failed to write component file properties".to_string())
        })
    }
}
