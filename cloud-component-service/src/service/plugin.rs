use crate::model::CloudPluginScope;
use crate::service::component::CloudComponentService;
use crate::service::CloudComponentError;
use cloud_common::auth::CloudAuthCtx;
use cloud_common::clients::auth::BaseAuthService;
use cloud_common::model::{CloudPluginOwner, Role};
use golem_common::model::plugin::PluginDefinition;
use golem_component_service_base::api::dto;
use golem_component_service_base::service::plugin::PluginService;
use std::sync::Arc;

/// Wraps a `PluginService` implementation (defined in `golem-component-service-base`) so that each
/// operation receives a `CloudAuthCtx` and gets authorized
pub struct CloudPluginService {
    base_plugin_service: Arc<dyn PluginService<CloudPluginOwner, CloudPluginScope> + Sync + Send>,
    cloud_component_service: Arc<CloudComponentService>,
    auth_service: Arc<dyn BaseAuthService + Sync + Send>,
}

impl CloudPluginService {
    pub fn new(
        base_plugin_service: Arc<
            dyn PluginService<CloudPluginOwner, CloudPluginScope> + Sync + Send,
        >,
        cloud_component_service: Arc<CloudComponentService>,
        auth_service: Arc<dyn BaseAuthService + Sync + Send>,
    ) -> Self {
        Self {
            base_plugin_service,
            cloud_component_service,
            auth_service,
        }
    }

    pub async fn list_plugins(
        &self,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self.base_plugin_service.list_plugins(&owner).await?)
    }

    pub async fn list_plugins_for_scope(
        &self,
        auth: &CloudAuthCtx,
        scope: &CloudPluginScope,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self
            .base_plugin_service
            .list_plugins_for_scope(
                &owner,
                scope,
                (self.cloud_component_service.clone(), auth.clone()),
            )
            .await?)
    }

    pub async fn list_plugin_versions(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self
            .base_plugin_service
            .list_plugin_versions(&owner, name)
            .await?)
    }

    pub async fn create_plugin(
        &self,
        auth: &CloudAuthCtx,
        definition: dto::PluginDefinitionCreation<CloudPluginScope>,
    ) -> Result<(), CloudComponentError> {
        let owner = self.authorize(auth, Role::CreatePlugin).await?;
        let definition = definition.with_owner(owner.clone());
        self.base_plugin_service.create_plugin(definition).await?;
        Ok(())
    }

    pub async fn get(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.authorize(auth, Role::ViewPlugin).await?;
        Ok(self.base_plugin_service.get(&owner, name, version).await?)
    }

    pub async fn delete(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
        version: &str,
    ) -> Result<(), CloudComponentError> {
        let owner = self.authorize(auth, Role::DeletePlugin).await?;
        self.base_plugin_service
            .delete(&owner, name, version)
            .await?;
        Ok(())
    }

    async fn authorize(
        &self,
        auth: &CloudAuthCtx,
        role: Role,
    ) -> Result<CloudPluginOwner, CloudComponentError> {
        let account_id = self.auth_service.authorize_role(role, auth).await?;
        Ok(CloudPluginOwner { account_id })
    }
}
