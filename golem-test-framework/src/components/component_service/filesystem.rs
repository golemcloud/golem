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

use super::ComponentServiceGrpcClient;
use super::PluginServiceGrpcClient;
use crate::components::component_service::{AddComponentError, ComponentService};
use crate::config::GolemClientProtocol;
use anyhow::Context;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::GetLatestComponentRequest;
use golem_api_grpc::proto::golem::component::{Component, ComponentMetadata, VersionedComponentId};
use golem_common::model::agent::extraction::extract_agent_types;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::{
    component_metadata::{LinearMemory, RawComponentMetadata},
    ComponentId, ComponentType, ComponentVersion, InitialComponentFile,
};
use golem_common::model::{AccountId, ProjectId};
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::testing::LocalFileSystemComponentMetadata;
use golem_wasm::analysis::AnalysedExport;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tonic::transport::Channel;
use tracing::{debug, info};
use uuid::Uuid;

const WASMS_DIRNAME: &str = "wasms";
// const PLACEHOLDER_ACCOUNT: uuid::Uuid = uuid!("91879a4b-6c62-4dd1-91fe-9dcd29ebe178");
// const PLACEHOLDER_PROJECT: uuid::Uuid = uuid!("6dfe5ca7-ab78-46b2-a98d-41098bb29c98");

pub struct FileSystemComponentService {
    root: PathBuf,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    account_id: AccountId,
    default_project_id: ProjectId,
}

impl FileSystemComponentService {
    pub async fn new(
        root: &Path,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
        account_id: AccountId,
        project_id: ProjectId,
    ) -> Self {
        info!("Using a directory for storing components: {root:?}");

        // If we keep metadata around for multiple runs invariants like unique name
        // might be violated.
        // Ignore the error as this will fail if the directory does not exist.
        let _ = tokio::fs::remove_dir_all(root).await;

        Self {
            root: root.to_path_buf(),
            plugin_wasm_files_service,
            account_id,
            default_project_id: project_id,
        }
    }

    async fn write_component_to_filesystem(
        &self,
        source_path: &Path,
        component_name: &str,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        component_type: ComponentType,
        files: &[InitialComponentFile],
        skip_analysis: bool,
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        env: &HashMap<String, String>,
        project_id_override: Option<ProjectId>,
    ) -> Result<Component, AddComponentError> {
        let target_dir = &self.root;

        debug!("Local component store: {target_dir:?}");
        {
            let wasm_dir = target_dir.join(WASMS_DIRNAME);
            if !wasm_dir.exists() {
                tokio::fs::create_dir_all(wasm_dir).await.map_err(|err| {
                    AddComponentError::Other(format!(
                        "Failed to create component store directory: {err}"
                    ))
                })?;
            }
        }

        if !source_path.exists() {
            return Err(AddComponentError::Other(format!(
                "Source file does not exist: {source_path:?}"
            )));
        }

        let wasm_filename = format!("{WASMS_DIRNAME}/{component_id}-{component_version}.wasm");
        let target_path = target_dir.join(&wasm_filename);

        tokio::fs::copy(source_path, &target_path)
            .await
            .map_err(|err| {
                AddComponentError::Other(format!(
                    "Failed to copy WASM to the local component store: {err:#}"
                ))
            })?;

        let (raw_component_metadata, memories, exports) = if skip_analysis {
            (RawComponentMetadata::default(), vec![], vec![])
        } else {
            Self::analyze_memories_and_exports(&target_path)
                .await
                .map_err(|err| {
                    AddComponentError::Other(format!("Failed to analyze component: {err:#}"))
                })?
        };

        let agent_types = if skip_analysis {
            vec![]
        } else {
            extract_agent_types(&target_path, false, true)
                .await
                .map_err(|err| {
                    AddComponentError::Other(format!("Failed analyzing component: {err}"))
                })?
        };

        let size = tokio::fs::metadata(&target_path)
            .await
            .map_err(|err| {
                AddComponentError::Other(format!("Failed to read component size: {err:#}"))
            })?
            .len();

        let project_id = project_id_override.unwrap_or_else(|| self.default_project_id.clone());

        let metadata = LocalFileSystemComponentMetadata {
            account_id: self.account_id.clone(),
            project_id: project_id.clone(),
            component_id: component_id.clone(),
            component_name: component_name.to_string(),
            version: component_version,
            component_type,
            files: files.to_owned(),
            size,
            memories: memories.clone(),
            exports: exports.clone(),
            dynamic_linking: dynamic_linking.clone(),
            wasm_filename,
            env: env.clone(),
            agent_types,
            root_package_name: raw_component_metadata.root_package_name.clone(),
            root_package_version: raw_component_metadata.root_package_version.clone(),
        };
        write_metadata_to_file(
            metadata,
            &target_dir.join(metadata_filename(component_id, component_version)),
        )
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
                binary_wit: raw_component_metadata.binary_wit,
                root_package_name: raw_component_metadata.root_package_name,
                root_package_version: raw_component_metadata.root_package_version,
                agent_types: vec![],
            }),
            account_id: Some(self.account_id.clone().into()),
            project_id: Some(project_id.into()),
            created_at: Some(SystemTime::now().into()),
            component_type: Some(component_type as i32),
            files: files.iter().map(|file| file.clone().into()).collect(),
            installed_plugins: vec![],
            env: env.clone(),
        })
    }

    async fn analyze_memories_and_exports(
        path: &Path,
    ) -> crate::Result<(RawComponentMetadata, Vec<LinearMemory>, Vec<AnalysedExport>)> {
        let component_bytes = &tokio::fs::read(path).await?;
        let raw_component_metadata = RawComponentMetadata::analyse_component(component_bytes)?;

        let exports = raw_component_metadata.exports.to_vec();

        let linear_memories: Vec<LinearMemory> = raw_component_metadata.memories.clone();
        Ok((raw_component_metadata, linear_memories, exports))
    }

    async fn load_metadata(
        &self,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> crate::Result<LocalFileSystemComponentMetadata> {
        let path = self
            .root
            .join(metadata_filename(component_id, component_version));

        let content = tokio::fs::read_to_string(path)
            .await
            .context("failed to read old metadata")?;

        let result = serde_json::from_str(&content)?;
        Ok(result)
    }
}

#[async_trait]
impl ComponentService for FileSystemComponentService {
    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }

    fn client_protocol(&self) -> GolemClientProtocol {
        panic!("No real component service running")
    }

    async fn base_http_client(&self) -> reqwest::Client {
        panic!("No real component service running")
    }

    async fn component_grpc_client(&self) -> ComponentServiceGrpcClient<Channel> {
        panic!("No real component service running")
    }

    async fn plugin_grpc_client(&self) -> PluginServiceGrpcClient<Channel> {
        panic!("No real component service running")
    }

    async fn get_or_add_component(
        &self,
        token: &Uuid,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> Component {
        self.add_component(
            token,
            local_path,
            name,
            component_type,
            files,
            dynamic_linking,
            unverified,
            env,
            project_id,
        )
        .await
        .expect("Failed to add component")
    }

    async fn add_component(
        &self,
        _token: &Uuid,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> Result<Component, AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            name,
            &ComponentId(Uuid::new_v4()),
            0,
            component_type,
            &files
                .iter()
                .map(|(_source, file)| file.clone())
                .collect::<Vec<_>>(),
            unverified,
            dynamic_linking,
            env,
            project_id,
        )
        .await
    }

    async fn add_component_with_id(
        &self,
        local_path: &Path,
        component_id: &ComponentId,
        component_name: &str,
        component_type: ComponentType,
        project_id: Option<ProjectId>,
    ) -> Result<(), AddComponentError> {
        self.write_component_to_filesystem(
            local_path,
            component_name,
            component_id,
            0,
            component_type,
            &[],
            false,
            &HashMap::new(),
            &HashMap::new(),
            project_id,
        )
        .await?;
        Ok(())
    }

    async fn update_component(
        &self,
        token: &Uuid,
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
        dynamic_linking: Option<&HashMap<String, DynamicLinkedInstance>>,
        env: &HashMap<String, String>,
    ) -> crate::Result<u64> {
        let target_dir = &self.root;

        debug!("Local component store: {target_dir:?}");
        if !target_dir.exists() {
            std::fs::create_dir_all(target_dir)
                .expect("Failed to create component store directory");
        }

        if !local_path.exists() {
            std::panic!("Source file does not exist: {local_path:?}");
        }

        let last_version = self.get_latest_version(token, component_id).await;
        let new_version = last_version + 1;

        let old_metadata = self
            .load_metadata(component_id, last_version)
            .await
            .expect("failed to read metadata");

        let files = files.map(|inner| {
            inner
                .iter()
                .map(|(_, file)| file.clone())
                .collect::<Vec<_>>()
        });

        self.write_component_to_filesystem(
            local_path,
            &old_metadata.component_name,
            component_id,
            new_version,
            component_type,
            files.as_ref().unwrap_or(&old_metadata.files),
            false,
            dynamic_linking.unwrap_or(&old_metadata.dynamic_linking),
            env,
            Some(old_metadata.project_id),
        )
        .await
        .expect("Failed to write component to filesystem");

        Ok(new_version)
    }

    async fn get_latest_version(&self, _token: &Uuid, component_id: &ComponentId) -> u64 {
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
        *versions.last().unwrap_or(&0)
    }

    async fn get_latest_component_metadata(
        &self,
        token: &Uuid,
        request: GetLatestComponentRequest,
    ) -> crate::Result<Component> {
        let component_id: ComponentId = request.component_id.unwrap().try_into().unwrap();
        let version = self.get_latest_version(token, &component_id).await;
        let metadata = self.load_metadata(&component_id, version).await?;
        let component: golem_service_base::model::Component = metadata.into();
        Ok(component.into())
    }

    async fn get_component_size(
        &self,
        _token: &Uuid,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> crate::Result<u64> {
        let metadata = self.load_metadata(component_id, component_version).await?;
        Ok(metadata.size)
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

async fn write_metadata_to_file(
    metadata: LocalFileSystemComponentMetadata,
    path: &Path,
) -> Result<(), AddComponentError> {
    let json = serde_json::to_string(&metadata).map_err(|_| {
        AddComponentError::Other("Failed to serialize component file properties".to_string())
    })?;
    tokio::fs::write(path, json).await.map_err(|_| {
        AddComponentError::Other("Failed to write component file properties".to_string())
    })
}

fn metadata_filename(component_id: &ComponentId, component_version: ComponentVersion) -> String {
    format!("{component_id}-{component_version}.json")
}
