use std::fmt::{Debug, Display};
use std::sync::Arc;
use async_trait::async_trait;
use tracing::info;
use golem_common::model::ComponentId;
use golem_common::tracing::directive::default::info;
use golem_service_base::model::VersionedComponentId;
use golem_service_base::service::ifs_object_store::IFSObjectStore;
use golem_service_base::stream::ByteStream;
use crate::repo::component::ComponentRepo;
use crate::service::component::ComponentError;

#[async_trait]
pub trait InitialFileSystemService<Namespace>{

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace,
    ) -> Result<ByteStream, ComponentError>;
}

pub struct InitialFileSystemServiceDefault {
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
    object_store: Arc<dyn IFSObjectStore + Sync + Send>,
}

impl InitialFileSystemServiceDefault {

    pub fn new(
        component_repo: Arc<dyn ComponentRepo + Sync + Send>,
        object_store: Arc<dyn IFSObjectStore + Sync + Send>,
    ) -> Self {

        InitialFileSystemServiceDefault {
            component_repo,
            object_store,
        }

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
    fn get_protected_object_store_key(&self, id: &VersionedComponentId) -> String {
        format!("{id}:protected")
    }
}

#[async_trait]
impl <Namespace> InitialFileSystemService<Namespace> for InitialFileSystemServiceDefault
where
    Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
    <Namespace as TryFrom<String>>::Error: Display + Debug + Send + Sync + 'static,
{
    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        namespace: &Namespace
    ) -> Result<ByteStream, ComponentError> {
        let version_component_id = self
            .get_versioned_component_id(component_id, version, namespace)
            .await?
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        info!(namespace = %namespace, "Download component as stream");

        let stream = self
            .object_store
            .get_stream(&format!("{}.zip", component_id))
            .await;

        Ok(stream)


    }
}