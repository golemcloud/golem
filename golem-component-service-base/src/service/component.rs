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
use crate::service::component_processor::process_component;
use async_trait::async_trait;
use golem_common::component_metadata::ComponentProcessingError;
use golem_common::model::ComponentId;
use tap::TapFallible;
use tracing::{error, info};

use crate::model::Component;
use crate::repo::component::ComponentRepo;
use crate::repo::RepoError;
use golem_service_base::model::{
    ComponentMetadata, ComponentName, ProtectedComponentId, UserComponentId, VersionedComponentId,
};
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
        E: Display + Debug + Send + Sync + 'static,
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

pub fn create_new_component<Namespace>(
    component_id: &ComponentId,
    component_name: &ComponentName,
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

    let user_component_id = UserComponentId {
        versioned_component_id: versioned_component_id.clone(),
    };
    let protected_component_id = ProtectedComponentId {
        versioned_component_id: versioned_component_id.clone(),
    };

    Ok(Component {
        namespace: namespace.clone(),
        component_name: component_name.clone(),
        component_size: data.len() as u64,
        metadata,
        versioned_component_id,
        user_component_id,
        protected_component_id,
    })
}

#[async_trait]
pub trait ComponentService<Namespace> {
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
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
    ) -> Result<Component<Namespace>, ComponentError> {
        info!(namespace = %namespace, "Create component");

        self.find_id_by_name(component_name, namespace)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let component = create_new_component(component_id, component_name, &data, namespace)?;

        info!(namespace = %namespace,"Uploaded component - exports {:?}",component.metadata.exports
        );
        tokio::try_join!(
            self.upload_user_component(&component.user_component_id, data.clone()),
            self.upload_protected_component(&component.protected_component_id, data)
        )?;

        let record = component
            .clone()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert record"))?;

        self.component_repo.create(&record).await?;

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version)
            .await;

        Ok(component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError> {
        info!(namespace = %namespace, "Update component");

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
                    .map_err(|e| ComponentError::internal(e, "Failed to convert record"))
            })
            .map(Component::next_version)?;

        info!(namespace = %namespace, "Uploaded component - exports {:?}", metadata.exports);

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
        let record = component
            .clone()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert record"))?;

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

        let id = ProtectedComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };

        self.object_store
            .get(&self.get_protected_object_store_key(&id))
            .await
            .tap_err(
                |e| error!(namespace = %namespace, "Error downloading component - error: {}", e),
            )
            .map_err(|e| ComponentError::internal(e.to_string(), "Error downloading component"))
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
        info!(namespace = %namespace, "Get component protected data");

        let versioned_component_id = self
            .get_versioned_component_id(component_id, version, namespace)
            .await?;

        match versioned_component_id {
            Some(versioned_component_id) => {
                let id = ProtectedComponentId {
                    versioned_component_id: versioned_component_id.clone(),
                };
                let data = self
                    .object_store
                    .get(&self.get_protected_object_store_key(&id))
                    .await
                    .tap_err(|e| error!(namespace = %namespace, "Error getting component data - error: {}", e))
                    .map_err(|e| {
                        ComponentError::internal(e.to_string(), "Error retrieving component")
                    })?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
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
            .map_err(|e| ComponentError::internal(e, "Failed to convert record".to_string()))?;

        Ok(values)
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
            .map_err(|e| ComponentError::internal(e, "Failed to convert record".to_string()))?;

        Ok(values)
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
                let value = c.try_into().map_err(|e| {
                    ComponentError::internal(e, "Failed to convert record".to_string())
                })?;
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
                let value = c.try_into().map_err(|e| {
                    ComponentError::internal(e, "Failed to convert record".to_string())
                })?;
                Ok(Some(value))
            }
            _ => Ok(None),
        }
    }

    async fn get_namespace(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError> {
        info!("Get component namespace");
        let result = self.component_repo.get_namespace(&component_id.0).await?;
        if let Some(result) = result {
            let value = result.clone().try_into().map_err(|e| {
                ComponentError::internal(e, "Failed to convert namespace".to_string())
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
                    .delete(&self.get_protected_object_store_key(&ProtectedComponentId {
                        versioned_component_id: versioned_component_id.clone(),
                    }))
                    .await
                    .map_err(|e| {
                        ComponentError::internal(e.to_string(), "Failed to delete component")
                    })?;
                self.object_store
                    .delete(&self.get_user_object_store_key(&UserComponentId {
                        versioned_component_id: versioned_component_id.clone(),
                    }))
                    .await
                    .map_err(|e| {
                        ComponentError::internal(e.to_string(), "Failed to delete component")
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

#[derive(Default)]
pub struct ComponentServiceNoop {}

#[async_trait]
impl<Namespace: Display + Eq + Clone + Send + Sync> ComponentService<Namespace>
    for ComponentServiceNoop
{
    async fn create(
        &self,
        component_id: &ComponentId,
        component_name: &ComponentName,
        _data: Vec<u8>,
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError> {
        let fake_component = Component {
            namespace: namespace.clone(),
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
        namespace: &Namespace,
    ) -> Result<Component<Namespace>, ComponentError> {
        let fake_component = Component {
            namespace: namespace.clone(),
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

    async fn find_id_by_name(
        &self,
        _component_name: &ComponentName,
        _namespace: &Namespace,
    ) -> Result<Option<ComponentId>, ComponentError> {
        Ok(None)
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
    ) -> Result<Vec<Component<Namespace>>, ComponentError> {
        Ok(vec![])
    }

    async fn get_by_version(
        &self,
        _component_id: &VersionedComponentId,
        _namespace: &Namespace,
    ) -> Result<Option<Component<Namespace>>, ComponentError> {
        Ok(None)
    }

    async fn get_latest_version(
        &self,
        _component_id: &ComponentId,
        _namespace: &Namespace,
    ) -> Result<Option<Component<Namespace>>, ComponentError> {
        Ok(None)
    }

    async fn get(
        &self,
        _component_id: &ComponentId,
        _namespace: &Namespace,
    ) -> Result<Vec<Component<Namespace>>, ComponentError> {
        Ok(vec![])
    }

    async fn get_namespace(
        &self,
        _component_id: &ComponentId,
    ) -> Result<Option<Namespace>, ComponentError> {
        Ok(None)
    }

    async fn delete(
        &self,
        _component_id: &ComponentId,
        _namespace: &Namespace,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}
