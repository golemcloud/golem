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
use crate::service::component::ComponentService;
use crate::service::plugin::PluginService;
use golem_common::model::auth::{AccountAction, AuthCtx, ProjectAction};
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::plugin::{PluginOwner, PluginScope};
use golem_common::model::{AccountId, PluginId};
use golem_service_base::clients::auth::AuthService;
use std::sync::Arc;

pub struct AuthedPluginService {
    plugin_service: Arc<PluginService>,
    auth_service: Arc<AuthService>,
    component_service: Arc<dyn ComponentService>,
}

impl AuthedPluginService {
    pub fn new(
        base_plugin_service: Arc<PluginService>,
        auth_service: Arc<AuthService>,
        component_service: Arc<dyn ComponentService>,
    ) -> Self {
        Self {
            plugin_service: base_plugin_service,
            auth_service,
            component_service,
        }
    }

    pub async fn list_plugins_for_scope(
        &self,
        auth: &AuthCtx,
        scope: &PluginScope,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let (owner, valid_scopes) = self.accessible_scopes(scope, auth).await?;

        self.plugin_service
            .list_plugins_for_scopes(owner, valid_scopes)
            .await
    }

    pub async fn create_plugin(
        &self,
        auth: &AuthCtx,
        definition: PluginDefinitionCreation,
    ) -> Result<PluginDefinition, ComponentError> {
        let result = match &definition.scope {
            PluginScope::Global(_) => {
                // Global plugins are always owned by the user creating them.
                let owner = self.get_owner(auth).await?;
                self.auth_service
                    .authorize_account_action(
                        &owner.account_id,
                        AccountAction::CreateGlobalPlugin,
                        auth,
                    )
                    .await?;
                self.plugin_service
                    .create_plugin(&owner, definition)
                    .await?
            }
            PluginScope::Project(inner) => {
                // Project scoped plugins are owned by the user owning the project.
                let project = self
                    .auth_service
                    .authorize_project_action(
                        &inner.project_id,
                        ProjectAction::CreatePluginDefinition,
                        auth,
                    )
                    .await?;
                let owner = PluginOwner {
                    account_id: project.account_id,
                };
                self.plugin_service
                    .create_plugin(&owner, definition)
                    .await?
            }
            PluginScope::Component(inner) => {
                // Component scoped plugins are owned by the user owning the component
                let component_owner = self
                    .component_service
                    .get_owner(&inner.component_id)
                    .await?
                    .ok_or(ComponentError::UnknownComponentId(
                        inner.component_id.clone(),
                    ))?;

                self.auth_service
                    .authorize_project_action(
                        &component_owner.project_id,
                        ProjectAction::CreatePluginDefinition,
                        auth,
                    )
                    .await?;

                let owner = PluginOwner {
                    account_id: component_owner.account_id,
                };

                self.plugin_service
                    .create_plugin(&owner, definition)
                    .await?
            }
        };
        Ok(result)
    }

    pub async fn get(
        &self,
        auth: &AuthCtx,
        owner_account: AccountId,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition>, ComponentError> {
        let owner = PluginOwner {
            account_id: owner_account,
        };
        let plugin = self.plugin_service.get(&owner, name, version).await?;
        if let Some(plugin) = &plugin {
            self.check_plugin_access(
                plugin,
                ProjectAction::ViewPluginDefinition,
                AccountAction::ViewGlobalPlugins,
                auth,
            )
            .await?;
        };
        Ok(plugin)
    }

    pub async fn get_by_id(
        &self,
        auth: &AuthCtx,
        id: &PluginId,
    ) -> Result<Option<PluginDefinition>, ComponentError> {
        let plugin = self.plugin_service.get_by_id(id).await?;
        if let Some(plugin) = &plugin {
            self.check_plugin_access(
                plugin,
                ProjectAction::ViewPluginDefinition,
                AccountAction::ViewGlobalPlugins,
                auth,
            )
            .await?;
        };
        Ok(plugin)
    }

    pub async fn delete(
        &self,
        auth: &AuthCtx,
        owner_account: AccountId,
        name: &str,
        version: &str,
    ) -> Result<(), ComponentError> {
        let owner = PluginOwner {
            account_id: owner_account.clone(),
        };
        let plugin = self.plugin_service.get(&owner, name, version).await?;
        if let Some(plugin) = &plugin {
            self.check_plugin_access(
                plugin,
                ProjectAction::DeletePluginDefinition,
                AccountAction::DeleteGlobalPlugin,
                auth,
            )
            .await?;
        } else {
            Err(ComponentError::PluginNotFound {
                account_id: owner_account,
                plugin_name: name.to_string(),
                plugin_version: version.to_string(),
            })?
        };

        self.plugin_service.delete(&owner, name, version).await?;
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
    ) -> Result<(PluginOwner, Vec<PluginScope>), ComponentError> {
        match scope {
            PluginScope::Global(_) =>
            // In global scope we only have access to our own plugins in global scope
            {
                let account_id = self.auth_service.get_account(auth_ctx).await?;
                let owner = PluginOwner { account_id };
                Ok((owner, vec![scope.clone()]))
            }
            PluginScope::Project(inner) =>
            // In a project scope we have access to plugins in that particular scope, and all the global ones of the owning account
            {
                let project_namespace = self
                    .auth_service
                    .authorize_project_action(
                        &inner.project_id,
                        ProjectAction::ViewPluginDefinition,
                        auth_ctx,
                    )
                    .await?;
                let owner = PluginOwner {
                    account_id: project_namespace.account_id,
                };

                Ok((owner, vec![PluginScope::global(), scope.clone()]))
            }
            PluginScope::Component(inner) =>
            // In a component scope we have access to
            // - plugins in that particular scope
            // - plugins of the component's owner project
            // - and all the global ones in the owning account
            {
                let component_owner = self
                    .component_service
                    .get_owner(&inner.component_id)
                    .await?
                    .ok_or(ComponentError::UnknownComponentId(
                        inner.component_id.clone(),
                    ))?;

                self.auth_service
                    .authorize_project_action(
                        &component_owner.project_id,
                        ProjectAction::ViewPluginDefinition,
                        auth_ctx,
                    )
                    .await?;

                let owner = PluginOwner {
                    account_id: component_owner.account_id,
                };

                let scopes = vec![
                    PluginScope::global(),
                    PluginScope::project(component_owner.project_id),
                    scope.clone(),
                ];

                Ok((owner, scopes))
            }
        }
    }

    async fn check_plugin_access(
        &self,
        plugin: &PluginDefinition,
        project_action: ProjectAction,
        account_action: AccountAction,
        auth: &AuthCtx,
    ) -> Result<(), ComponentError> {
        match &plugin.scope {
            PluginScope::Component(inner) => {
                let component = self
                    .component_service
                    .get_owner(&inner.component_id)
                    .await?
                    .ok_or(ComponentError::UnknownComponentId(
                        inner.component_id.clone(),
                    ))?;

                self.auth_service
                    .authorize_project_action(&component.project_id, project_action, auth)
                    .await?;
            }
            PluginScope::Project(inner) => {
                self.auth_service
                    .authorize_project_action(&inner.project_id, project_action, auth)
                    .await?;
            }
            PluginScope::Global(_) => {
                self.auth_service
                    .authorize_account_action(&plugin.owner.account_id, account_action, auth)
                    .await?;
            }
        };

        Ok(())
    }
}
