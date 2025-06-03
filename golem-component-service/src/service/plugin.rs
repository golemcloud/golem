// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::service::component::CloudComponentService;
use crate::service::CloudComponentError;
use golem_common::model::auth::CloudAuthCtx;
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::plugin::{CloudPluginOwner, CloudPluginScope};
use golem_common::model::PluginId;
use golem_component_service_base::model::plugin::PluginDefinitionCreation;
use golem_component_service_base::service::plugin::PluginService;
use golem_service_base::clients::auth::BaseAuthService;
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
        let owner = self.get_owner(auth).await?;
        Ok(self.base_plugin_service.list_plugins(&owner).await?)
    }

    pub async fn list_plugins_for_scope(
        &self,
        auth: &CloudAuthCtx,
        scope: &CloudPluginScope,
    ) -> Result<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.get_owner(auth).await?;
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
        let owner = self.get_owner(auth).await?;
        Ok(self
            .base_plugin_service
            .list_plugin_versions(&owner, name)
            .await?)
    }

    pub async fn create_plugin(
        &self,
        auth: &CloudAuthCtx,
        definition: PluginDefinitionCreation<CloudPluginScope>,
    ) -> Result<(), CloudComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service
            .create_plugin(&owner, definition)
            .await?;
        Ok(())
    }

    pub async fn get(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.get_owner(auth).await?;
        Ok(self.base_plugin_service.get(&owner, name, version).await?)
    }

    pub async fn get_by_id(
        &self,
        auth: &CloudAuthCtx,
        id: &PluginId,
    ) -> Result<Option<PluginDefinition<CloudPluginOwner, CloudPluginScope>>, CloudComponentError>
    {
        let owner = self.get_owner(auth).await?;
        Ok(self.base_plugin_service.get_by_id(&owner, id).await?)
    }

    pub async fn delete(
        &self,
        auth: &CloudAuthCtx,
        name: &str,
        version: &str,
    ) -> Result<(), CloudComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service
            .delete(&owner, name, version)
            .await?;
        Ok(())
    }

    async fn get_owner(
        &self,
        auth: &CloudAuthCtx,
    ) -> Result<CloudPluginOwner, CloudComponentError> {
        let account_id = self.auth_service.get_account(auth).await?;
        Ok(CloudPluginOwner { account_id })
    }
}
