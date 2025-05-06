use cloud_common::clients::plugin::{PluginError, PluginServiceClient};
use cloud_common::model::{CloudComponentOwner, TokenSecret};
use futures_util::{stream, StreamExt, TryStreamExt};
use golem_common::model::plugin::PluginInstallation;
use golem_component_service_base::api::dto;
use golem_component_service_base::model::Component;
use std::sync::Arc;

pub struct RemoteCloudApiMapper {
    plugin_service_client: Arc<dyn PluginServiceClient + Sync + Send>,
}

impl RemoteCloudApiMapper {
    pub fn new(plugin_service_client: Arc<dyn PluginServiceClient + Sync + Send>) -> Self {
        Self {
            plugin_service_client,
        }
    }

    // Note: cannot implement ApiMapper<CloudComponentOwner> because we need more than the owner
    // to chain the user's token into the plugin query. But this is not a problem at the moment,
    // because the ApiMapper trait is not used in any of the base implementations anyway.

    pub async fn convert_plugin_installation(
        &self,
        token: &TokenSecret,
        plugin_installation: PluginInstallation,
    ) -> Result<dto::PluginInstallation, PluginError> {
        let definition = self
            .plugin_service_client
            .get_by_id(&plugin_installation.plugin_id, token)
            .await?
            .expect("Plugin referenced by id not found");
        Ok(dto::PluginInstallation::from_model(
            plugin_installation,
            definition,
        ))
    }

    pub async fn convert_component(
        &self,
        token: &TokenSecret,
        component: Component<CloudComponentOwner>,
    ) -> Result<dto::Component, PluginError> {
        let installed_plugins = stream::iter(component.installed_plugins)
            .then(async |p| self.convert_plugin_installation(token, p).await)
            .try_collect::<Vec<_>>()
            .await?;

        Ok(dto::Component {
            versioned_component_id: component.versioned_component_id,
            component_name: component.component_name,
            component_size: component.component_size,
            metadata: component.metadata,
            created_at: component.created_at,
            component_type: component.component_type,
            files: component.files,
            installed_plugins,
            env: component.env,
        })
    }
}
