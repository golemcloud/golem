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

use crate::model::InitialComponentFilesArchiveAndPermissions;
use crate::model::{Component, ComponentConstraints};
use crate::repo::component::{record_metadata_serde, ComponentRecord, FileRecord};
use crate::repo::component::{ComponentConstraintsRecord, ComponentRepo};
use crate::service::component_compilation::ComponentCompilationService;
use crate::service::component_object_store::ComponentObjectStore;
use crate::service::plugin::{PluginError, PluginService};
use async_trait::async_trait;
use async_zip::tokio::read::seek::ZipFileReader;
use async_zip::ZipEntry;
use bytes::Bytes;
use futures::TryStreamExt;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_error;
use golem_common::model::component::ComponentOwner;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::component_metadata::{
    ComponentMetadata, ComponentProcessingError, DynamicLinkedInstance,
};
use golem_common::model::plugin::{
    ComponentPluginInstallationTarget, PluginInstallation, PluginInstallationCreation,
    PluginInstallationUpdate, PluginScope, PluginTypeSpecificDefinition,
};
use golem_common::model::ComponentVersion;
use golem_common::model::RetryConfig;
use golem_common::model::{AccountId, PluginInstallationId};
use golem_common::model::{
    ComponentFilePath, ComponentFilePermissions, ComponentId, ComponentType, InitialComponentFile,
    InitialComponentFileKey,
};
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_service_base::repo::plugin_installation::PluginInstallationRecord;
use golem_service_base::repo::RepoError;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::ReplayableStream;
use golem_wasm_ast::analysis::AnalysedType;
use http::StatusCode;
use reqwest::multipart::{Form, Part};
use rib::{FunctionTypeRegistry, RegistryKey, RegistryValue};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::vec;
use tap::TapFallible;
use tempfile::NamedTempFile;
use tokio::io::BufReader;
use tokio_stream::Stream;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, info_span};
use tracing_futures::Instrument;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component already exists: {0}")]
    AlreadyExists(ComponentId),
    #[error("Unknown component id: {0}")]
    UnknownComponentId(ComponentId),
    #[error("Unknown versioned component id: {0}")]
    UnknownVersionedComponentId(VersionedComponentId),
    #[error(transparent)]
    ComponentProcessingError(#[from] ComponentProcessingError),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error("Internal error: failed to convert {what}: {error}")]
    InternalConversionError { what: String, error: String },
    #[error("Internal component store error: {message}: {error}")]
    ComponentStoreError { message: String, error: String },
    #[error("Component Constraint Error. Make sure the component is backward compatible as the functions are already in use:\n{0}"
    )]
    ComponentConstraintConflictError(ConflictReport),
    #[error("Component Constraint Create Error: {0}")]
    ComponentConstraintCreateError(String),
    #[error("Malformed component archive error: {message}: {error:?}")]
    MalformedComponentArchiveError {
        message: String,
        error: Option<String>,
    },
    #[error("Failed uploading initial component files: {message}: {error}")]
    InitialComponentFileUploadError { message: String, error: String },
    #[error("Provided component file not found: {path} (key: {key})")]
    InitialComponentFileNotFound { path: String, key: String },
    #[error("Component transformation plugin was not found ({plugin_name}:{plugin_version})")]
    TransformationPluginNotFound {
        plugin_name: String,
        plugin_version: String,
    },
    #[error(transparent)]
    InternalPluginError(#[from] Box<PluginError>),
    #[error("Component transformation failed: {0}")]
    TransformationFailed(TransformationFailedReason),
}

impl ComponentError {
    pub fn conversion_error(what: impl AsRef<str>, error: String) -> ComponentError {
        Self::InternalConversionError {
            what: what.as_ref().to_string(),
            error,
        }
    }

    pub fn component_store_error(message: impl AsRef<str>, error: anyhow::Error) -> ComponentError {
        Self::ComponentStoreError {
            message: message.as_ref().to_string(),
            error: format!("{error}"),
        }
    }

    pub fn malformed_component_archive_from_message(message: impl AsRef<str>) -> Self {
        Self::MalformedComponentArchiveError {
            message: message.as_ref().to_string(),
            error: None,
        }
    }

    pub fn malformed_component_archive_from_error(
        message: impl AsRef<str>,
        error: anyhow::Error,
    ) -> Self {
        Self::MalformedComponentArchiveError {
            message: message.as_ref().to_string(),
            error: Some(format!("{error}")),
        }
    }

    pub fn initial_component_file_upload_error(
        message: impl AsRef<str>,
        error: impl AsRef<str>,
    ) -> Self {
        Self::InitialComponentFileUploadError {
            message: message.as_ref().to_string(),
            error: error.as_ref().to_string(),
        }
    }

    pub fn initial_component_file_not_found(
        path: &ComponentFilePath,
        key: &InitialComponentFileKey,
    ) -> Self {
        Self::InitialComponentFileNotFound {
            path: path.to_string(),
            key: key.to_string(),
        }
    }
}

impl SafeDisplay for ComponentError {
    fn to_safe_string(&self) -> String {
        match self {
            ComponentError::AlreadyExists(_) => self.to_string(),
            ComponentError::UnknownComponentId(_) => self.to_string(),
            ComponentError::UnknownVersionedComponentId(_) => self.to_string(),
            ComponentError::ComponentProcessingError(inner) => inner.to_safe_string(),
            ComponentError::InternalRepoError(inner) => inner.to_safe_string(),
            ComponentError::InternalConversionError { .. } => self.to_string(),
            ComponentError::ComponentStoreError { .. } => self.to_string(),
            ComponentError::ComponentConstraintConflictError(_) => self.to_string(),
            ComponentError::ComponentConstraintCreateError(_) => self.to_string(),
            ComponentError::MalformedComponentArchiveError { .. } => self.to_string(),
            ComponentError::InitialComponentFileUploadError { .. } => self.to_string(),
            ComponentError::InitialComponentFileNotFound { .. } => self.to_string(),
            ComponentError::TransformationPluginNotFound { .. } => self.to_string(),
            ComponentError::InternalPluginError(_) => self.to_string(),
            ComponentError::TransformationFailed(_) => self.to_string(),
        }
    }
}

impl From<RepoError> for ComponentError {
    fn from(error: RepoError) -> Self {
        ComponentError::InternalRepoError(error)
    }
}

impl From<ComponentError> for golem_api_grpc::proto::golem::component::v1::ComponentError {
    fn from(value: ComponentError) -> Self {
        let error = match value {
            ComponentError::AlreadyExists(_) => component_error::Error::AlreadyExists(ErrorBody {
                error: value.to_safe_string(),
            }),
            ComponentError::UnknownComponentId(_)
            | ComponentError::UnknownVersionedComponentId(_) => {
                component_error::Error::NotFound(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::ComponentProcessingError(error) => {
                component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                })
            }
            ComponentError::InternalRepoError(_) => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::InternalConversionError { .. } => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::ComponentStoreError { .. } => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::ComponentConstraintConflictError(_) => {
                component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                })
            }
            ComponentError::ComponentConstraintCreateError(_) => {
                component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                })
            }
            ComponentError::InitialComponentFileUploadError { .. } => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::MalformedComponentArchiveError { .. } => {
                component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                })
            }
            ComponentError::InitialComponentFileNotFound { .. } => {
                component_error::Error::NotFound(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::TransformationPluginNotFound { .. } => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::InternalPluginError(_) => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            ComponentError::TransformationFailed(_) => {
                component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        Self { error: Some(error) }
    }
}

#[derive(Debug)]
pub enum TransformationFailedReason {
    Failure(String),
    Request(reqwest::Error),
    HttpStatus(StatusCode),
}

impl Display for TransformationFailedReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformationFailedReason::Failure(message) => write!(f, "{message}"),
            TransformationFailedReason::Request(error) => write!(f, "Request error: {error}"),
            TransformationFailedReason::HttpStatus(status) => write!(f, "HTTP status: {status}"),
        }
    }
}

impl SafeDisplay for TransformationFailedReason {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

pub fn create_new_versioned_component_id(component_id: &ComponentId) -> VersionedComponentId {
    VersionedComponentId {
        component_id: component_id.clone(),
        version: 0,
    }
}

#[async_trait]
pub trait ComponentService<Owner: ComponentOwner>: Debug {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError>;

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
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError>;

    // Files must have been uploaded to the blob store before calling this method
    async fn update_internal(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        // None signals that files should be reused from the previous version
        files: Option<Vec<InitialComponentFile>>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError>;

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &Owner,
    ) -> Result<Vec<u8>, ComponentError>;

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &Owner,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>> + Send + Sync>>,
        ComponentError,
    >;

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        owner: &Owner,
    ) -> Result<Vec<Component<Owner>>, ComponentError>;

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        owner: &Owner,
    ) -> Result<Option<ComponentId>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        owner: &Owner,
    ) -> Result<Option<Component<Owner>>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        owner: &Owner,
    ) -> Result<Option<Component<Owner>>, ComponentError>;

    async fn get(
        &self,
        component_id: &ComponentId,
        owner: &Owner,
    ) -> Result<Vec<Component<Owner>>, ComponentError>;

    async fn get_owner(&self, component_id: &ComponentId) -> Result<Option<Owner>, ComponentError>;

    async fn delete(&self, component_id: &ComponentId, owner: &Owner)
        -> Result<(), ComponentError>;

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints<Owner>,
    ) -> Result<ComponentConstraints<Owner>, ComponentError>;

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
        owner: &Owner,
    ) -> Result<Option<FunctionConstraintCollection>, ComponentError>;

    /// Gets the list of installed plugins for a given component version belonging to `owner`
    async fn get_plugin_installations_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, PluginError>;

    async fn create_plugin_installation_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, PluginError>;

    async fn update_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError>;

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), PluginError>;
}

pub struct ComponentServiceDefault<Owner: ComponentOwner, Scope: PluginScope> {
    component_repo: Arc<dyn ComponentRepo<Owner> + Sync + Send>,
    object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
    component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    plugin_service: Arc<dyn PluginService<Owner::PluginOwner, Scope> + Sync + Send>,
}

impl<Owner: ComponentOwner, Scope: PluginScope> Debug for ComponentServiceDefault<Owner, Scope> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentServiceDefault").finish()
    }
}

impl<Owner: ComponentOwner, Scope: PluginScope> ComponentServiceDefault<Owner, Scope> {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo<Owner> + Sync + Send>,
        object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
        component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        plugin_service: Arc<dyn PluginService<Owner::PluginOwner, Scope> + Sync + Send>,
    ) -> Self {
        ComponentServiceDefault {
            component_repo,
            object_store,
            component_compilation,
            initial_component_files_service,
            plugin_service,
        }
    }

    pub fn find_component_metadata_conflicts(
        function_constraint_collection: &FunctionConstraintCollection,
        new_type_registry: &FunctionTypeRegistry,
    ) -> ConflictReport {
        let mut missing_functions = vec![];
        let mut conflicting_functions = vec![];

        for existing_function_call in &function_constraint_collection.function_constraints {
            if let Some(new_registry_value) =
                new_type_registry.lookup(&existing_function_call.function_key)
            {
                let mut parameter_conflict = false;
                let mut return_conflict = false;

                if existing_function_call.parameter_types != new_registry_value.argument_types() {
                    parameter_conflict = true;
                }

                let new_return_types = match new_registry_value.clone() {
                    RegistryValue::Function { return_types, .. } => return_types,
                    _ => vec![],
                };

                if existing_function_call.return_types != new_return_types {
                    return_conflict = true;
                }

                if parameter_conflict || return_conflict {
                    conflicting_functions.push(ConflictingFunction {
                        function: existing_function_call.function_key.clone(),
                        existing_parameter_types: existing_function_call.parameter_types.clone(),
                        new_parameter_types: new_registry_value.clone().argument_types().clone(),
                        existing_result_types: existing_function_call.return_types.clone(),
                        new_result_types: new_return_types,
                    });
                }
            } else {
                missing_functions.push(existing_function_call.function_key.clone());
            }
        }

        ConflictReport {
            missing_functions,
            conflicting_functions,
        }
    }

    async fn upload_component_files(
        &self,
        account_id: &AccountId,
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
                    .put_if_not_exists(account_id, &stream)
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
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError> {
        let component = Component::new(
            component_id.clone(),
            component_name.clone(),
            component_type,
            &data,
            uploaded_files,
            installed_plugins,
            dynamic_linking,
            owner.clone(),
        )?;

        info!(
            owner = %owner,
            exports = ?component.metadata.exports,
            dynamic_linking = ?component.metadata.dynamic_linking,
            "Uploaded component",
        );

        let transformed_data = self.apply_transformations(&component, data.clone()).await?;
        let transformed_metadata = ComponentMetadata::analyse_component(&transformed_data)
            .map_err(ComponentError::ComponentProcessingError)?;

        tokio::try_join!(
            self.upload_user_component(&component, data),
            self.upload_protected_component(&component, transformed_data)
        )?;

        let mut record = ComponentRecord::try_from_model(component.clone(), true)
            .map_err(|e| ComponentError::conversion_error("record", e))?;
        record.metadata = record_metadata_serde::serialize(&transformed_metadata)
            .map_err(|err| ComponentError::conversion_error("metadata", err))?
            .to_vec();

        let result = self.component_repo.create(&record).await;
        if let Err(RepoError::UniqueViolation(_)) = result {
            Err(ComponentError::AlreadyExists(component_id.clone()))?;
        }

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version)
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
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError> {
        let mut metadata = ComponentMetadata::analyse_component(&data)
            .map_err(ComponentError::ComponentProcessingError)?;
        metadata.dynamic_linking = dynamic_linking;

        let constraints = self
            .component_repo
            .get_constraint(&owner.to_string(), &component_id.0)
            .await?;

        let new_type_registry = FunctionTypeRegistry::from_export_metadata(&metadata.exports);

        if let Some(constraints) = constraints {
            let conflicts =
                Self::find_component_metadata_conflicts(&constraints, &new_type_registry);
            if !conflicts.is_empty() {
                return Err(ComponentError::ComponentConstraintConflictError(conflicts));
            }
        }

        info!(
            owner = %owner,
            exports = ?metadata.exports,
            dynamic_linking = ?metadata.dynamic_linking,
            "Uploaded component",
        );

        let files = files.map(|files| {
            files
                .into_iter()
                .map(|file| {
                    FileRecord::from_component_id_and_version_and_file(component_id.0, 0, &file)
                })
                .collect()
        });

        let owner_record: Owner::Row = owner.clone().into();
        let component_record = self
            .component_repo
            .update(
                &owner_record,
                &owner.to_string(),
                &component_id.0,
                data.clone(),
                record_metadata_serde::serialize(&metadata)
                    .map_err(|err| ComponentError::conversion_error("metadata", err))?
                    .to_vec(),
                component_type.map(|ct| ct as i32),
                files,
            )
            .await?;
        let mut component: Component<Owner> = component_record
            .clone()
            .try_into()
            .map_err(|e| ComponentError::conversion_error("record", e))?;
        let object_store_key = component.versioned_component_id.to_string();
        component.object_store_key = Some(object_store_key.clone());
        component.transformed_object_store_key = Some(object_store_key.clone());

        debug!("Result component: {component:?}");

        let transformed_data = self.apply_transformations(&component, data.clone()).await?;
        let transformed_metadata = ComponentMetadata::analyse_component(&transformed_data)
            .map_err(ComponentError::ComponentProcessingError)?;

        tokio::try_join!(
            self.upload_user_component(&component, data),
            self.upload_protected_component(&component, transformed_data)
        )?;

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version)
            .await;

        self.component_repo
            .activate(
                &owner.to_string(),
                &component_id.0,
                component.versioned_component_id.version as i64,
                &object_store_key,
                &object_store_key,
                record_metadata_serde::serialize(&transformed_metadata)
                    .map_err(|err| ComponentError::conversion_error("metadata", err))?
                    .to_vec(),
            )
            .await?;

        Ok(component)
    }

    async fn upload_user_component(
        &self,
        component: &Component<Owner>,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(&component.user_object_store_key(), data)
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload user component", e)
            })
    }

    async fn upload_protected_component(
        &self,
        component: &Component<Owner>,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(&component.protected_object_store_key(), data)
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload protected component", e)
            })
    }

    async fn apply_transformations(
        &self,
        component: &Component<Owner>,
        mut data: Vec<u8>,
    ) -> Result<Vec<u8>, ComponentError> {
        if !component.installed_plugins.is_empty() {
            let mut installed_plugins = component.installed_plugins.clone();
            installed_plugins.sort_by_key(|p| p.priority);

            let plugin_owner = component.owner.clone().into();

            for installation in installed_plugins {
                let plugin = self
                    .plugin_service
                    .get(&plugin_owner, &installation.name, &installation.version)
                    .await
                    .map_err(Box::new)?;

                if let Some(plugin) = plugin {
                    if let PluginTypeSpecificDefinition::ComponentTransformer(spec) = plugin.specs {
                        let span = info_span!("component transformation",
                            owner = %component.owner,
                            component_id = %component.versioned_component_id,
                            plugin_name = %installation.name,
                            plugin_version = %installation.version,
                            plugin_installation_id = %installation.id,
                        );

                        data = self
                            .apply_transformation(
                                component,
                                &data,
                                spec.transform_url,
                                &installation.parameters,
                            )
                            .instrument(span)
                            .await?;
                    }
                } else {
                    Err(ComponentError::TransformationPluginNotFound {
                        plugin_name: installation.name.clone(),
                        plugin_version: installation.version.clone(),
                    })?
                }
            }
        }

        Ok(data)
    }

    async fn apply_transformation(
        &self,
        component: &Component<Owner>,
        data: &[u8],
        url: String,
        parameters: &HashMap<String, String>,
    ) -> Result<Vec<u8>, ComponentError> {
        info!(%url, "Applying component transformation plugin");

        // NOTE: the client could be cached per target url to keep connection pools open for component
        //       transformer plugins, however this is not done yet as component update is not that frequent
        let client = reqwest::Client::new();
        let serializable_component: golem_service_base::model::Component = component.clone().into();
        let response = with_retries(
            "component_transformer_plugin",
            "transform",
            None,
            &RetryConfig::default(), // TODO
            &(client, serializable_component, url, data, parameters),
            |(client, serializable_component, url, data, parameters)| {
                Box::pin(async move {
                    let mut form = Form::new();
                    form = form.part("component", Part::bytes(data.to_vec()));
                    form = form.part(
                        "metadata",
                        Part::text(serde_json::to_string(&serializable_component).map_err(
                            |err| {
                                ComponentError::conversion_error(
                                    "component metadata",
                                    err.to_string(),
                                )
                            },
                        )?)
                        .mime_str("application/json")
                        .unwrap(),
                    );
                    for (key, value) in *parameters {
                        if key == "component" {
                            return Err(ComponentError::TransformationFailed(
                                TransformationFailedReason::Failure(
                                    "Parameter key 'component' is reserved".to_string(),
                                ),
                            ));
                        }
                        if key == "metadata" {
                            return Err(ComponentError::TransformationFailed(
                                TransformationFailedReason::Failure(
                                    "Parameter key 'metadata' is reserved".to_string(),
                                ),
                            ));
                        }
                        form = form.part(key.clone(), Part::text(value.clone()));
                    }

                    let request = client.post(url).multipart(form);

                    let response = request.send().await.map_err(|err| {
                        ComponentError::TransformationFailed(TransformationFailedReason::Request(
                            err,
                        ))
                    })?;

                    if response.status().is_server_error() {
                        return Err(ComponentError::TransformationFailed(
                            TransformationFailedReason::HttpStatus(response.status()),
                        ));
                    }

                    Ok(response)
                })
            },
            |err| {
                matches!(
                    err,
                    ComponentError::TransformationFailed(TransformationFailedReason::HttpStatus(_))
                        | ComponentError::TransformationFailed(
                            TransformationFailedReason::Request(_)
                        )
                )
            },
        )
        .await?;

        if response.status().is_success() {
            let body = response.bytes().await.map_err(|err| {
                ComponentError::TransformationFailed(TransformationFailedReason::Failure(format!(
                    "Failed to read response from transformation plugin: {}",
                    err
                )))
            })?;

            Ok(body.to_vec())
        } else {
            Err(ComponentError::TransformationFailed(
                TransformationFailedReason::HttpStatus(response.status()),
            ))
        }
    }

    async fn retransform(
        &self,
        namespace: &str,
        new_component: Component<Owner>,
    ) -> Result<(), PluginError> {
        let data = self
            .object_store
            .get(&new_component.user_object_store_key())
            .await
            .map_err(|err| {
                ComponentError::component_store_error("Failed to download user component", err)
            })?;

        let transformed_data = self.apply_transformations(&new_component, data).await?;
        let transformed_metadata = ComponentMetadata::analyse_component(&transformed_data)
            .map_err(ComponentError::ComponentProcessingError)?;

        self.object_store
            .put(
                &new_component.protected_object_store_key(),
                transformed_data,
            )
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload protected component", e)
            })?;

        self.component_repo
            .activate(
                namespace,
                &new_component.versioned_component_id.component_id.0,
                new_component.versioned_component_id.version as i64,
                &new_component
                    .object_store_key
                    .unwrap_or(new_component.versioned_component_id.to_string()),
                &new_component.versioned_component_id.to_string(),
                record_metadata_serde::serialize(&transformed_metadata)
                    .map_err(|err| ComponentError::conversion_error("metadata", err))?
                    .to_vec(),
            )
            .await?;

        Ok(())
    }
}

#[async_trait]
impl<Owner: ComponentOwner, Scope: PluginScope> ComponentService<Owner>
    for ComponentServiceDefault<Owner, Scope>
{
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError> {
        info!(owner = %owner, "Create component");

        self.find_id_by_name(component_name, owner)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let uploaded_files = match files {
            Some(files) => {
                self.upload_component_files(&owner.account_id(), files)
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
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError> {
        info!(owner = %owner, "Create component");

        self.find_id_by_name(component_name, owner)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        for file in &files {
            let exists = self
                .initial_component_files_service
                .exists(&owner.account_id(), &file.key)
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
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError> {
        info!(owner = %owner, "Update component");

        let uploaded_files = match files {
            Some(files) => Some(
                self.upload_component_files(&owner.account_id(), files)
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
        owner: &Owner,
    ) -> Result<Component<Owner>, ComponentError> {
        info!(owner = %owner, "Update component");

        for file in files.iter().flatten() {
            let exists = self
                .initial_component_files_service
                .exists(&owner.account_id(), &file.key)
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
        )
        .await
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<ComponentVersion>,
        owner: &Owner,
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
                .get(&component.protected_object_store_key())
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
        owner: &Owner,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>> + Send + Sync>>,
        ComponentError,
    > {
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
            info!(owner = %owner, component_id = %component.versioned_component_id, "Download component as stream");

            let stream = self
                .object_store
                .get_stream(&component.protected_object_store_key())
                .await;

            Ok(stream)
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        owner: &Owner,
    ) -> Result<Vec<Component<Owner>>, ComponentError> {
        info!(owner = %owner, "Find component by name");

        let records = match component_name {
            Some(name) => {
                self.component_repo
                    .get_by_name(&owner.to_string(), &name.0)
                    .await?
            }
            None => self.component_repo.get_all(&owner.to_string()).await?,
        };

        let values: Vec<Component<Owner>> = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component<Owner>>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        owner: &Owner,
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
        owner: &Owner,
    ) -> Result<Option<Component<Owner>>, ComponentError> {
        info!(owner = %owner, "Get component by version");

        let result = self
            .component_repo
            .get_by_version(
                &owner.to_string(),
                &component_id.component_id.0,
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
        owner: &Owner,
    ) -> Result<Option<Component<Owner>>, ComponentError> {
        info!(owner = %owner, "Get latest component");
        let result = self
            .component_repo
            .get_latest_version(&owner.to_string(), &component_id.0)
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
        owner: &Owner,
    ) -> Result<Vec<Component<Owner>>, ComponentError> {
        info!(owner = %owner, component_id = %component_id ,"Get component");
        let records = self
            .component_repo
            .get(&owner.to_string(), &component_id.0)
            .await?;

        let values: Vec<Component<Owner>> = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component<Owner>>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn get_owner(&self, component_id: &ComponentId) -> Result<Option<Owner>, ComponentError> {
        info!(component_id = %component_id, "Get component owner");
        let result = self.component_repo.get_namespace(&component_id.0).await?;
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
        owner: &Owner,
    ) -> Result<(), ComponentError> {
        info!(owner = %owner, component_id = %component_id, "Delete component");

        let records = self
            .component_repo
            .get(&owner.to_string(), &component_id.0)
            .await?;
        let components = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component<Owner>>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let mut object_store_keys = Vec::new();

        for component in components {
            if component.owns_stored_object() {
                object_store_keys.push(component.protected_object_store_key());
                object_store_keys.push(component.user_object_store_key());
            }
        }

        if !object_store_keys.is_empty() {
            for key in object_store_keys {
                self.object_store.delete(&key).await.map_err(|e| {
                    ComponentError::component_store_error("Failed to delete component data", e)
                })?;
            }
            self.component_repo
                .delete(&owner.to_string(), &component_id.0)
                .await?;
            Ok(())
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints<Owner>,
    ) -> Result<ComponentConstraints<Owner>, ComponentError> {
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
                &component_constraint.component_id.0,
            )
            .await?
            .ok_or(ComponentError::ComponentConstraintCreateError(format!(
                "Failed to create constraints for {}",
                component_id
            )))?;

        let component_constraints = ComponentConstraints {
            owner: component_constraint.owner.clone(),
            component_id: component_id.clone(),
            constraints: result,
        };

        Ok(component_constraints)
    }

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
        owner: &Owner,
    ) -> Result<Option<FunctionConstraintCollection>, ComponentError> {
        info!(component_id = %component_id, "Get component constraint");

        let result = self
            .component_repo
            .get_constraint(&owner.to_string(), &component_id.0)
            .await?;
        Ok(result)
    }

    async fn get_plugin_installations_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, PluginError> {
        let owner_record: Owner::Row = owner.clone().into();
        let plugin_owner_record = owner_record.into();
        let records = self
            .component_repo
            .get_installed_plugins(&plugin_owner_record, &component_id.0, component_version)
            .await?;

        records
            .into_iter()
            .map(PluginInstallation::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::conversion_error("plugin installation", e))
    }

    async fn create_plugin_installation_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, PluginError> {
        let namespace = owner.to_string();
        let owner_row: Owner::Row = owner.clone().into();
        let plugin_owner_row = owner_row.into();

        let latest = self
            .component_repo
            .get_latest_version(&namespace, &component_id.0)
            .await?;

        if let Some(latest) = latest {
            let installation = installation.with_generated_id();
            let record = PluginInstallationRecord {
                installation_id: installation.id.0,
                plugin_name: installation.name.clone(),
                plugin_version: installation.version.clone(),
                priority: installation.priority,
                parameters: serde_json::to_vec(&installation.parameters).map_err(|e| {
                    PluginError::conversion_error("plugin installation parameters", e.to_string())
                })?,
                target: ComponentPluginInstallationTarget {
                    component_id: component_id.clone(),
                    component_version: latest.version as u64,
                }
                .into(),
                owner: plugin_owner_row,
            };

            let new_component_version = self.component_repo.install_plugin(&record).await?;
            let new_versioned_component_id = VersionedComponentId {
                component_id: component_id.clone(),
                version: new_component_version,
            };
            let mut new_component: Component<Owner> = latest
                .try_into()
                .map_err(|err| ComponentError::conversion_error("component", err))?;
            new_component.versioned_component_id = new_versioned_component_id;
            new_component.transformed_object_store_key = None;
            new_component.installed_plugins.push(installation.clone());

            self.retransform(&namespace, new_component).await?;

            Ok(installation)
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: component_id.clone(),
            })
        }
    }

    async fn update_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError> {
        let namespace: String = owner.to_string();
        let owner_record: Owner::Row = owner.clone().into();
        let plugin_owner_record = owner_record.into();

        let latest = self
            .component_repo
            .get_latest_version(&owner.to_string(), &component_id.0)
            .await?;

        if let Some(latest) = latest {
            let new_component_version = self
                .component_repo
                .update_plugin_installation(
                    &plugin_owner_record,
                    &component_id.0,
                    &installation_id.0,
                    update.priority,
                    serde_json::to_vec(&update.parameters).map_err(|e| {
                        PluginError::conversion_error(
                            "plugin installation parameters",
                            e.to_string(),
                        )
                    })?,
                )
                .await?;

            let new_versioned_component_id = VersionedComponentId {
                component_id: component_id.clone(),
                version: new_component_version,
            };
            let mut new_component: Component<Owner> = latest
                .try_into()
                .map_err(|err| ComponentError::conversion_error("component", err))?;
            new_component.versioned_component_id = new_versioned_component_id;
            new_component.transformed_object_store_key = None;

            for installation in &mut new_component.installed_plugins {
                if &installation.id == installation_id {
                    installation.priority = update.priority;
                    installation.parameters = update.parameters.clone();
                }
            }

            self.retransform(&namespace, new_component).await?;

            Ok(())
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: component_id.clone(),
            })
        }
    }

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), PluginError> {
        let namespace: String = owner.to_string();
        let owner_record: Owner::Row = owner.clone().into();
        let plugin_owner_record = owner_record.into();

        let latest = self
            .component_repo
            .get_latest_version(&owner.to_string(), &component_id.0)
            .await?;

        if let Some(latest) = latest {
            let new_component_version = self
                .component_repo
                .uninstall_plugin(&plugin_owner_record, &component_id.0, &installation_id.0)
                .await?;

            let new_versioned_component_id = VersionedComponentId {
                component_id: component_id.clone(),
                version: new_component_version,
            };
            let mut new_component: Component<Owner> = latest
                .try_into()
                .map_err(|err| ComponentError::conversion_error("component", err))?;
            new_component.versioned_component_id = new_versioned_component_id;
            new_component.transformed_object_store_key = None;
            new_component
                .installed_plugins
                .retain(|i| &i.id != installation_id);

            self.retransform(&namespace, new_component).await?;

            Ok(())
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: component_id.clone(),
            })
        }
    }
}

#[derive(Debug)]
pub struct ConflictingFunction {
    pub function: RegistryKey,
    pub existing_parameter_types: Vec<AnalysedType>,
    pub new_parameter_types: Vec<AnalysedType>,
    pub existing_result_types: Vec<AnalysedType>,
    pub new_result_types: Vec<AnalysedType>,
}

impl Display for ConflictingFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Function: {}", self.function)?;
        writeln!(f, "  Parameter Type Conflict:")?;
        writeln!(
            f,
            "    Existing: {}",
            internal::convert_to_pretty_types(&self.existing_parameter_types)
        )?;
        writeln!(
            f,
            "    New:      {}",
            internal::convert_to_pretty_types(&self.new_parameter_types)
        )?;

        writeln!(f, "  Result Type Conflict:")?;
        writeln!(
            f,
            "    Existing: {}",
            internal::convert_to_pretty_types(&self.existing_result_types)
        )?;
        writeln!(
            f,
            "    New:      {}",
            internal::convert_to_pretty_types(&self.new_result_types)
        )?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct ConflictReport {
    pub missing_functions: Vec<RegistryKey>,
    pub conflicting_functions: Vec<ConflictingFunction>,
}

impl Display for ConflictReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Handling missing functions
        writeln!(f, "Missing Functions:")?;
        if self.missing_functions.is_empty() {
            writeln!(f, "  None")?;
        } else {
            for missing_function in &self.missing_functions {
                writeln!(f, "  - {}", missing_function)?;
            }
        }

        // Handling conflicting functions
        writeln!(f, "\nFunctions with conflicting types:")?;
        if self.conflicting_functions.is_empty() {
            writeln!(f, "  None")?;
        } else {
            for conflict in &self.conflicting_functions {
                writeln!(f, "{}", conflict)?;
            }
        }

        Ok(())
    }
}

impl ConflictReport {
    pub fn is_empty(&self) -> bool {
        self.missing_functions.is_empty() && self.conflicting_functions.is_empty()
    }
}

fn initial_component_file_path_from_zip_entry(
    entry: &ZipEntry,
) -> Result<ComponentFilePath, ComponentError> {
    let file_path = entry.filename().as_str().map_err(|e| {
        ComponentError::malformed_component_archive_from_message(format!(
            "Failed to convert filename to string: {}",
            e
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
            "Failed to convert path to InitialComponentFilePath: {}",
            e
        ))
    })
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

#[async_trait]
impl ReplayableStream for ZipEntryStream {
    type Item = Result<Bytes, String>;

    async fn make_stream(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Self::Item> + Send + Sync>>, String> {
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

mod internal {
    use golem_wasm_ast::analysis::AnalysedType;
    pub(crate) fn convert_to_pretty_types(analysed_types: &[AnalysedType]) -> String {
        let type_names = analysed_types
            .iter()
            .map(|x| {
                rib::TypeName::try_from(x.clone()).map_or("unknown".to_string(), |x| x.to_string())
            })
            .collect::<Vec<_>>();

        type_names.join(", ")
    }
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
