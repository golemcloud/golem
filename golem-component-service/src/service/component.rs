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

use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_component_service_base::service::component_compilation::ComponentCompilationService;
use golem_component_service_base::service::component_processor::{
    process_component, ComponentProcessingError,
};
use tap::TapFallible;
use tracing::{error, info};

use crate::repo::component::ComponentRepo;
use crate::repo::RepoError;
use crate::service::component_object_store::ComponentObjectStore;
use golem_service_base::model::*;
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
pub trait ComponentService {
    async fn create(
        &self,
        component_name: &ComponentName,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError>;

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
    ) -> Result<Vec<u8>, ComponentError>;

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
    ) -> Result<ByteStream, ComponentError>;

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
    ) -> Result<Option<Vec<u8>>, ComponentError>;

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
    ) -> Result<Vec<Component>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
    ) -> Result<Option<Component>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Component>, ComponentError>;

    async fn get(&self, component_id: &ComponentId) -> Result<Vec<Component>, ComponentError>;
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
impl ComponentService for ComponentServiceDefault {
    async fn create(
        &self,
        component_name: &ComponentName,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError> {
        let tn = component_name.0.clone();
        info!("Creating component  with name {}", tn);

        self.check_new_name(component_name).await?;

        let metadata = process_component(&data)?;

        let component_id = ComponentId::new_v4();

        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };

        let user_component_id = UserComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };
        let protected_component_id = ProtectedComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };

        info!(
            "Uploaded component {} version 0 with exports {:?}",
            versioned_component_id.component_id, metadata.exports
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

        self.component_repo
            .upsert(&component.clone().into())
            .await?;

        self.component_compilation
            .enqueue_compilation(&component.versioned_component_id.component_id, 0)
            .await;

        Ok(component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
    ) -> Result<Component, ComponentError> {
        info!("Updating component {}", component_id);

        let metadata = process_component(&data)?;

        let next_component = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?
            .map(Component::from)
            .map(Component::next_version)
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        info!(
            "Uploaded component {} version {} with exports {:?}",
            component_id, next_component.versioned_component_id.version, metadata.exports
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

        self.component_repo
            .upsert(&component.clone().into())
            .await?;

        self.component_compilation
            .enqueue_compilation(component_id, component.versioned_component_id.version)
            .await;

        Ok(component)
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
    ) -> Result<Vec<u8>, ComponentError> {
        let versioned_component_id = {
            match version {
                Some(version) => VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                None => self
                    .component_repo
                    .get_latest_version(&component_id.0)
                    .await?
                    .map(Component::from)
                    .map(|t| t.versioned_component_id)
                    .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?,
            }
        };
        info!(
            "Downloading component {} version {}",
            component_id, versioned_component_id.version
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
    ) -> Result<ByteStream, ComponentError> {
        let versioned_component_id = {
            match version {
                Some(version) => VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                None => self
                    .component_repo
                    .get_latest_version(&component_id.0)
                    .await?
                    .map(Component::from)
                    .map(|t| t.versioned_component_id)
                    .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?,
            }
        };
        info!(
            "Downloading component {} version {}",
            component_id, versioned_component_id.version
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
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        info!(
            "Getting component {} version {} data",
            component_id,
            version.map_or("N/A".to_string(), |v| v.to_string())
        );

        let latest_component = self
            .component_repo
            .get_latest_version(&component_id.0)
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

    async fn find_by_name(
        &self,
        component_name: Option<ComponentName>,
    ) -> Result<Vec<Component>, ComponentError> {
        let tn = component_name.clone().map_or("N/A".to_string(), |n| n.0);
        info!("Getting component name {}", tn);

        let result = match component_name {
            Some(name) => self.component_repo.get_by_name(&name.0).await?,
            None => self.component_repo.get_all().await?,
        };

        Ok(result.into_iter().map(|t| t.into()).collect())
    }

    async fn get(&self, component_id: &ComponentId) -> Result<Vec<Component>, ComponentError> {
        info!("Getting component {}", component_id);
        let result = self.component_repo.get(&component_id.0).await?;

        Ok(result.into_iter().map(|t| t.into()).collect())
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
    ) -> Result<Option<Component>, ComponentError> {
        info!(
            "Getting component {} version {}",
            component_id.component_id, component_id.version
        );

        let result = self
            .component_repo
            .get_by_version(&component_id.component_id.0, component_id.version)
            .await?;
        Ok(result.map(|t| t.into()))
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
    ) -> Result<Option<Component>, ComponentError> {
        info!("Getting component {} latest version", component_id);
        let result = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?;
        Ok(result.map(|t| t.into()))
    }
}

impl ComponentServiceDefault {
    async fn check_new_name(&self, component_name: &ComponentName) -> Result<(), ComponentError> {
        let existing_components = self
            .component_repo
            .get_by_name(&component_name.0)
            .await
            .tap_err(|e| error!("Error getting existing components: {}", e))?;

        existing_components
            .into_iter()
            .next()
            .map(Component::from)
            .map_or(Ok(()), |t| {
                Err(ComponentError::AlreadyExists(
                    t.versioned_component_id.component_id,
                ))
            })
    }

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
        info!("Uploading user component: {:?}", user_component_id);

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
            "Uploading protected component: {:?}",
            protected_component_id
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
impl ComponentService for ComponentServiceNoop {
    async fn create(
        &self,
        _component_name: &ComponentName,
        _data: Vec<u8>,
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
                component_id: ComponentId::new_v4(),
                version: 0,
            },
            user_component_id: UserComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_component)
    }

    async fn update(
        &self,
        _component_id: &ComponentId,
        _data: Vec<u8>,
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
                component_id: ComponentId::new_v4(),
                version: 0,
            },
            user_component_id: UserComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
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
    ) -> Result<Vec<u8>, ComponentError> {
        Ok(vec![])
    }

    async fn download_stream(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
    ) -> Result<ByteStream, ComponentError> {
        Ok(ByteStream::empty())
    }

    async fn get_protected_data(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        Ok(None)
    }

    async fn find_by_name(
        &self,
        _component_name: Option<ComponentName>,
    ) -> Result<Vec<Component>, ComponentError> {
        Ok(vec![])
    }

    async fn get_by_version(
        &self,
        _component_id: &VersionedComponentId,
    ) -> Result<Option<Component>, ComponentError> {
        Ok(None)
    }

    async fn get_latest_version(
        &self,
        _component_id: &ComponentId,
    ) -> Result<Option<Component>, ComponentError> {
        Ok(None)
    }

    async fn get(&self, _component_id: &ComponentId) -> Result<Vec<Component>, ComponentError> {
        Ok(vec![])
    }
}
