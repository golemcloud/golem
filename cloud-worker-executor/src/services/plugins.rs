use std::sync::Arc;

use crate::CloudGolemTypes;
use async_trait::async_trait;
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginDefinition, PluginInstallation,
};
use golem_common::model::PluginInstallationId;
use golem_common::model::{AccountId, ComponentId, ComponentVersion};
use golem_worker_executor_base::error::GolemError;
use golem_worker_executor_base::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor_base::{DefaultGolemTypes, GolemTypes};

pub struct CloudPluginsWrapper<T: GolemTypes> {
    inner_observations: Arc<dyn PluginsObservations>,
    inner_plugins: Arc<dyn Plugins<T>>,
}

impl<T: GolemTypes> CloudPluginsWrapper<T> {
    pub fn new(
        inner_observations: Arc<dyn PluginsObservations>,
        inner_plugins: Arc<dyn Plugins<T>>,
    ) -> Self {
        Self {
            inner_observations,
            inner_plugins,
        }
    }
}

#[async_trait]
impl<T: GolemTypes> PluginsObservations for CloudPluginsWrapper<T> {
    async fn observe_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<(), GolemError> {
        self.inner_observations
            .observe_plugin_installation(
                account_id,
                component_id,
                component_version,
                plugin_installation,
            )
            .await
    }
}

#[async_trait]
impl Plugins<CloudGolemTypes> for CloudPluginsWrapper<DefaultGolemTypes> {
    async fn get(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation_id: &PluginInstallationId,
    ) -> Result<
        (
            PluginInstallation,
            PluginDefinition<DefaultPluginOwner, DefaultPluginScope>,
        ),
        GolemError,
    > {
        self.inner_plugins
            .get(account_id, component_id, component_version, installation_id)
            .await
    }

    async fn get_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation_id: &PluginInstallationId,
    ) -> Result<PluginInstallation, GolemError> {
        self.inner_plugins
            .get_plugin_installation(account_id, component_id, component_version, installation_id)
            .await
    }

    async fn get_plugin_definition(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>, GolemError> {
        self.inner_plugins
            .get_plugin_definition(
                account_id,
                component_id,
                component_version,
                plugin_installation,
            )
            .await
    }
}
