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

use crate::components::component_service::{AddComponentError, ComponentService};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient;
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::{
    component_metadata::{LinearMemory, RawComponentMetadata},
    ComponentId, ComponentType, ComponentVersion, InitialComponentFile,
};
use golem_wasm_ast::analysis::AnalysedExport;
use serde::Serialize;
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tonic::transport::Channel;
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
        component_id: &ComponentId,
        component_version: ComponentVersion,
        component_type: ComponentType,
        files: &[InitialComponentFile],
        skip_analysis: bool,
    ) -> Result<ComponentId, AddComponentError> {
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
            .size();

        let metadata = ComponentMetadata {
            version: component_version,
            component_type,
            files: files.to_owned(),
            size,
            memories,
            exports,
            plugin_installations: vec![],
        };
        metadata
            .write_to_file(&target_dir.join(format!("{component_id}-{component_version}.json")))
            .await?;

        Ok(component_id.clone())
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
    async fn client(&self) -> ComponentServiceClient<Channel> {
        panic!("No real component service running")
    }

    async fn plugins_client(&self) -> PluginServiceClient<Channel> {
        panic!("No real component service running")
    }

    async fn get_or_add_component(
        &self,
        local_path: &Path,
        component_type: ComponentType,
    ) -> ComponentId {
        self.add_component(local_path, component_type)
            .await
            .expect("Failed to add component")
    }

    async fn get_or_add_component_unverified(
        &self,
        local_path: &Path,
        component_type: ComponentType,
    ) -> ComponentId {
        self.write_component_to_filesystem(
            local_path,
            &ComponentId(Uuid::new_v4()),
            0,
            component_type,
            &[],
            true,
        )
        .await
        .expect("Failed to add component")
    }

    async fn add_component_with_id(
        &self,
        local_path: &Path,
        component_id: &ComponentId,
        component_type: ComponentType,
    ) -> Result<(), AddComponentError> {
        self.write_component_to_filesystem(local_path, component_id, 0, component_type, &[], false)
            .await?;
        Ok(())
    }

    async fn add_component(
        &self,
        local_path: &Path,
        component_type: ComponentType,
    ) -> Result<ComponentId, AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            &ComponentId(Uuid::new_v4()),
            0,
            component_type,
            &[],
            false,
        )
        .await
    }

    async fn add_component_with_name(
        &self,
        local_path: &Path,
        _name: &str,
        component_type: ComponentType,
    ) -> Result<ComponentId, AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            &ComponentId(Uuid::new_v4()),
            0,
            component_type,
            &[],
            false,
        )
        .await
    }

    async fn add_component_with_files(
        &self,
        local_path: &Path,
        _name: &str,
        component_type: ComponentType,
        files: &[InitialComponentFile],
    ) -> Result<ComponentId, AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            &ComponentId(Uuid::new_v4()),
            0,
            component_type,
            files,
            false,
        )
        .await
    }

    async fn update_component(
        &self,
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
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

        self.write_component_to_filesystem(
            local_path,
            component_id,
            new_version,
            component_type,
            &[],
            false,
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
pub struct ComponentMetadata {
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub plugin_installations: Vec<PluginInstallation>,
}

impl ComponentMetadata {
    async fn write_to_file(&self, path: &Path) -> Result<(), AddComponentError> {
        let json = serde_json::to_string(self).map_err(|_| {
            AddComponentError::Other("Failed to serialize component file properties".to_string())
        })?;
        tokio::fs::write(path, json).await.map_err(|_| {
            AddComponentError::Other("Failed to write component file properties".to_string())
        })
    }
}
