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

use super::plugin::PluginService;
use super::transformer_plugin_caller::TransformerPluginCaller;
use crate::error::ComponentError;
use crate::model::InitialComponentFilesArchiveAndPermissions;
use crate::model::{
    Component, ComponentByNameAndVersion, ComponentConstraints, ConflictReport,
    ConflictingFunction, ParameterTypeConflict, ReturnTypeConflict,
};
use crate::repo::component::ComponentRecord;
use crate::repo::component::{ComponentConstraintsRecord, ComponentRepo};
use crate::service::component_compilation::ComponentCompilationService;
use crate::service::component_object_store::ComponentObjectStore;
use async_trait::async_trait;
use async_zip::tokio::read::seek::ZipFileReader;
use async_zip::ZipEntry;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use golem_common::model::agent::AgentType;
use golem_common::model::component::ComponentOwner;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::component_constraint::FunctionConstraints;
use golem_common::model::component_constraint::FunctionSignature;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::PluginOwner;
use golem_common::model::plugin::{
    AppPluginDefinition, LibraryPluginDefinition, PluginInstallationUpdateWithId,
    PluginTypeSpecificDefinition, PluginUninstallation,
};
use golem_common::model::plugin::{
    PluginInstallation, PluginInstallationAction, PluginInstallationCreation,
    PluginInstallationUpdate,
};
use golem_common::model::InitialComponentFile;
use golem_common::model::ProjectId;
use golem_common::model::{ComponentFilePath, ComponentFilePermissions};
use golem_common::model::{ComponentId, ComponentType, ComponentVersion, PluginInstallationId};
use golem_common::repo::ComponentOwnerRow;
use golem_common::widen_infallible;
use golem_service_base::clients::limit::LimitService;
use golem_service_base::model::ComponentName;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::repo::RepoError;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_wasm_ast::analysis::AnalysedType;
use rib::FunctionDictionary;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;
use std::vec;
use tap::TapFallible;
use tempfile::NamedTempFile;
use tokio::io::BufReader;
use tokio::sync::RwLock;
use tokio_stream::Stream;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::io::ReaderStream;
use tracing::{error, info, info_span};
use tracing_futures::Instrument;
use wac_graph::types::Package;
use wac_graph::{CompositionGraph, EncodeOptions, PlugError};

#[async_trait]
pub trait ComponentService: Debug + Send + Sync {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError>;

    // Files must have been uploaded to the blob store before calling this method
    async fn create_internal(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Vec<InitialComponentFile>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError>;

    // Files must have been uploaded to the blob store before calling this method
    async fn update_internal(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        // None signals that files should be reused from the previous version
        files: Option<Vec<InitialComponentFile>>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError>;

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &ComponentOwner,
    ) -> Result<Vec<u8>, ComponentError>;

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &ComponentOwner,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError>;

    async fn get_file_contents(
        &self,
        component_id: &ComponentId,
        version: ComponentVersion,
        path: &str,
        owner: &ComponentOwner,
    ) -> Result<BoxStream<'static, Result<Bytes, ComponentError>>, ComponentError>;

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError>;

    async fn find_by_names(
        &self,
        component: Vec<ComponentByNameAndVersion>,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError>;

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        owner: &ComponentOwner,
    ) -> Result<Option<ComponentId>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<Component>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<Component>, ComponentError>;

    async fn get(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError>;

    async fn get_owner(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<ComponentOwner>, ComponentError>;

    async fn delete(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<(), ComponentError>;

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints,
    ) -> Result<ComponentConstraints, ComponentError>;

    async fn delete_constraints(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        constraints_to_remove: &[FunctionSignature],
    ) -> Result<ComponentConstraints, ComponentError>;

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<FunctionConstraints>, ComponentError>;

    /// Gets the list of installed plugins for a given component version belonging to `owner`
    async fn get_plugin_installations_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, ComponentError>;

    async fn create_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, ComponentError>;

    async fn update_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), ComponentError>;

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), ComponentError>;

    async fn batch_update_plugin_installations_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        actions: &[PluginInstallationAction],
    ) -> Result<Vec<Option<PluginInstallation>>, ComponentError>;
}

#[derive(Debug)]
pub struct LazyComponentService(RwLock<Option<Box<dyn ComponentService>>>);

impl Default for LazyComponentService {
    fn default() -> Self {
        Self::new()
    }
}

impl LazyComponentService {
    pub fn new() -> Self {
        Self(RwLock::new(None))
    }

    pub async fn set_implementation(&self, value: impl ComponentService + 'static) {
        let _ = self.0.write().await.insert(Box::new(value));
    }
}

#[async_trait]
impl ComponentService for LazyComponentService {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .create(
                component_id,
                component_name,
                component_type,
                data,
                files,
                installed_plugins,
                dynamic_linking,
                owner,
                env,
                agent_types,
            )
            .await
    }

    // Files must have been uploaded to the blob store before calling this method
    async fn create_internal(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Vec<InitialComponentFile>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .create_internal(
                component_id,
                component_name,
                component_type,
                data,
                files,
                installed_plugins,
                dynamic_linking,
                owner,
                env,
                agent_types,
            )
            .await
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .update(
                component_id,
                data,
                component_type,
                files,
                dynamic_linking,
                owner,
                env,
                agent_types,
            )
            .await
    }

    // Files must have been uploaded to the blob store before calling this method
    async fn update_internal(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        // None signals that files should be reused from the previous version
        files: Option<Vec<InitialComponentFile>>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .update_internal(
                component_id,
                data,
                component_type,
                files,
                dynamic_linking,
                owner,
                env,
                agent_types,
            )
            .await
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &ComponentOwner,
    ) -> Result<Vec<u8>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .download(component_id, version, owner)
            .await
    }

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &ComponentOwner,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .download_stream(component_id, version, owner)
            .await
    }

    async fn get_file_contents(
        &self,
        component_id: &ComponentId,
        version: ComponentVersion,
        path: &str,
        owner: &ComponentOwner,
    ) -> Result<BoxStream<'static, Result<Bytes, ComponentError>>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .get_file_contents(component_id, version, path, owner)
            .await
    }

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .find_by_name(component_name, owner)
            .await
    }

    async fn find_by_names(
        &self,
        component_names: Vec<ComponentByNameAndVersion>,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .find_by_names(component_names, owner)
            .await
    }

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        owner: &ComponentOwner,
    ) -> Result<Option<ComponentId>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .find_id_by_name(component_name, owner)
            .await
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<Component>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .get_by_version(component_id, owner)
            .await
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<Component>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .get_latest_version(component_id, owner)
            .await
    }

    async fn get(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref().unwrap().get(component_id, owner).await
    }

    async fn get_owner(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<ComponentOwner>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref().unwrap().get_owner(component_id).await
    }

    async fn delete(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<(), ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref().unwrap().delete(component_id, owner).await
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints,
    ) -> Result<ComponentConstraints, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .create_or_update_constraint(component_constraint)
            .await
    }

    async fn delete_constraints(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        constraints_to_remove: &[FunctionSignature],
    ) -> Result<ComponentConstraints, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .delete_constraints(owner, component_id, constraints_to_remove)
            .await
    }

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<FunctionConstraints>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .get_component_constraint(component_id, owner)
            .await
    }

    /// Gets the list of installed plugins for a given component version belonging to `owner`
    async fn get_plugin_installations_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .get_plugin_installations_for_component(owner, component_id, component_version)
            .await
    }

    async fn create_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .create_plugin_installation_for_component(owner, component_id, installation)
            .await
    }

    async fn update_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .update_plugin_installation_for_component(owner, installation_id, component_id, update)
            .await
    }

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .delete_plugin_installation_for_component(owner, installation_id, component_id)
            .await
    }

    async fn batch_update_plugin_installations_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        actions: &[PluginInstallationAction],
    ) -> Result<Vec<Option<PluginInstallation>>, ComponentError> {
        let lock = self.0.read().await;
        lock.as_ref()
            .unwrap()
            .batch_update_plugin_installations_for_component(owner, component_id, actions)
            .await
    }
}

pub struct ComponentServiceDefault {
    component_repo: Arc<dyn ComponentRepo>,
    object_store: Arc<dyn ComponentObjectStore>,
    component_compilation: Arc<dyn ComponentCompilationService>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_service: Arc<PluginService>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    transformer_plugin_caller: Arc<dyn TransformerPluginCaller>,
    limit_service: Arc<dyn LimitService>,
}

impl ComponentServiceDefault {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<dyn ComponentObjectStore>,
        component_compilation: Arc<dyn ComponentCompilationService>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        plugin_service: Arc<PluginService>,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
        transformer_plugin_caller: Arc<dyn TransformerPluginCaller>,
        limit_service: Arc<dyn LimitService>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            component_compilation,
            initial_component_files_service,
            plugin_service,
            plugin_wasm_files_service,
            transformer_plugin_caller,
            limit_service,
        }
    }

    pub fn find_component_metadata_conflicts(
        function_constraints: &FunctionConstraints,
        new_type_registry: &FunctionDictionary,
    ) -> ConflictReport {
        let mut missing_functions = vec![];
        let mut conflicting_functions = vec![];

        for existing_function_call in &function_constraints.constraints {
            if let Some(new_registry_value) =
                new_type_registry.get(existing_function_call.function_key())
            {
                let mut parameter_conflict = false;
                let mut return_conflict = false;

                if existing_function_call.parameter_types() != &new_registry_value.parameter_types()
                {
                    parameter_conflict = true;
                }

                let new_return_type = new_registry_value
                    .return_type
                    .as_ref()
                    .map(|x| AnalysedType::try_from(x).unwrap());

                // AnalysedType conversion from function `FunctionType` should never fail
                if existing_function_call.return_type() != &new_return_type {
                    return_conflict = true;
                }

                let parameter_conflict = if parameter_conflict {
                    Some(ParameterTypeConflict {
                        existing: existing_function_call.parameter_types().clone(),
                        new: new_registry_value.clone().parameter_types().clone(),
                    })
                } else {
                    None
                };

                let return_conflict = if return_conflict {
                    Some(ReturnTypeConflict {
                        existing: existing_function_call.return_type().clone(),
                        new: new_return_type,
                    })
                } else {
                    None
                };

                if parameter_conflict.is_some() || return_conflict.is_some() {
                    conflicting_functions.push(ConflictingFunction {
                        function: existing_function_call.function_key().clone(),
                        parameter_type_conflict: parameter_conflict,
                        return_type_conflict: return_conflict,
                    });
                }
            } else {
                missing_functions.push(existing_function_call.function_key().clone());
            }
        }

        ConflictReport {
            missing_functions,
            conflicting_functions,
        }
    }

    async fn upload_component_files(
        &self,
        project_id: &ProjectId,
        payload: InitialComponentFilesArchiveAndPermissions,
    ) -> Result<Vec<InitialComponentFile>, ComponentError> {
        let path_permissions: HashMap<ComponentFilePath, ComponentFilePermissions> =
            HashMap::from_iter(
                payload
                    .files
                    .iter()
                    .map(|f| (f.path.clone(), f.permissions)),
            );

        let to_upload = self
            .prepare_component_files_for_upload(&path_permissions, payload)
            .await?;
        let tasks = to_upload
            .into_iter()
            .map(|(path, permissions, stream)| async move {
                info!("Uploading file: {}", path.to_string());

                self.initial_component_files_service
                    .put_if_not_exists(project_id, &stream)
                    .await
                    .map_err(|e| {
                        ComponentError::initial_component_file_upload_error(
                            "Failed to upload component files",
                            e,
                        )
                    })
                    .map(|key| InitialComponentFile {
                        key,
                        path,
                        permissions,
                    })
            });

        let uploaded = futures::future::try_join_all(tasks).await?;

        let uploaded_paths = uploaded
            .iter()
            .map(|f| f.path.clone())
            .collect::<HashSet<_>>();

        for path in path_permissions.keys() {
            if !uploaded_paths.contains(path) {
                return Err(ComponentError::malformed_component_archive_from_message(
                    format!("Didn't find expected file in the archive: {path}"),
                ));
            }
        }

        Ok(uploaded)
    }

    async fn prepare_component_files_for_upload(
        &self,
        path_permissions: &HashMap<ComponentFilePath, ComponentFilePermissions>,
        payload: InitialComponentFilesArchiveAndPermissions,
    ) -> Result<Vec<(ComponentFilePath, ComponentFilePermissions, ZipEntryStream)>, ComponentError>
    {
        let files_file = Arc::new(payload.archive);

        let tokio_file = tokio::fs::File::from_std(files_file.reopen().map_err(|e| {
            ComponentError::initial_component_file_upload_error(
                "Failed to open provided component files",
                e.to_string(),
            )
        })?);

        let mut buf_reader = BufReader::new(tokio_file);

        let mut zip_archive = ZipFileReader::with_tokio(&mut buf_reader)
            .await
            .map_err(|e| {
                ComponentError::malformed_component_archive_from_error(
                    "Failed to unpack provided component files",
                    e.into(),
                )
            })?;

        let mut result = vec![];

        for i in 0..zip_archive.file().entries().len() {
            let entry_reader = zip_archive.reader_with_entry(i).await.map_err(|e| {
                ComponentError::malformed_component_archive_from_error(
                    "Failed to read entry from archive",
                    e.into(),
                )
            })?;

            let entry = entry_reader.entry();

            let is_dir = entry.dir().map_err(|e| {
                ComponentError::malformed_component_archive_from_error(
                    "Failed to check if entry is a directory",
                    e.into(),
                )
            })?;

            if is_dir {
                continue;
            }

            let path = initial_component_file_path_from_zip_entry(entry)?;

            let permissions = path_permissions
                .get(&path)
                .cloned()
                .unwrap_or(ComponentFilePermissions::ReadOnly);

            let stream = ZipEntryStream::from_zip_file_and_index(files_file.clone(), i);

            result.push((path, permissions, stream));
        }

        Ok(result)
    }

    // All files must be confirmed to be in the blob store before calling this method
    async fn create_unchecked(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        uploaded_files: Vec<InitialComponentFile>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let component_size: u64 = data.len() as u64;

        // FIXME: This needs to be reverted in case creation fails.
        self.limit_service
            .update_component_limit(&owner.account_id, component_id, 1, component_size as i64)
            .await?;

        let component = Component::new(
            component_id.clone(),
            component_name.clone(),
            component_type,
            &data,
            uploaded_files,
            installed_plugins,
            dynamic_linking,
            owner.clone(),
            env,
            agent_types,
        )?;

        info!(
            owner = %owner,
            exports = ?component.metadata.exports(),
            dynamic_linking = ?component.metadata.dynamic_linking(),
            "Uploaded component",
        );

        let (component, transformed_data) =
            self.apply_transformations(component, data.clone()).await?;

        if let Some(known_root_package_name) = &component.metadata.root_package_name() {
            if &component_name.0 != known_root_package_name {
                Err(ComponentError::InvalidComponentName {
                    actual: component_name.0.clone(),
                    expected: known_root_package_name.clone(),
                })?;
            }
        }

        tokio::try_join!(
            self.upload_user_component(&component, data),
            self.upload_protected_component(&component, transformed_data)
        )?;

        let record = ComponentRecord::try_from_model(component.clone())
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let result = self.component_repo.create(&record).await;
        match result {
            Err(RepoError::UniqueViolation(_)) => {
                Err(ComponentError::AlreadyExists(component_id.clone()))?
            }
            Err(other) => Err(other)?,
            Ok(()) => {}
        };

        self.component_compilation
            .enqueue_compilation(
                &owner.project_id,
                component_id,
                component.versioned_component_id.version,
            )
            .await;

        Ok(component)
    }

    async fn update_unchecked(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<Vec<InitialComponentFile>>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let component_size: u64 = data.len() as u64;

        // FIXME: This needs to be reverted in case update fails.
        self.limit_service
            .update_component_limit(&owner.account_id, component_id, 0, component_size as i64)
            .await?;

        let metadata = ComponentMetadata::analyse_component(&data, dynamic_linking, agent_types)
            .map_err(ComponentError::ComponentProcessingError)?;

        let constraints = self
            .component_repo
            .get_constraint(&owner.to_string(), component_id.0)
            .await?;

        let new_type_registry = FunctionDictionary::from_exports(metadata.exports())
            .map_err(|e| ComponentError::conversion_error("exports", e))?;

        if let Some(constraints) = constraints {
            let conflicts =
                Self::find_component_metadata_conflicts(&constraints, &new_type_registry);
            if !conflicts.is_empty() {
                return Err(ComponentError::ComponentConstraintConflictError(conflicts));
            }
        }

        info!(
            owner = %owner,
            exports = ?metadata.exports(),
            dynamic_linking = ?metadata.dynamic_linking(),
            "Uploaded component",
        );

        let mut component: Component = self
            .get_latest_version(component_id, owner)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        component.bump_version();
        // make sure we are storing data under new keys so we don't clobber old data.
        component.regenerate_object_store_key();
        component.regenerate_transformed_object_store_key();
        component.files = files.unwrap_or(component.files);
        component.metadata = metadata;
        component.env = env;
        component.component_type = component_type.unwrap_or(component.component_type);

        // reset transformations so that plugins see original data of the component
        component.reset_transformations();
        let (component, transformed_data) =
            self.apply_transformations(component, data.clone()).await?;

        tokio::try_join!(
            self.upload_user_component(&component, data),
            self.upload_protected_component(&component, transformed_data)
        )?;

        let record = ComponentRecord::try_from_model(component.clone())
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let result = self.component_repo.create(&record).await;

        match result {
            Err(RepoError::UniqueViolation(_)) => Err(ComponentError::ConcurrentUpdate {
                component_id: component_id.clone(),
                version: component.versioned_component_id.version,
            })?,
            Err(other) => Err(other)?,
            Ok(()) => {}
        };

        self.component_compilation
            .enqueue_compilation(
                &owner.project_id,
                component_id,
                component.versioned_component_id.version,
            )
            .await;

        Ok(component)
    }

    async fn upload_user_component(
        &self,
        component: &Component,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(
                &component.owner.project_id,
                &component.user_object_store_key(),
                data,
            )
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload user component", e)
            })
    }

    async fn upload_protected_component(
        &self,
        component: &Component,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(
                &component.owner.project_id,
                &component.transformed_object_store_key(),
                data,
            )
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload protected component", e)
            })
    }

    async fn apply_transformations(
        &self,
        mut component: Component,
        mut data: Vec<u8>,
    ) -> Result<(Component, Vec<u8>), ComponentError> {
        if !component.installed_plugins.is_empty() {
            let mut installed_plugins = component.installed_plugins.clone();
            installed_plugins.sort_by_key(|p| p.priority);

            for installation in installed_plugins {
                let plugin = self
                    .plugin_service
                    .get_by_id(&installation.plugin_id)
                    .await?
                    .expect("Failed to resolve plugin by id");

                match plugin.specs {
                    PluginTypeSpecificDefinition::ComponentTransformer(spec) => {
                        let span = info_span!("component transformation",
                            owner = %component.owner,
                            component_id = %component.versioned_component_id,
                            plugin_id = %installation.plugin_id,
                            plugin_installation_id = %installation.id,
                        );

                        (component, data) = self
                            .apply_component_transformer_plugin(
                                component,
                                data,
                                spec.transform_url,
                                &installation.parameters,
                            )
                            .instrument(span)
                            .await?;
                    }
                    PluginTypeSpecificDefinition::Library(spec) => {
                        let span = info_span!("library plugin",
                            owner = %component.owner,
                            component_id = %component.versioned_component_id,
                            plugin_id = %installation.plugin_id,
                            plugin_installation_id = %installation.id,
                        );
                        data = self
                            .apply_library_plugin(&component, &data, spec)
                            .instrument(span)
                            .await?;
                    }
                    PluginTypeSpecificDefinition::App(spec) => {
                        let span = info_span!("app plugin",
                            owner = %component.owner,
                            component_id = %component.versioned_component_id,
                            plugin_id = %installation.plugin_id,
                            plugin_installation_id = %installation.id,
                        );
                        data = self
                            .apply_app_plugin(&component, &data, spec)
                            .instrument(span)
                            .await?;
                    }
                    PluginTypeSpecificDefinition::OplogProcessor(_) => (),
                }
            }
        }

        component.metadata = ComponentMetadata::analyse_component(
            &data,
            component.metadata.dynamic_linking().clone(),
            component.metadata.agent_types().to_vec(),
        )
        .map_err(ComponentError::ComponentProcessingError)?;

        Ok((component, data))
    }

    async fn apply_component_transformer_plugin(
        &self,
        mut component: Component,
        data: Vec<u8>,
        url: String,
        parameters: &HashMap<String, String>,
    ) -> Result<(Component, Vec<u8>), ComponentError> {
        info!(%url, "Applying component transformation plugin");

        let response = self
            .transformer_plugin_caller
            .call_remote_transformer_plugin(&component, &data, url, parameters)
            .await
            .map_err(ComponentError::TransformationFailed)?;

        let data = response.data.map(|b64| b64.0).unwrap_or(data);

        for (k, v) in response.env.unwrap_or_default() {
            component.transformed_env.insert(k, v);
        }

        let mut files = component.transformed_files;
        for file in response.additional_files.unwrap_or_default() {
            let content_stream = Bytes::from(file.content.0)
                .map_item(|i| i.map_err(widen_infallible::<String>))
                .map_error(widen_infallible::<String>);

            let key = self
                .initial_component_files_service
                .put_if_not_exists(&component.owner.project_id, content_stream)
                .await
                .map_err(|e| {
                    ComponentError::initial_component_file_upload_error(
                        "Failed to upload component files",
                        e,
                    )
                })?;

            let item = InitialComponentFile {
                key,
                path: file.path,
                permissions: file.permissions,
            };

            files.retain_mut(|f| f.path != item.path);
            files.push(item)
        }
        component.transformed_files = files;

        Ok((component, data))
    }

    async fn apply_library_plugin(
        &self,
        component: &Component,
        data: &[u8],
        plugin_spec: LibraryPluginDefinition,
    ) -> Result<Vec<u8>, ComponentError> {
        info!(%component.versioned_component_id, "Applying library plugin");

        let plug_bytes = self
            .plugin_wasm_files_service
            .get(&component.owner.account_id, &plugin_spec.blob_storage_key)
            .await
            .map_err(|e| {
                ComponentError::PluginApplicationFailed(format!("Failed to fetch plugin wasm: {e}"))
            })?
            .ok_or(ComponentError::PluginApplicationFailed(
                "Plugin data not found".to_string(),
            ))?;

        let composed = compose_components(data, &plug_bytes).map_err(|e| {
            ComponentError::PluginApplicationFailed(format!(
                "Failed to compose plugin with component: {e}"
            ))
        })?;

        Ok(composed)
    }

    async fn apply_app_plugin(
        &self,
        component: &Component,
        data: &[u8],
        plugin_spec: AppPluginDefinition,
    ) -> Result<Vec<u8>, ComponentError> {
        info!(%component.versioned_component_id, "Applying app plugin");

        let socket_bytes = self
            .plugin_wasm_files_service
            .get(&component.owner.account_id, &plugin_spec.blob_storage_key)
            .await
            .map_err(|e| {
                ComponentError::PluginApplicationFailed(format!("Failed to fetch plugin wasm: {e}"))
            })?
            .ok_or(ComponentError::PluginApplicationFailed(
                "Plugin data not found".to_string(),
            ))?;

        let composed = compose_components(&socket_bytes, data).map_err(|e| {
            ComponentError::PluginApplicationFailed(format!(
                "Failed to compose plugin with component: {e}"
            ))
        })?;

        Ok(composed)
    }
}

impl Debug for ComponentServiceDefault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentServiceDefault").finish()
    }
}

#[async_trait]
impl ComponentService for ComponentServiceDefault {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        info!(owner = %owner, "Create component");

        self.find_id_by_name(component_name, owner)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let uploaded_files = match files {
            Some(files) => {
                self.upload_component_files(&owner.project_id, files)
                    .await?
            }
            None => vec![],
        };

        self.create_unchecked(
            component_id,
            component_name,
            component_type,
            data,
            uploaded_files,
            installed_plugins,
            dynamic_linking,
            owner,
            env,
            agent_types,
        )
        .await
    }

    async fn create_internal(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Vec<InitialComponentFile>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        info!(owner = %owner, "Create component");

        self.find_id_by_name(component_name, owner)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        for file in &files {
            let exists = self
                .initial_component_files_service
                .exists(&owner.project_id, &file.key)
                .await
                .map_err(|e| {
                    ComponentError::initial_component_file_upload_error(
                        "Error checking if file exists",
                        e,
                    )
                })?;

            if !exists {
                return Err(ComponentError::initial_component_file_not_found(
                    &file.path, &file.key,
                ));
            }
        }

        self.create_unchecked(
            component_id,
            component_name,
            component_type,
            data,
            files,
            installed_plugins,
            dynamic_linking,
            owner,
            env,
            agent_types,
        )
        .await
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        info!(owner = %owner, "Update component");

        let uploaded_files = match files {
            Some(files) => Some(
                self.upload_component_files(&owner.project_id, files)
                    .await?,
            ),
            None => None,
        };

        self.update_unchecked(
            component_id,
            data,
            component_type,
            uploaded_files,
            dynamic_linking,
            owner,
            env,
            agent_types,
        )
        .await
    }

    async fn update_internal(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<Vec<InitialComponentFile>>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &ComponentOwner,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        info!(owner = %owner, "Update component");

        for file in files.iter().flatten() {
            let exists = self
                .initial_component_files_service
                .exists(&owner.project_id, &file.key)
                .await
                .map_err(|e| {
                    ComponentError::initial_component_file_upload_error(
                        "Error checking if file exists",
                        e,
                    )
                })?;

            if !exists {
                return Err(ComponentError::initial_component_file_not_found(
                    &file.path, &file.key,
                ));
            }
        }

        self.update_unchecked(
            component_id,
            data,
            component_type,
            files,
            dynamic_linking,
            owner,
            env,
            agent_types,
        )
        .await
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &ComponentOwner,
    ) -> Result<Vec<u8>, ComponentError> {
        let component = match version {
            None => self.get_latest_version(component_id, owner).await?,
            Some(version) => {
                self.get_by_version(
                    &VersionedComponentId {
                        component_id: component_id.clone(),
                        version,
                    },
                    owner,
                )
                .await?
            }
        };

        if let Some(component) = component {
            info!(owner = %owner, component_id = %component.versioned_component_id, "Download component");

            self.object_store
                .get(
                    &component.owner.project_id,
                    &component.transformed_object_store_key(),
                )
                .await
                .tap_err(|e| error!(owner = %owner, "Error downloading component - error: {}", e))
                .map_err(|e| {
                    ComponentError::component_store_error("Error downloading component", e)
                })
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &ComponentOwner,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
        let component = match version {
            None => self.get_latest_version(component_id, owner).await?,
            Some(version) => {
                self.get_by_version(
                    &VersionedComponentId {
                        component_id: component_id.clone(),
                        version,
                    },
                    owner,
                )
                .await?
            }
        };

        if let Some(component) = component {
            let protected_object_store_key = component.transformed_object_store_key();

            info!(
                owner = %owner,
                component_id = %component.versioned_component_id,
                protected_object_store_key = %protected_object_store_key,
                "Download component as stream",
            );

            self.object_store
                .get_stream(&component.owner.project_id, &protected_object_store_key)
                .await
                .map_err(|e| {
                    ComponentError::component_store_error("Error downloading component", e)
                })
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn get_file_contents(
        &self,
        component_id: &ComponentId,
        version: ComponentVersion,
        path: &str,
        owner: &ComponentOwner,
    ) -> Result<BoxStream<'static, Result<Bytes, ComponentError>>, ComponentError> {
        let component = self
            .get_by_version(
                &VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                owner,
            )
            .await?;
        if let Some(component) = component {
            info!(owner = %owner, component_id = %component.versioned_component_id, "Stream component file: {}", path);

            let file = component
                .files
                .iter()
                .find(|&file| file.path.to_rel_string() == path)
                .ok_or(ComponentError::InvalidFilePath(path.to_string()))?;

            let stream = self
                .initial_component_files_service
                .get(&owner.project_id, &file.key)
                .await
                .map_err(|e| {
                    ComponentError::initial_component_file_upload_error(
                        "Failed to get component file",
                        e,
                    )
                })?
                .ok_or(ComponentError::FailedToDownloadFile)?
                .map_err(|e| {
                    ComponentError::initial_component_file_upload_error(
                        "Error streaming file data",
                        e,
                    )
                });

            Ok(Box::pin(stream))
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(owner = %owner, "Find component by name");

        let records = match component_name {
            Some(name) => {
                self.component_repo
                    .get_by_name(&owner.to_string(), &name.0)
                    .await?
            }
            None => self.component_repo.get_all(&owner.to_string()).await?,
        };

        let values: Vec<Component> = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn find_by_names(
        &self,
        component_names: Vec<ComponentByNameAndVersion>,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError> {
        info!("Find components by names");

        let component_records = self
            .component_repo
            .get_by_names(&owner.to_string(), &component_names)
            .await?;

        let values: Vec<Component> = component_records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        owner: &ComponentOwner,
    ) -> Result<Option<ComponentId>, ComponentError> {
        info!(owner = %owner, "Find component id by name");
        let records = self
            .component_repo
            .get_id_by_name(&owner.to_string(), &component_name.0)
            .await?;
        Ok(records.map(ComponentId))
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<Component>, ComponentError> {
        info!(
            owner = %owner,
            component_id = %component_id,
            "Get component by version"
        );

        let result = self
            .component_repo
            .get_by_version(
                &owner.to_string(),
                component_id.component_id.0,
                component_id.version,
            )
            .await?;

        match result {
            Some(c) => {
                let value = c
                    .try_into()
                    .map_err(|e| ComponentError::conversion_error("record", e))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<Component>, ComponentError> {
        info!(owner = %owner, "Get latest component");
        let result = self
            .component_repo
            .get_latest_version(&owner.to_string(), component_id.0)
            .await?;

        match result {
            Some(c) => {
                let value = c
                    .try_into()
                    .map_err(|e| ComponentError::conversion_error("record", e))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn get(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(owner = %owner, component_id = %component_id ,"Get component");
        let records = self
            .component_repo
            .get(&owner.to_string(), component_id.0)
            .await?;

        let values: Vec<Component> = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn get_owner(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<ComponentOwner>, ComponentError> {
        info!(component_id = %component_id, "Get component owner");
        let result = self.component_repo.get_namespace(component_id.0).await?;
        if let Some(result) = result {
            let value = result
                .parse()
                .map_err(|e| ComponentError::conversion_error("namespace", e))?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    async fn delete(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<(), ComponentError> {
        info!(owner = %owner, component_id = %component_id, "Delete component");

        let records = self
            .component_repo
            .get(&owner.to_string(), component_id.0)
            .await?;
        let components = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let mut object_store_keys = Vec::new();

        for component in components {
            object_store_keys.push((
                component.owner.project_id.clone(),
                component.transformed_object_store_key(),
            ));
            object_store_keys.push((
                component.owner.project_id.clone(),
                component.user_object_store_key(),
            ));
        }

        if !object_store_keys.is_empty() {
            for (project_id, key) in object_store_keys {
                self.object_store
                    .delete(&project_id, &key)
                    .await
                    .map_err(|e| {
                        ComponentError::component_store_error("Failed to delete component data", e)
                    })?;
            }
            self.component_repo
                .delete(&owner.to_string(), component_id.0)
                .await?;
            Ok(())
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints,
    ) -> Result<ComponentConstraints, ComponentError> {
        info!(owner = %component_constraint.owner, component_id = %component_constraint.component_id, "Create or update component constraint");
        let component_id = &component_constraint.component_id;
        let record = ComponentConstraintsRecord::try_from(component_constraint.clone())
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        self.component_repo
            .create_or_update_constraint(&record)
            .await?;
        let result = self
            .component_repo
            .get_constraint(
                &component_constraint.owner.to_string(),
                component_constraint.component_id.0,
            )
            .await?
            .ok_or(ComponentError::ComponentConstraintCreateError(format!(
                "Failed to create constraints for {component_id}"
            )))?;

        let component_constraints = ComponentConstraints {
            owner: component_constraint.owner.clone(),
            component_id: component_id.clone(),
            constraints: result,
        };

        Ok(component_constraints)
    }

    async fn delete_constraints(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        constraints_to_remove: &[FunctionSignature],
    ) -> Result<ComponentConstraints, ComponentError> {
        info!(owner = %owner, component_id = %component_id, "Delete constraint");

        self.component_repo
            .delete_constraints(&owner.to_string(), component_id.0, constraints_to_remove)
            .await?;

        let result = self
            .component_repo
            .get_constraint(&owner.to_string(), component_id.0)
            .await?
            .ok_or(ComponentError::ComponentConstraintCreateError(format!(
                "Failed to get constraints for {component_id}"
            )))?;

        let component_constraints = ComponentConstraints {
            owner: owner.clone(),
            component_id: component_id.clone(),
            constraints: result,
        };

        Ok(component_constraints)
    }

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
        owner: &ComponentOwner,
    ) -> Result<Option<FunctionConstraints>, ComponentError> {
        info!(component_id = %component_id, "Get component constraint");

        let result = self
            .component_repo
            .get_constraint(&owner.to_string(), component_id.0)
            .await?;
        Ok(result)
    }

    async fn get_plugin_installations_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, ComponentError> {
        let owner_record: ComponentOwnerRow = owner.clone().into();
        let plugin_owner_record = owner_record.into();
        let records = self
            .component_repo
            .get_installed_plugins(&plugin_owner_record, component_id.0, component_version)
            .await?;

        records
            .into_iter()
            .map(PluginInstallation::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ComponentError::conversion_error("plugin installation", e))
    }

    async fn create_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, ComponentError> {
        let result = self
            .batch_update_plugin_installations_for_component(
                owner,
                component_id,
                &[PluginInstallationAction::Install(installation)],
            )
            .await?;
        Ok(result.into_iter().next().unwrap().unwrap())
    }

    async fn update_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), ComponentError> {
        let _ = self
            .batch_update_plugin_installations_for_component(
                owner,
                component_id,
                &[PluginInstallationAction::Update(
                    PluginInstallationUpdateWithId {
                        installation_id: installation_id.clone(),
                        priority: update.priority,
                        parameters: update.parameters,
                    },
                )],
            )
            .await?;
        Ok(())
    }

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &ComponentOwner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), ComponentError> {
        let _ = self
            .batch_update_plugin_installations_for_component(
                owner,
                component_id,
                &[PluginInstallationAction::Uninstall(PluginUninstallation {
                    installation_id: installation_id.clone(),
                })],
            )
            .await;
        Ok(())
    }

    async fn batch_update_plugin_installations_for_component(
        &self,
        owner: &ComponentOwner,
        component_id: &ComponentId,
        actions: &[PluginInstallationAction],
    ) -> Result<Vec<Option<PluginInstallation>>, ComponentError> {
        let mut component: Component = self
            .get_latest_version(component_id, owner)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        component.bump_version();
        // reuse the untransformed object store key from the previous version
        component.regenerate_transformed_object_store_key();
        component.reset_transformations();

        let mut result = Vec::new();

        for action in actions {
            match action {
                PluginInstallationAction::Install(installation) => {
                    let plugin_definition = self
                        .plugin_service
                        .get(
                            &PluginOwner::from(owner.clone()),
                            &installation.name,
                            &installation.version,
                        )
                        .await?
                        .ok_or(ComponentError::PluginNotFound {
                            account_id: owner.account_id.clone(),
                            plugin_name: installation.name.clone(),
                            plugin_version: installation.version.clone(),
                        })?;

                    {
                        let installation_allowed = plugin_definition
                            .scope
                            .valid_in_component(component_id, &owner.project_id);

                        if !installation_allowed {
                            Err(ComponentError::InvalidPluginScope {
                                plugin_name: installation.name.clone(),
                                plugin_version: installation.version.clone(),
                                details: format!("not available for component {}", component_id.0),
                            })?
                        };
                    }

                    let plugin_installation = PluginInstallation {
                        id: PluginInstallationId::new_v4(),
                        plugin_id: plugin_definition.id,
                        priority: installation.priority,
                        parameters: installation.parameters.clone(),
                    };

                    component
                        .installed_plugins
                        .push(plugin_installation.clone());

                    result.push(Some(plugin_installation));
                }
                PluginInstallationAction::Update(update) => {
                    let existing = component
                        .installed_plugins
                        .iter_mut()
                        .find(|ip| ip.id == update.installation_id);

                    let Some(existing) = existing else {
                        Err(ComponentError::PluginInstallationNotFound {
                            installation_id: update.installation_id.clone(),
                        })?
                    };

                    existing.priority = update.priority;
                    existing.parameters = update.parameters.clone();
                    result.push(None);
                }
                PluginInstallationAction::Uninstall(uninstallation) => {
                    result.push(None);

                    let len_before = component.installed_plugins.len();
                    component
                        .installed_plugins
                        .retain(|ip| ip.id != uninstallation.installation_id);

                    if component.installed_plugins.len() == len_before {
                        // we failed to find the installation
                        Err(ComponentError::PluginInstallationNotFound {
                            installation_id: uninstallation.installation_id.clone(),
                        })?
                    };
                }
            }
        }

        let data = self
            .object_store
            .get(
                &component.owner.project_id,
                &component.user_object_store_key(),
            )
            .await
            .map_err(|err| {
                ComponentError::component_store_error("Failed to download user component", err)
            })?;

        let (component, transformed_data) = self.apply_transformations(component, data).await?;

        self.object_store
            .put(
                &component.owner.project_id,
                &component.transformed_object_store_key(),
                transformed_data,
            )
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload protected component", e)
            })?;

        let record = ComponentRecord::try_from_model(component.clone())
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let create_result = self.component_repo.create(&record).await;
        match create_result {
            Err(RepoError::UniqueViolation(_)) => Err(ComponentError::ConcurrentUpdate {
                component_id: component_id.clone(),
                version: component.versioned_component_id.version,
            })?,
            Err(other) => Err(other)?,
            Ok(()) => {}
        };

        Ok(result)
    }
}

struct ZipEntryStream {
    file: Arc<NamedTempFile>,
    index: usize,
}

impl ZipEntryStream {
    pub fn from_zip_file_and_index(file: Arc<NamedTempFile>, index: usize) -> Self {
        Self { file, index }
    }
}

impl ReplayableStream for ZipEntryStream {
    type Item = Result<Bytes, String>;
    type Error = String;

    async fn make_stream(&self) -> Result<impl Stream<Item = Self::Item> + Send + 'static, String> {
        let reopened = self
            .file
            .reopen()
            .map_err(|e| format!("Failed to reopen file: {e}"))?;
        let file = tokio::fs::File::from_std(reopened);
        let buf_reader = BufReader::new(file);
        let zip_archive = ZipFileReader::with_tokio(buf_reader)
            .await
            .map_err(|e| format!("Failed to open zip archive: {e}"))?;
        let entry_reader = zip_archive
            .into_entry(self.index)
            .await
            .map_err(|e| format!("Failed to read entry from archive: {e}"))?;
        let stream = ReaderStream::new(entry_reader.compat());
        let mapped_stream = stream.map_err(|e| format!("Error reading entry: {e}"));
        Ok(Box::pin(mapped_stream))
    }

    async fn length(&self) -> Result<u64, String> {
        let reopened = self
            .file
            .reopen()
            .map_err(|e| format!("Failed to reopen file: {e}"))?;
        let file = tokio::fs::File::from_std(reopened);
        let buf_reader = BufReader::new(file);
        let zip_archive = ZipFileReader::with_tokio(buf_reader)
            .await
            .map_err(|e| format!("Failed to open zip archive: {e}"))?;

        Ok(zip_archive
            .file()
            .entries()
            .get(self.index)
            .ok_or("Entry with not found in archive")?
            .uncompressed_size())
    }
}

fn compose_components(socket_bytes: &[u8], plug_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut graph = CompositionGraph::new();

    let socket = Package::from_bytes("socket", None, socket_bytes, graph.types_mut())?;
    let socket = graph.register_package(socket)?;

    let plug_package = Package::from_bytes("plug", None, plug_bytes, graph.types_mut())?;
    let plub_package_id = graph.register_package(plug_package)?;

    match wac_graph::plug(&mut graph, vec![plub_package_id], socket) {
        Ok(()) => {
            let bytes = graph.encode(EncodeOptions::default())?;
            Ok(bytes)
        }
        Err(PlugError::NoPlugHappened) => {
            info!("No plugs where executed when composing components");
            Ok(socket_bytes.to_vec())
        }
        Err(error) => Err(error.into()),
    }
}

fn initial_component_file_path_from_zip_entry(
    entry: &ZipEntry,
) -> Result<ComponentFilePath, ComponentError> {
    let file_path = entry.filename().as_str().map_err(|e| {
        ComponentError::malformed_component_archive_from_message(format!(
            "Failed to convert filename to string: {e}"
        ))
    })?;

    // convert windows path separators to unix and sanitize the path
    let file_path: String = file_path
        .replace('\\', "/")
        .split('/')
        .map(sanitize_filename::sanitize)
        .collect::<Vec<_>>()
        .join("/");

    ComponentFilePath::from_abs_str(&format!("/{file_path}")).map_err(|e| {
        ComponentError::malformed_component_archive_from_message(format!(
            "Failed to convert path to InitialComponentFilePath: {e}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::service::component::ComponentError;
    use golem_common::SafeDisplay;
    use golem_service_base::repo::RepoError;

    #[test]
    pub fn test_repo_error_to_service_error() {
        let repo_err = RepoError::Internal("some sql error".to_string());
        let component_err: ComponentError = repo_err.into();
        assert_eq!(
            component_err.to_safe_string(),
            "Internal repository error".to_string()
        );
    }
}
