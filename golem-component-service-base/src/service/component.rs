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

use std::fmt::{Debug, Display};
use std::io::Cursor;
use std::num::TryFromIntError;
use std::sync::Arc;
use anyhow::Error;
use crate::model::Component;
use crate::repo::component::ComponentRepo;
use crate::service::component_compilation::ComponentCompilationService;
use crate::service::component_processor::process_component;
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::component_metadata::ComponentProcessingError;
use golem_common::model::{ComponentId, ComponentType};
use golem_common::SafeDisplay;
use golem_service_base::model::{ComponentName, Configuration, VersionedComponentId};
use golem_service_base::repo::RepoError;
use golem_service_base::service::component_object_store::ComponentObjectStore;
use golem_service_base::stream::ByteStream;
use tap::TapFallible;
use tonic::include_file_descriptor_set;
use tracing::{error, info};
use golem_common::tracing::directive::default::info;
use zip::read::ZipArchive;
use tokio::fs;
use golem_service_base::service::ifs_object_store::IFSObjectStore;

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
    #[error("Initial file system storage error: {message}")]
    InitialFileSystemStorageError { message: String },
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
            ComponentError::InitialFileSystemStorageError { .. } => self.to_string(),
        }
    }
}

impl From<RepoError> for ComponentError {
    fn from(error: RepoError) -> Self {
        ComponentError::InternalRepoError(error)
    }
}

pub fn create_new_component<Namespace>(
    component_id: &ComponentId,
    component_name: &ComponentName,
    component_type: ComponentType,
    data: &[u8],
    namespace: &Namespace,
) -> Result<Component<Namespace>, ComponentProcessingError>
where
    Namespace: Eq + Clone + Send + Sync,
{
    let metadata = process_component(data)?;

    let versioned_component_id = VersionedComponentId {
        component_id: component_id.clone(),
        version: 0,
    };

    Ok(Component {
        namespace: namespace.clone(),
        component_name: component_name.clone(),
        component_size: data.len() as u64,
        metadata,
        created_at: Utc::now(),
        versioned_component_id,
        component_type,
    })
}

#[async_trait]
pub trait ComponentService<Namespace> {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        namespace: &Namespace,
        ifs_data: Vec<u8>,
        config: Configuration
    ) -> Result<Component<Namespace>, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        namespace: &Namespace,
        config: Configuration
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
    ) -> Result<ByteStream, ComponentError>;

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
}

pub struct ComponentServiceDefault {
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
    object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
    component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
    ifs_store: Arc<dyn IFSObjectStore + Sync + Send>
}

impl ComponentServiceDefault {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo + Sync + Send>,
        object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
        component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
        ifs_store: Arc<dyn IFSObjectStore + Sync + Send>,
    ) -> Self {
        ComponentServiceDefault {
            component_repo,
            object_store,
            component_compilation,
            ifs_store
        }
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
        namespace: &Namespace,
        ifs_data: Vec<u8>,
        config: Configuration
    ) -> Result<Component<Namespace>, ComponentError> {
        info!(namespace = %namespace, "Create component");

        self.find_id_by_name(component_name, namespace)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let component = create_new_component(
            component_id,
            component_name,
            component_type,
            &data,
            namespace,
        )?;

        info!(namespace = %namespace,"Uploaded component - exports {:?}",component.metadata.exports
        );
        tokio::try_join!(
            self.upload_user_component(&component.versioned_component_id, data.clone()),
            self.upload_protected_component(&component.versioned_component_id, data)

        )?;

        match self.save_ifs_zip(component_id, ifs_data.clone()).await {
            Ok(_) => {
                info!(
            "Successfully saved IFS zip for component: {}",
            component.versioned_component_id
        );
            }
            Err(e) => {
                // Log the error and handle it appropriately
                error!(
            "Failed to save IFS for component {}: {}",
            component.versioned_component_id, e
                );
                return Err(ComponentError::InitialFileSystemStorageError {
                    message: format!("Failed to decompress and save IFS: {}", e),
                });
            }
        }
        let record = component
            .clone()
            .try_into()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        let result = self.component_repo.create(&record).await;
        if let Err(RepoError::UniqueViolation(_)) = result {
            Err(ComponentError::AlreadyExists(component_id.clone()))?;
        }

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version, ifs_data, config)
            .await;

        Ok(component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        namespace: &Namespace,
        config: Configuration
    ) -> Result<Component<Namespace>, ComponentError> {
        info!(namespace = %namespace, "Update component");
        let created_at = Utc::now();
        let metadata =
            process_component(&data).map_err(ComponentError::ComponentProcessingError)?;

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

        tokio::try_join!(
            self.upload_user_component(&next_component.versioned_component_id, data.clone()),
            self.upload_protected_component(&next_component.versioned_component_id, data)
        )?;

        let component = Component {
            component_size,
            metadata,
            created_at,
            component_type: component_type.unwrap_or(next_component.component_type),
            ..next_component
        };
        let record = component
            .clone()
            .try_into()
            .map_err(|e| ComponentError::conversion_error("record", e))?;

        self.component_repo.create(&record).await?;

        let ifs_data = vec![];
        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version, ifs_data, config)
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
    ) -> Result<ByteStream, ComponentError> {
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
        info!("Uploading use content --------------------------------------------------");
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
        info!("Uploading protected component  ------------------------------------");
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


    async fn save_ifs_zip(
        &self,
        component_id: &ComponentId,
        ifs_data : Vec<u8>
    ) -> Result<(), ComponentError> {

        if ifs_data.is_empty(){
            return Err(ComponentError::InitialFileSystemStorageError {
                message: "Initial file system data is empty".to_string(),
            })
        };

        let object_key = format!("{}.zip",component_id);
        info!("(99999999999999999999999999999999999999999999999999)");
        self.ifs_store.put(
            &object_key,ifs_data
        ).await.map_err(|e| {
            ComponentError::InitialFileSystemStorageError {
                message: format!("Failed to upload IFS zip to object store: {}", e.to_string())
            }
        })?;

        info!("Saved IFS zip to object store");
        Ok(())


    }

    async fn decompress_and_save_ifs(
        &self,
        component_id: &ComponentId,  // Component ID for which we are saving IFS
        ifs_data: Vec<u8>,                    // The compressed IFS data
    ) -> Result<(), ComponentError> {

        // Check if the IFS data is empty
        if ifs_data.is_empty() {
            return Err(ComponentError::InitialFileSystemStorageError {
                message: "Initial file system data is empty".to_string(),
            });
        }

        // Create a cursor for in-memory IFS data (assumed to be in ZIP format)
        let cursor = Cursor::new(ifs_data);

        // Create a ZIP archive from the in-memory data
        let mut zip = ZipArchive::new(cursor)
            .map_err(|e| ComponentError::InitialFileSystemStorageError {
                message: format!("Failed to open zip archive: {}", e.to_string())
            })?;

        // Collect all the files and their content before any await call
        let mut extracted_files = Vec::new();

        // Iterate through the files in the ZIP archive
        for i in 0..zip.len() {
            let mut file  = zip.by_index(i).map_err(|e| {
                ComponentError::InitialFileSystemStorageError {
                    message: format!("Failed to read ZIP entry at index {}: {}", i, e),
                }
            })?;
            let file_name = file.name().to_string();

            info!("Processing file: {}", file_name);

            // Create a buffer to hold the file content
            let mut file_content = Vec::new();
            std::io::copy(&mut file, &mut file_content)
                .map_err(|e| ComponentError::InitialFileSystemStorageError {
                    message: format!("Failed to read file from zip: {}", e.to_string())
                })?;

            // Collect file name and content for later upload
            extracted_files.push((file_name, file_content));
        }

        // Now, asynchronously upload the files
        for (file_name, file_content) in extracted_files {
            // let file_name = file_name.trim_start_matches('/').to_string();
            let object_key = format!("ifs/{}/{}", component_id, file_name);

            // Upload the decompressed file to the object store
            self.object_store
                .put(&object_key, file_content)
                .await
                .map_err(|e| ComponentError::InitialFileSystemStorageError {
                    message: format!("Failed to upload file to object store: {}", e.to_string())
                })?;
        }

        // Log the success message
        info!("Successfully decompressed and saved IFS for component: {}", component_id);

        Ok(())
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
