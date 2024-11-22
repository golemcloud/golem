use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cli::clients::plugin::PluginClient;
use golem_cli::cloud::ProjectId;
use golem_cli::model::GolemError;
use golem_cloud_client::model::{
    PluginDefinitionCloudPluginOwnerCloudPluginScope, PluginDefinitionWithoutOwnerCloudPluginScope,
};
use golem_cloud_client::CloudPluginScope;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PluginClientLive<C: golem_cloud_client::api::PluginClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::PluginClient + Sync + Send> PluginClient for PluginClientLive<C> {
    type ProjectContext = ProjectId;
    type PluginDefinition = PluginDefinitionCloudPluginOwnerCloudPluginScope;
    type PluginDefinitionWithoutOwner = PluginDefinitionWithoutOwnerCloudPluginScope;
    type PluginScope = CloudPluginScope;

    async fn list_plugins(
        &self,
        scope: Option<CloudPluginScope>,
    ) -> Result<Vec<Self::PluginDefinition>, GolemError> {
        info!("Getting registered plugins");

        Ok(self
            .client
            .list_plugins(scope.as_ref())
            .await
            .map_err(CloudGolemError::from)?)
    }

    async fn get_plugin(
        &self,
        plugin_name: &str,
        plugin_version: &str,
    ) -> Result<Self::PluginDefinition, GolemError> {
        info!("Getting plugin {} version {}", plugin_name, plugin_version);

        Ok(self
            .client
            .get_plugin(plugin_name, plugin_version)
            .await
            .map_err(CloudGolemError::from)?)
    }

    async fn register_plugin(
        &self,
        definition: Self::PluginDefinitionWithoutOwner,
    ) -> Result<Self::PluginDefinition, GolemError> {
        info!("Registering plugin {}", definition.name);

        let _ = self
            .client
            .create_plugin(&definition)
            .await
            .map_err(CloudGolemError::from)?;
        let definition = self
            .client
            .get_plugin(&definition.name, &definition.version)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(definition)
    }

    async fn unregister_plugin(
        &self,
        plugin_name: &str,
        plugin_version: &str,
    ) -> Result<(), GolemError> {
        info!(
            "Unregistering plugin {} version {}",
            plugin_name, plugin_version
        );

        let _ = self
            .client
            .delete_plugin(plugin_name, plugin_version)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(())
    }
}
