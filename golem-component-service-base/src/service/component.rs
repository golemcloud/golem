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
use std::sync::Arc;

use crate::service::component_compilation::ComponentCompilationService;
use crate::service::component_processor::{process_component, ComponentProcessingError};
use async_trait::async_trait;
use golem_common::model::ComponentId;
use tap::TapFallible;
use tracing::{error, info};

use crate::repo::component::{ComponentRecord, ComponentRepo};
use crate::repo::RepoError;
use golem_service_base::model::*;
use golem_service_base::service::component_object_store::ComponentObjectStore;
use golem_service_base::stream::ByteStream;

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
    #[error("Internal error: {0}")]
    Internal(anyhow::Error),
}

impl ComponentError {
    fn internal<E, C>(error: E, context: C) -> Self
    where
        E: Display + std::fmt::Debug + Send + Sync + 'static,
        C: Display + Send + Sync + 'static,
    {
        ComponentError::Internal(anyhow::Error::msg(error).context(context))
    }
}

impl From<RepoError> for ComponentError {
    fn from(error: RepoError) -> Self {
        ComponentError::Internal(anyhow::Error::msg(error.to_string()))
    }
}

#[async_trait]
pub trait ComponentService<Namespace> {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component, ComponentError>;

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
    ) -> Result<Vec<Component>, ComponentError>;

    async fn find_ids_by_name(
        &self,
        component_name: &ComponentName,
        namespace: &Namespace,
    ) -> Result<Vec<ComponentId>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component>, ComponentError>;

    async fn get(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Vec<Component>, ComponentError>;

    async fn get_namespace(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError>;
}

pub struct ComponentServiceDefault {
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
    object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
    component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
}

impl ComponentServiceDefault {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo + Sync + Send>,
        object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
        component_compilation: Arc<dyn ComponentCompilationService + Sync + Send>,
    ) -> Self {
        ComponentServiceDefault {
            component_repo,
            object_store,
            component_compilation,
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
        data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component, ComponentError> {
        info!(
            "Creating component - namespace: {}, id: {}, name: {}",
            namespace,
            component_id,
            component_name.0.clone()
        );

        self.find_ids_by_name(component_name, namespace)
            .await?
            .into_iter()
            .next()
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let metadata = process_component(&data)?;

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.clone(),
            version: 0,
        };

        let user_component_id = UserComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };
        let protected_component_id = ProtectedComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };

        info!(
            "Uploaded component - namespace: {}, id: {}, version: 0, exports {:?}",
            namespace, versioned_component_id.component_id, metadata.exports
        );

        let component_size: u64 = data
            .len()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert data length"))?;

        tokio::try_join!(
            self.upload_user_component(&user_component_id, data.clone()),
            self.upload_protected_component(&protected_component_id, data)
        )?;

        let component = Component {
            component_name: component_name.clone(),
            component_size,
            metadata,
            versioned_component_id,
            user_component_id,
            protected_component_id,
        };

        let record = ComponentRecord::new(namespace, component.clone())
            .map_err(|e| ComponentError::internal(e, "Failed to convert record"))?;

        self.component_repo.upsert(&record).await?;

        self.component_compilation
            .enqueue_compilation(&component.versioned_component_id.component_id, 0)
            .await;

        Ok(component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component, ComponentError> {
        info!(
            "Updating component - namespace: {}, id: {}",
            namespace, component_id
        );

        let metadata = process_component(&data)?;

        let next_component = self
            .component_repo
            .get_latest_version(namespace.to_string().as_str(), &component_id.0)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))
            .and_then(|c| {
                c.try_into()
                    .map_err(|e| ComponentError::internal(e, "Failed to convert record"))
            })
            .map(Component::next_version)?;

        info!(
            "Uploaded component - namespace: {}, id: {}, version: {}, exports {:?}",
            namespace,
            component_id,
            next_component.versioned_component_id.version,
            metadata.exports
        );

        let component_size: u64 = data
            .len()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert data length"))?;

        tokio::try_join!(
            self.upload_user_component(&next_component.user_component_id, data.clone()),
            self.upload_protected_component(&next_component.protected_component_id, data)
        )?;

        let component = Component {
            component_size,
            metadata,
            ..next_component
        };
        let record = ComponentRecord::new(namespace, component.clone())
            .map_err(|e| ComponentError::internal(e, "Failed to convert record"))?;

        self.component_repo.upsert(&record).await?;

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
        let versioned_component_id = {
            match version {
                Some(version) => VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                None => self
                    .component_repo
                    .get_latest_version(namespace.to_string().as_str(), &component_id.0)
                    .await?
                    .map(|c| c.into())
                    .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?,
            }
        };
        info!(
            "Downloading component - namespace: {}, id: {}, version: {}",
            namespace, component_id, versioned_component_id.version
        );

        let id = ProtectedComponentId {
            versioned_component_id,
        };

        self.object_store
            .get(&self.get_protected_object_store_key(&id))
            .await
            .tap_err(|e| error!("Error downloading component: {}", e))
            .map_err(|e| ComponentError::internal(e.to_string(), "Error downloading component"))
    }

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<ByteStream, ComponentError> {
        let versioned_component_id = {
            match version {
                Some(version) => VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                None => self
                    .component_repo
                    .get_latest_version(namespace.to_string().as_str(), &component_id.0)
                    .await?
                    .map(|c| c.into())
                    .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?,
            }
        };
        info!(
            "Downloading component - namespace: {}, id: {}, version: {}",
            namespace, component_id, versioned_component_id.version
        );

        let id = ProtectedComponentId {
            versioned_component_id,
        };

        let stream = self
            .object_store
            .get_stream(&self.get_protected_object_store_key(&id))
            .await;

        Ok(stream)
    }

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        info!(
            "Getting component data - namespace: {}, id: {}, version: {}",
            namespace,
            component_id,
            version.map_or("N/A".to_string(), |v| v.to_string())
        );

        let latest_component = self
            .component_repo
            .get_latest_version(namespace.to_string().as_str(), &component_id.0)
            .await?;

        let v = match latest_component {
            Some(component) => match version {
                Some(v) if v <= component.version as u64 => v,
                None => component.version as u64,
                _ => {
                    return Ok(None);
                }
            },
            None => {
                return Ok(None);
            }
        };

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.clone(),
            version: v,
        };

        let protected_id = ProtectedComponentId {
            versioned_component_id,
        };

        let object_key = self.get_protected_object_store_key(&protected_id);

        let result = self
            .object_store
            .get(&object_key)
            .await
            .tap_err(|e| error!("Error retrieving component: {}", e))
            .map_err(|e| ComponentError::internal(e.to_string(), "Error retrieving component"))?;

        Ok(Some(result))
    }

    async fn find_ids_by_name(
        &self,
        component_name: &ComponentName,
        namespace: &Namespace,
    ) -> Result<Vec<ComponentId>, ComponentError> {
        let records = self
            .component_repo
            .get_ids_by_name(namespace.to_string().as_str(), &component_name.0)
            .await?;
        Ok(records.into_iter().map(ComponentId).collect())
    }

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
        namespace: &Namespace,
    ) -> Result<Vec<Component>, ComponentError> {
        let cn = component_name.clone().map_or("N/A".to_string(), |n| n.0);
        info!(
            "Find component by name - namespace: {}, name: {}",
            namespace, cn
        );

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

        let values: Vec<Component> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<Component>, _>>()
            .map_err(|e| ComponentError::internal(e, "Failed to convert record".to_string()))?;

        Ok(values)
    }

    async fn get(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(
            "Getting component - namespace: {}, id: {}",
            namespace, component_id
        );
        let records = self
            .component_repo
            .get(namespace.to_string().as_str(), &component_id.0)
            .await?;

        let values: Vec<Component> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<Component>, _>>()
            .map_err(|e| ComponentError::internal(e, "Failed to convert record".to_string()))?;

        Ok(values)
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component>, ComponentError> {
        info!(
            "Getting component - namespace: {}, id: {}, version: {}",
            namespace, component_id.component_id, component_id.version
        );

        let result = self
            .component_repo
            .get_by_version(
                namespace.to_string().as_str(),
                &component_id.component_id.0,
                component_id.version,
            )
            .await?;

        match result {
            Some(c) => {
                let value = c.try_into().map_err(|e| {
                    ComponentError::internal(e, "Failed to convert record".to_string())
                })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        namespace: &Namespace,
    ) -> Result<Option<Component>, ComponentError> {
        info!(
            "Getting component - namespace: {}, id: {}, version: latest",
            namespace, component_id
        );
        let result = self
            .component_repo
            .get_latest_version(namespace.to_string().as_str(), &component_id.0)
            .await?;

        match result {
            Some(c) => {
                let value = c.try_into().map_err(|e| {
                    ComponentError::internal(e, "Failed to convert record".to_string())
                })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn get_namespace(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError> {
        info!("Getting component namespace - id: {}", component_id);
        let result = self.component_repo.get_namespaces(&component_id.0).await?;

        if result.is_empty() {
            Ok(None)
        } else if result.len() == 1 {
            let value = result[0].clone().0.try_into().map_err(|e| {
                ComponentError::internal(e, "Failed to convert namespace".to_string())
            })?;
            Ok(Some(value))
        } else {
            Err(ComponentError::internal(
                "",
                "Namespace is not unique".to_string(),
            ))
        }
    }
}

impl ComponentServiceDefault {
    fn get_user_object_store_key(&self, id: &UserComponentId) -> String {
        id.slug()
    }

    fn get_protected_object_store_key(&self, id: &ProtectedComponentId) -> String {
        id.slug()
    }

    async fn upload_user_component(
        &self,
        user_component_id: &UserComponentId,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        info!(
            "Uploading user component - id: {}",
            user_component_id.slug()
        );

        self.object_store
            .put(&self.get_user_object_store_key(user_component_id), data)
            .await
            .map_err(|e| ComponentError::internal(e.to_string(), "Failed to upload user component"))
    }

    async fn upload_protected_component(
        &self,
        protected_component_id: &ProtectedComponentId,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        info!(
            "Uploading protected component - id: {}",
            protected_component_id.slug()
        );

        self.object_store
            .put(
                &self.get_protected_object_store_key(protected_component_id),
                data,
            )
            .await
            .map_err(|e| {
                ComponentError::internal(e.to_string(), "Failed to upload protected component")
            })
    }
}

#[derive(Default)]
pub struct ComponentServiceNoop {}

#[async_trait]
impl<Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync> ComponentService<Namespace>
    for ComponentServiceNoop
{
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        _data: Vec<u8>,
        _namespace: &Namespace,
    ) -> Result<Component, ComponentError> {
        let fake_component = Component {
            component_name: component_name.clone(),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
                memories: vec![],
            },
            versioned_component_id: VersionedComponentId {
                component_id: component_id.clone(),
                version: 0,
            },
            user_component_id: UserComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: component_id.clone(),
                    version: 0,
                },
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: component_id.clone(),
                    version: 0,
                },
            },
        };

        Ok(fake_component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        _data: Vec<u8>,
        _namespace: &Namespace,
    ) -> Result<Component, ComponentError> {
        let fake_component = Component {
            component_name: ComponentName("fake".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
                memories: vec![],
            },
            versioned_component_id: VersionedComponentId {
                component_id: component_id.clone(),
                version: 0,
            },
            user_component_id: UserComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: component_id.clone(),
                    version: 0,
                },
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: component_id.clone(),
                    version: 0,
                },
            },
        };

        Ok(fake_component)
    }

    async fn download(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
        _namespace: &Namespace,
    ) -> Result<Vec<u8>, ComponentError> {
        Ok(vec![])
    }

    async fn download_stream(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
        _namespace: &Namespace,
    ) -> Result<ByteStream, ComponentError> {
        Ok(ByteStream::empty())
    }

    async fn find_ids_by_name(
        &self,
        _component_name: &ComponentName,
        _namespace: &Namespace,
    ) -> Result<Vec<ComponentId>, ComponentError> {
        Ok(vec![])
    }

    async fn get_protected_data(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
        _namespace: &Namespace,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        Ok(None)
    }

    async fn find_by_name(
        &self,
        _component_name: Option<ComponentName>,
        _namespace: &Namespace,
    ) -> Result<Vec<Component>, ComponentError> {
        Ok(vec![])
    }

    async fn get_by_version(
        &self,
        _component_id: &VersionedComponentId,
        _namespace: &Namespace,
    ) -> Result<Option<Component>, ComponentError> {
        Ok(None)
    }

    async fn get_latest_version(
        &self,
        _component_id: &ComponentId,
        _namespace: &Namespace,
    ) -> Result<Option<Component>, ComponentError> {
        Ok(None)
    }

    async fn get(
        &self,
        _component_id: &ComponentId,
        _namespace: &Namespace,
    ) -> Result<Vec<Component>, ComponentError> {
        Ok(vec![])
    }

    async fn get_namespace(
        &self,
        _component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError> {
        Ok(None)
    }
}
