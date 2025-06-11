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

use crate::error::ComponentError;
use crate::model::plugin::PluginDefinitionCreation;
use crate::repo::plugin::PluginRepo;
use crate::service::component::ComponentService;
use crate::service::plugin::PluginService;
use golem_common::model::auth::{AuthCtx, ProjectAction};
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::plugin::{PluginOwner, PluginScope};
use golem_common::model::PluginId;
use golem_service_base::clients::auth::BaseAuthService;
use std::sync::Arc;

pub struct AuthedPluginService {
    base_plugin_service: Arc<PluginService>,
    auth_service: Arc<dyn BaseAuthService>,
    component_service: Arc<dyn ComponentService>,
}

impl AuthedPluginService {
    pub fn new(
        base_plugin_service: Arc<PluginService>,
        auth_service: Arc<dyn BaseAuthService>,
        component_service: Arc<dyn ComponentService>,
    ) -> Self {
        Self {
            base_plugin_service,
            auth_service,
            component_service,
        }
    }

    pub async fn list_plugins(
        &self,
        auth: &AuthCtx,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service.list_plugins(&owner).await
    }

    pub async fn list_plugins_for_scope(
        &self,
        auth: &AuthCtx,
        scope: &PluginScope,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let owner = self.get_owner(auth).await?;

        let valid_scopes = self.accessible_scopes(scope, auth).await?;

        self.base_plugin_service
            .list_plugins_for_scopes(&owner, valid_scopes)
            .await
    }

    pub async fn list_plugin_versions(
        &self,
        auth: &AuthCtx,
        name: &str,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service
            .list_plugin_versions(&owner, name)
            .await
    }

    pub async fn create_plugin(
        &self,
        auth: &AuthCtx,
        definition: PluginDefinitionCreation,
    ) -> Result<(), ComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service
            .create_plugin(&owner, definition)
            .await?;
        Ok(())
    }

    pub async fn get(
        &self,
        auth: &AuthCtx,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition>, ComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service.get(&owner, name, version).await
    }

    pub async fn get_by_id(
        &self,
        auth: &AuthCtx,
        id: &PluginId,
    ) -> Result<Option<PluginDefinition>, ComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service.get_by_id(&owner, id).await
    }

    pub async fn delete(
        &self,
        auth: &AuthCtx,
        name: &str,
        version: &str,
    ) -> Result<(), ComponentError> {
        let owner = self.get_owner(auth).await?;
        self.base_plugin_service
            .delete(&owner, name, version)
            .await?;
        Ok(())
    }

    async fn get_owner(&self, auth: &AuthCtx) -> Result<PluginOwner, ComponentError> {
        let account_id = self.auth_service.get_account(auth).await?;
        Ok(PluginOwner { account_id })
    }

    async fn accessible_scopes(
        &self,
        scope: &PluginScope,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<PluginScope>, ComponentError> {
        match scope {
            PluginScope::Global(_) =>
            // In global scope we only have access to plugins in global scope
            {
                Ok(vec![scope.clone()])
            }
            PluginScope::Project(inner) =>
            // In a project scope we have access to plugins in that particular scope, and all the global ones
            {
                self.auth_service
                    .authorize_project_action(
                        &inner.project_id,
                        ProjectAction::ViewProject,
                        auth_ctx,
                    )
                    .await?;

                Ok(vec![PluginScope::global(), scope.clone()])
            }
            PluginScope::Component(inner) =>
            // In a component scope we have access to
            // - plugins in that particular scope
            // - plugins of the component's owner project
            // - and all the global ones
            {
                let owner = self
                    .component_service
                    .get_owner(&inner.component_id)
                    .await?
                    .ok_or(ComponentError::UnknownComponentId(
                        inner.component_id.clone(),
                    ))?;

                self.auth_service
                    .authorize_project_action(
                        &owner.project_id,
                        ProjectAction::ViewComponent,
                        auth_ctx,
                    )
                    .await?;

                Ok(vec![
                    PluginScope::global(),
                    PluginScope::project(owner.project_id),
                    scope.clone(),
                ])
            }
        }
    }
}
