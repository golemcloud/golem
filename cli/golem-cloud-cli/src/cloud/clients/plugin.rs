use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::{PluginDefinition, PluginDefinitionWithoutOwner};
use async_trait::async_trait;
use golem_cli::clients::plugin::PluginClient;
use golem_cli::cloud::ProjectId;
use golem_cli::model::GolemError;
use golem_cloud_client::CloudPluginScope;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PluginClientLive<C: golem_cloud_client::api::PluginClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::PluginClient + Sync + Send> PluginClient for PluginClientLive<C> {
    type ProjectContext = ProjectId;
    type PluginDefinition = PluginDefinition;
    type PluginDefinitionWithoutOwner = PluginDefinitionWithoutOwner;
    type PluginScope = CloudPluginScope;

    async fn list_plugins(
        &self,
        scope: Option<CloudPluginScope>,
    ) -> Result<Vec<Self::PluginDefinition>, GolemError> {
        info!("Getting registered plugins");

        let defs = self
            .client
            .list_plugins(scope.as_ref())
            .await
            .map_err(CloudGolemError::from)?;

        Ok(defs.into_iter().map(PluginDefinition).collect())
    }

    async fn get_plugin(
        &self,
        plugin_name: &str,
        plugin_version: &str,
    ) -> Result<Self::PluginDefinition, GolemError> {
        info!("Getting plugin {} version {}", plugin_name, plugin_version);

        Ok(PluginDefinition(
            self.client
                .get_plugin(plugin_name, plugin_version)
                .await
                .map_err(CloudGolemError::from)?,
        ))
    }

    async fn register_plugin(
        &self,
        definition: Self::PluginDefinitionWithoutOwner,
    ) -> Result<Self::PluginDefinition, GolemError> {
        info!("Registering plugin {}", definition.0.name);

        let _ = self
            .client
            .create_plugin(&definition.0)
            .await
            .map_err(CloudGolemError::from)?;
        let definition = self
            .client
            .get_plugin(&definition.0.name, &definition.0.version)
            .await
            .map_err(CloudGolemError::from)?;
        Ok(PluginDefinition(definition))
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
