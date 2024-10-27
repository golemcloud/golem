// Copyright 2024 Golem Cloud
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

use std::fmt::{Debug, Display, Formatter};
use std::collections::{HashMap, HashSet};
use std::vec;
use async_zip::ZipEntry;
use bytes::Bytes;
use sha2::{Sha256, Digest};
use std::num::TryFromIntError;
use std::pin::Pin;
use std::sync::Arc;

use crate::model::{Component, ComponentConstraints};
use crate::repo::component::{ComponentConstraintsRecord, ComponentRepo};
use crate::model::{InitialComponentFilesArchiveAndPermissions};
use crate::service::component_compilation::ComponentCompilationService;
use async_trait::async_trait;
use chrono::Utc;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_error;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::component_metadata::{ComponentMetadata, ComponentProcessingError};
use golem_common::model::{ComponentId, ComponentType};
use golem_common::model::{InitialComponentFile, InitialComponentFileKey, InitialComponentFilePath, InitialComponentFilePermissions};
use golem_common::SafeDisplay;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_service_base::repo::RepoError;
use golem_service_base::service::component_object_store::ComponentObjectStore;
use golem_wasm_ast::analysis::AnalysedType;
use rib::{FunctionTypeRegistry, RegistryKey, RegistryValue};
use tap::TapFallible;
use tokio_stream::Stream;
use tracing::{error, info};
use golem_worker_executor_base::services::initial_component_files::{InitialComponentFilesService};
use async_zip::tokio::read::seek::ZipFileReader;
use tokio::io::BufReader;

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
    #[error("Component Constraint Error. Make sure the component is backward compatible as the functions are already in use:\n{0}")]
    ComponentConstraintConflictError(ConflictReport),
    #[error("Component Constraint Create Error: {0}")]
    ComponentConstraintCreateError(String),
    #[error("Malformed component archive error: {message}: {error:?}")]
    MalformedComponentArchiveError { message: String, error: Option<String> },
    #[error("Failed uploading initial component files: {message}: {error}")]
    InitialComponentFileUploadError { message: String, error: String },
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

    pub fn malformed_component_archive_from_error(message: impl AsRef<str>, error: anyhow::Error) -> Self {
        Self::MalformedComponentArchiveError {
            message: message.as_ref().to_string(),
            error: Some(format!("{error}")),
        }
    }

    pub fn initial_component_file_upload_error(message: impl AsRef<str>, error: impl AsRef<str>) -> Self {
        Self::InitialComponentFileUploadError {
            message: message.as_ref().to_string(),
            error: error.as_ref().to_string(),
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
        };
        Self { error: Some(error) }
    }
}

pub fn create_new_versioned_component_id(
    component_id: &ComponentId,
) -> VersionedComponentId {
    VersionedComponentId {
        component_id: component_id.clone(),
        version: 0,
    }
}

#[async_trait]
pub trait ComponentService<Namespace> {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError>;

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<Vec<u8>, ComponentError>;

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>> + Send + Sync>>,
        ComponentError,
    >;

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<Option<Vec<u8>>, ComponentError>;

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        namespace: &Namespace,
    ) -> Result<Vec<Component<Namespace>>, ComponentError>;

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        namespace: &Namespace,
    ) -> Result<Option<ComponentId>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component<Namespace>>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component<Namespace>>, ComponentError>;

    async fn get(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Vec<Component<Namespace>>, ComponentError>;

    async fn get_namespace(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError>;

    async fn delete(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<(), ComponentError>;

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints<Namespace>,
    ) -> Result<ComponentConstraints<Namespace>, ComponentError>;

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<FunctionConstraintCollection>, ComponentError>;
}

pub struct ComponentServiceDefault {
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
    object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
    component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
    initial_component_files_service: Arc<dyn InitialComponentFilesService + Sync + Send>,
}

impl ComponentServiceDefault {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo + Sync + Send>,
        object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
        component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
        initial_component_files_service: Arc<dyn InitialComponentFilesService + Sync + Send>,
    ) -> Self {
        ComponentServiceDefault {
            component_repo,
            object_store,
            component_compilation,
            initial_component_files_service,
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

#[async_trait]
impl<Namespace> ComponentService<Namespace> for ComponentServiceDefault
where
    Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
    <Namespace as TryFrom<String>>::Error: Display + Debug + Send + Sync + 'static,
{
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError> {
        info!(namespace = %namespace, "Create component");

        self.find_id_by_name(component_name, namespace)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let versioned_component_id = create_new_versioned_component_id(component_id);

        // analyze component before uploading anything so we fail early
        let component_metadata = ComponentMetadata::analyse_component(&data)
            .map_err(ComponentError::ComponentProcessingError)?;

        let component_size = data.len() as u64;

        info!(namespace = %namespace,"Uploaded component - exports {:?}", component_metadata.exports);

        let upload_files = async {
            match files {
                Some(files) => self.upload_component_files(files).await,
                None => Ok(vec![]),
            }
        };

        let (_, _, uploaded_files) = tokio::try_join!(
            self.upload_user_component(&versioned_component_id, data.clone()),
            self.upload_protected_component(&versioned_component_id, data),
            upload_files
        )?;

        let component = Component {
            component_size,
            metadata: component_metadata,
            created_at: Utc::now(),
            component_type,
            component_name: component_name.clone(),
            versioned_component_id: versioned_component_id.clone(),
            namespace: namespace.clone(),
            files: uploaded_files.clone(),
        };

        let record = component
            .clone()
            .try_into()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let result = self.component_repo.create(&record).await;

        if let Err(RepoError::UniqueViolation(_)) = result {
            Err(ComponentError::AlreadyExists(component_id.clone()))?;
        }

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version)
            .await;

        Ok(component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError> {
        info!(namespace = %namespace, "Update component");

        let created_at = Utc::now();

        let metadata = ComponentMetadata::analyse_component(&data)
            .map_err(ComponentError::ComponentProcessingError)?;

        let constraints = self.component_repo.get_constraint(component_id).await?;

        let new_type_registry = FunctionTypeRegistry::from_export_metadata(&metadata.exports);

        if let Some(constraints) = constraints {
            let conflicts =
                Self::find_component_metadata_conflicts(&constraints, &new_type_registry);
            if !conflicts.is_empty() {
                return Err(ComponentError::ComponentConstraintConflictError(conflicts));
            }
        }

        let next_component = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?
            .filter(|c| c.namespace == namespace.to_string())
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))
            .and_then(|c| {
                c.try_into()
                    .map_err(|e| ComponentError::conversion_error("record", e))
            })
            .map(Component::next_version)?;

        info!(namespace = %namespace, "Uploaded component - exports {:?}", metadata.exports);

        let component_size: u64 = data.len().try_into().map_err(|e: TryFromIntError| {
            ComponentError::conversion_error("data length", e.to_string())
        })?;

        let upload_files = async {
            match files {
                Some(files) => self.upload_component_files(files).await,
                None =>
                    // reuse files from the previous version
                    Ok(next_component.files.clone()),
            }
        };

        let (_, _, uploaded_files) = tokio::try_join!(
            self.upload_user_component(&next_component.versioned_component_id, data.clone()),
            self.upload_protected_component(&next_component.versioned_component_id, data),
            upload_files
        )?;

        let component = Component {
            component_size,
            metadata,
            created_at,
            component_type: component_type.unwrap_or(next_component.component_type),
            files: uploaded_files,
            ..next_component
        };
        let record = component
            .clone()
            .try_into()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        self.component_repo.create(&record).await?;

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version)
            .await;

        Ok(component)
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<Vec<u8>, ComponentError> {
        let versioned_component_id = self
            .get_versioned_component_id(component_id, version, namespace)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        info!(namespace = %namespace, "Download component");

        self.object_store
            .get(&self.get_protected_object_store_key(&versioned_component_id))
            .await
            .tap_err(
                |e| error!(namespace = %namespace, "Error downloading component - error: {}", e),
            )
            .map_err(|e| ComponentError::component_store_error("Error downloading component", e))
    }

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>> + Send + Sync>>,
        ComponentError,
    > {
        let versioned_component_id = self
            .get_versioned_component_id(component_id, version, namespace)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        info!(namespace = %namespace, "Download component as stream");

        let stream = self
            .object_store
            .get_stream(&self.get_protected_object_store_key(&versioned_component_id))
            .await;

        Ok(stream)
    }

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        info!(namespace = %namespace, "Get component protected data");

        let versioned_component_id = self
            .get_versioned_component_id(component_id, version, namespace)
            .await?;

        match versioned_component_id {
            Some(versioned_component_id) => {
                let data = self
                    .object_store
                    .get(&self.get_protected_object_store_key(&versioned_component_id))
                    .await
                    .tap_err(|e| error!(namespace = %namespace, "Error getting component data - error: {}", e))
                    .map_err(|e| {
                        ComponentError::component_store_error( "Error retrieving component", e)
                    })?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        namespace: &Namespace,
    ) -> Result<Vec<Component<Namespace>>, ComponentError> {
        info!(namespace = %namespace, "Find component by name");

        let records = match component_name {
            Some(name) => {
                self.component_repo
                    .get_by_name(namespace.to_string().as_str(), &name.0)
                    .await?
            }
            None => {
                self.component_repo
                    .get_all(namespace.to_string().as_str())
                    .await?
            }
        };

        let values: Vec<Component<Namespace>> = records
            .iter()
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component<Namespace>>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn find_id_by_name(
        &self,
        component_name: &ComponentName,
        namespace: &Namespace,
    ) -> Result<Option<ComponentId>, ComponentError> {
        info!(namespace = %namespace, "Find component id by name");
        let records = self
            .component_repo
            .get_id_by_name(namespace.to_string().as_str(), &component_name.0)
            .await?;
        Ok(records.map(ComponentId))
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component<Namespace>>, ComponentError> {
        info!(namespace = %namespace, "Get component by version");

        let result = self
            .component_repo
            .get_by_version(&component_id.component_id.0, component_id.version)
            .await?;

        match result {
            Some(c) if c.namespace == namespace.to_string() => {
                let value = c
                    .try_into()
                    .map_err(|e| ComponentError::conversion_error("record", e))?;
                Ok(Some(value))
            }
            _ => Ok(None),
        }
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component<Namespace>>, ComponentError> {
        info!(namespace = %namespace, "Get latest component");
        let result = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?;

        match result {
            Some(c) if c.namespace == namespace.to_string() => {
                let value = c
                    .try_into()
                    .map_err(|e| ComponentError::conversion_error("record", e))?;
                Ok(Some(value))
            }
            _ => Ok(None),
        }
    }

    async fn get(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Vec<Component<Namespace>>, ComponentError> {
        info!(namespace = %namespace, "Get component");
        let records = self.component_repo.get(&component_id.0).await?;

        let values: Vec<Component<Namespace>> = records
            .iter()
            .filter(|d| d.namespace == namespace.to_string())
            .map(|c| c.clone().try_into())
            .collect::<Result<Vec<Component<Namespace>>, _>>()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        Ok(values)
    }

    async fn get_namespace(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError> {
        info!("Get component namespace");
        let result = self.component_repo.get_namespace(&component_id.0).await?;
        if let Some(result) = result {
            let value =
                result
                    .clone()
                    .try_into()
                    .map_err(|e: <Namespace as TryFrom<String>>::Error| {
                        ComponentError::conversion_error("namespace", e.to_string())
                    })?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    async fn delete(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<(), ComponentError> {
        info!(namespace = %namespace, "Delete component");

        let records = self.component_repo.get(&component_id.0).await?;

        let versioned_component_ids: Vec<VersionedComponentId> = records
            .into_iter()
            .filter(|d| d.namespace == namespace.to_string())
            .map(|c| c.into())
            .collect();

        if !versioned_component_ids.is_empty() {
            for versioned_component_id in versioned_component_ids {
                self.object_store
                    .delete(&self.get_protected_object_store_key(&versioned_component_id))
                    .await
                    .map_err(|e| {
                        ComponentError::component_store_error("Failed to delete component", e)
                    })?;
                self.object_store
                    .delete(&self.get_user_object_store_key(&versioned_component_id))
                    .await
                    .map_err(|e| {
                        ComponentError::component_store_error("Failed to delete component", e)
                    })?;
            }
            self.component_repo
                .delete(namespace.to_string().as_str(), &component_id.0)
                .await?;
            Ok(())
        } else {
            Err(ComponentError::UnknownComponentId(component_id.clone()))
        }
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint: &ComponentConstraints<Namespace>,
    ) -> Result<ComponentConstraints<Namespace>, ComponentError> {
        info!(namespace = %component_constraint.namespace, "Create Component Constraint");
        let component_id = &component_constraint.component_id;
        let record = ComponentConstraintsRecord::try_from(component_constraint.clone())
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        self.component_repo
            .create_or_update_constraint(&record)
            .await?;
        let result = self
            .component_repo
            .get_constraint(&component_constraint.component_id)
            .await?
            .ok_or(ComponentError::ComponentConstraintCreateError(format!(
                "Failed to create constraints for {}",
                component_id
            )))?;

        let component_constraints = ComponentConstraints {
            namespace: component_constraint.namespace.clone(),
            component_id: component_id.clone(),
            constraints: result,
        };

        Ok(component_constraints)
    }

    async fn get_component_constraint(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<FunctionConstraintCollection>, ComponentError> {
        let result = self.component_repo.get_constraint(component_id).await?;
        Ok(result)
    }
}

impl ComponentServiceDefault {
    fn get_user_object_store_key(&self, id: &VersionedComponentId) -> String {
        format!("{id}:user")
    }

    fn get_protected_object_store_key(&self, id: &VersionedComponentId) -> String {
        format!("{id}:protected")
    }

    async fn upload_user_component(
        &self,
        user_component_id: &VersionedComponentId,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(&self.get_user_object_store_key(user_component_id), data)
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload user component", e)
            })
    }

    async fn upload_protected_component(
        &self,
        protected_component_id: &VersionedComponentId,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        self.object_store
            .put(
                &self.get_protected_object_store_key(protected_component_id),
                data,
            )
            .await
            .map_err(|e| {
                ComponentError::component_store_error("Failed to upload protected component", e)
            })
    }

    async fn upload_component_files(
        &self,
        files_properties: InitialComponentFilesArchiveAndPermissions,
    ) -> Result<Vec<InitialComponentFile>, ComponentError> {
        let files_file = files_properties.archive;
        let path_permissions: HashMap<InitialComponentFilePath, InitialComponentFilePermissions> = HashMap::from_iter(
            files_properties.permissions.values.iter().map(|f| (f.path.clone(), f.permissions)),
        );

        let mut buf_reader = BufReader::new(files_file);

        let mut zip_archive = ZipFileReader::with_tokio(&mut buf_reader).await.map_err(|e| {
            ComponentError::malformed_component_archive_from_error("Failed to unpack provided component files", e.into())
        })?;

        let mut uploaded: Vec<InitialComponentFile> = vec![];
        let mut hasher = Sha256::new();

        for i in 0..zip_archive.file().entries().len() {
            let (initial_component_file, content) = {
                let mut entry_reader = zip_archive.reader_with_entry(i).await.map_err(|e| {
                    ComponentError::malformed_component_archive_from_error("Failed to read entry from archive", e.into())
                })?;

                let entry = entry_reader.entry();

                let is_dir = entry.dir().map_err(|e| {
                    ComponentError::malformed_component_archive_from_error("Failed to check if entry is a directory", e.into())
                })?;

                if is_dir {
                    continue;
                }

                let file_path = initial_component_file_path_from_zip_entry(&entry)?;

                let mut buffer = Vec::new();
                entry_reader.read_to_end_checked(&mut buffer).await.map_err(|e| {
                    ComponentError::malformed_component_archive_from_error("Failed to read entry content", e.into())
                })?;

                hasher.update(&buffer);
                let hash = hex::encode(hasher.finalize_reset());

                // if permissions are not provided, default to read-only
                let initial_component_file = if let Some(permissions) = path_permissions.get(&file_path) {
                    InitialComponentFile {
                        key: InitialComponentFileKey(hash),
                        path: file_path,
                        permissions: permissions.clone(),
                    }
                } else {
                    InitialComponentFile {
                        key: InitialComponentFileKey(hash),
                        path: file_path,
                        permissions: InitialComponentFilePermissions::ReadOnly,
                    }
                };

                (initial_component_file, Bytes::from(buffer))
            };

            info!("Uploading file: {}", initial_component_file.path.to_string());

            self.initial_component_files_service
                .put_if_not_exists(&initial_component_file.key, content)
                .await
                .map_err(|e| {
                    ComponentError::initial_component_file_upload_error("Failed to upload component files", e)
                })?;

            uploaded.push(initial_component_file);
        }

        let uploaded_paths = uploaded.iter().map(|f| f.path.clone()).collect::<HashSet<_>>();
        for path in path_permissions.keys() {
            if !uploaded_paths.contains(path) {
                return Err(ComponentError::malformed_component_archive_from_message(format!("Didn't find expected file in the archive: {}", path.to_string())));
            }
        }

        Ok(uploaded)
    }

    async fn get_versioned_component_id<Namespace: Display + Clone>(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<Option<VersionedComponentId>, ComponentError> {
        let stored = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?;

        match stored {
            Some(stored) if stored.namespace == namespace.to_string() => {
                let stored_version = stored.version as u64;
                let requested_version = version.unwrap_or(stored_version);

                if requested_version <= stored_version {
                    Ok(Some(VersionedComponentId {
                        component_id: component_id.clone(),
                        version: requested_version,
                    }))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }
}

fn initial_component_file_path_from_zip_entry(entry: &ZipEntry) -> Result<InitialComponentFilePath, ComponentError> {
    let file_path = entry.filename().as_str().map_err(|e| {
        ComponentError::malformed_component_archive_from_message(format!("Failed to convert filename to string: {}", e))
    })?;

    // convert windows path separators to unix and sanitize the path
    let file_path: String = file_path
        .replace('\\', "/")
        .split('/')
        .map(sanitize_filename::sanitize)
        .collect::<Vec<_>>()
        .join("/");

    InitialComponentFilePath::from_str(&format!("/{file_path}")).map_err(|e| {
        ComponentError::malformed_component_archive_from_message(format!("Failed to convert path to InitialComponentFilePath: {}", e))
    })
}

mod internal {
    use golem_wasm_ast::analysis::AnalysedType;
    pub(crate) fn convert_to_pretty_types(analysed_types: &[AnalysedType]) -> String {
        let type_names = analysed_types
            .iter()
            .map(|x| {
                rib::TypeName::try_from(x.clone()).map_or("unknwon".to_string(), |x| x.to_string())
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
