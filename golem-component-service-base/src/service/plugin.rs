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

use crate::model::{
    ComponentPluginInstallationTarget, PluginDefinition, PluginInstallation,
    PluginInstallationCreation, PluginInstallationUpdate, PluginOwner, PluginScope,
};
use crate::repo::plugin::{PluginRecord, PluginRepo};
use crate::repo::plugin_installation::{
    ComponentPluginInstallationRow, PluginInstallationRecord, PluginInstallationRepo,
};
use async_trait::async_trait;
use golem_common::model::{ComponentId, ComponentVersion, PluginInstallationId};
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use poem_openapi::__private::serde_json;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error("Internal error: failed to convert {what}: {error}")]
    InternalConversionError { what: String, error: String },
}

impl PluginError {
    pub fn conversion_error(what: impl AsRef<str>, error: String) -> Self {
        Self::InternalConversionError {
            what: what.as_ref().to_string(),
            error,
        }
    }
}

impl SafeDisplay for PluginError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalRepoError(inner) => inner.to_safe_string(),
            Self::InternalConversionError { .. } => self.to_string(),
        }
    }
}

#[async_trait]
pub trait PluginService<Owner: PluginOwner, Scope: PluginScope> {
    /// Get all the registered plugins owned by `owner`, regardless of their scope
    async fn list_plugins(
        &self,
        owner: &Owner,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError>;

    /// Gets the registered plugins owned by `owner` which are available in the given `scope`
    async fn list_plugins_for_scope(
        &self,
        owner: &Owner,
        scope: &Scope,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError>;

    /// Gets all the registered versions of a plugin owned by `owner` and having the name `name`
    async fn list_plugin_versions(
        &self,
        owner: &Owner,
        name: &str,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError>;

    /// Registers a new plugin
    async fn create_plugin(
        &self,
        plugin: PluginDefinition<Owner, Scope>,
    ) -> Result<(), PluginError>;

    /// Gets a registered plugin belonging to a given `owner`, identified by its `name` and `version`
    async fn get(
        &self,
        owner: &Owner,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<Owner, Scope>>, PluginError>;

    /// Deletes a registered plugin belonging to a given `owner`, identified by its `name` and `version`
    async fn delete(&self, owner: &Owner, name: &str, version: &str) -> Result<(), PluginError>;

    /// Gets the list of installed plugins for a given component version belonging to `owner`
    async fn get_plugin_installations_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, PluginError>;

    async fn create_plugin_installation_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, PluginError>;

    async fn update_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError>;

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(), PluginError>;
}

pub struct PluginServiceDefault<Owner: PluginOwner, Scope: PluginScope> {
    plugin_repo: Arc<dyn PluginRepo<Owner, Scope> + Send + Sync>,
    component_plugin_installations_repo:
        Arc<dyn PluginInstallationRepo<Owner, ComponentPluginInstallationTarget> + Send + Sync>,
}

impl<Owner: PluginOwner, Scope: PluginScope> PluginServiceDefault<Owner, Scope> {
    pub fn new(
        plugin_repo: Arc<dyn PluginRepo<Owner, Scope> + Send + Sync>,
        component_plugin_installations_repo: Arc<
            dyn PluginInstallationRepo<Owner, ComponentPluginInstallationTarget> + Send + Sync,
        >,
    ) -> Self {
        Self {
            plugin_repo,
            component_plugin_installations_repo,
        }
    }

    fn decode_plugin_definitions(
        records: Vec<PluginRecord<Owner, Scope>>,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError> {
        records
            .into_iter()
            .map(PluginDefinition::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::conversion_error("plugin", e))
    }
}

#[async_trait]
impl<Owner: PluginOwner, Scope: PluginScope> PluginService<Owner, Scope>
    for PluginServiceDefault<Owner, Scope>
{
    async fn list_plugins(
        &self,
        owner: &Owner,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError> {
        let owner_record: Owner::Row = owner.clone().into();
        let records = self.plugin_repo.get_all(&owner_record).await?;
        Self::decode_plugin_definitions(records)
    }

    async fn list_plugins_for_scope(
        &self,
        owner: &Owner,
        scope: &Scope,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError> {
        let owner_record: Owner::Row = owner.clone().into();

        let valid_scopes = scope
            .accessible_scopes()
            .into_iter()
            .map(|scope| scope.into())
            .collect::<Vec<Scope::Row>>();
        let records = self
            .plugin_repo
            .get_for_scope(&owner_record, &valid_scopes)
            .await?;
        Self::decode_plugin_definitions(records)
    }

    async fn list_plugin_versions(
        &self,
        owner: &Owner,
        name: &str,
    ) -> Result<Vec<PluginDefinition<Owner, Scope>>, PluginError> {
        let owner_record: Owner::Row = owner.clone().into();
        let records = self
            .plugin_repo
            .get_all_with_name(&owner_record, name)
            .await?;
        Self::decode_plugin_definitions(records)
    }

    async fn create_plugin(
        &self,
        plugin: PluginDefinition<Owner, Scope>,
    ) -> Result<(), PluginError> {
        let record: PluginRecord<Owner, Scope> = plugin.into();
        self.plugin_repo.create(&record).await?;
        Ok(())
    }

    async fn get(
        &self,
        owner: &Owner,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<Owner, Scope>>, PluginError> {
        let owner_record: Owner::Row = owner.clone().into();
        let record = self.plugin_repo.get(&owner_record, name, version).await?;
        record
            .map(PluginDefinition::try_from)
            .transpose()
            .map_err(|e| PluginError::conversion_error("plugin", e))
    }

    async fn delete(&self, owner: &Owner, name: &str, version: &str) -> Result<(), PluginError> {
        let owner_record: Owner::Row = owner.clone().into();

        self.component_plugin_installations_repo
            .delete_all_installation_of_plugin(&owner_record, name, version)
            .await?;

        self.plugin_repo
            .delete(&owner_record, name, version)
            .await?;
        Ok(())
    }

    async fn get_plugin_installations_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, PluginError> {
        let owner_record: Owner::Row = owner.clone().into();
        let records = self
            .component_plugin_installations_repo
            .get_all(
                &owner_record,
                &ComponentPluginInstallationRow {
                    component_id: component_id.0,
                    component_version: component_version as i64,
                },
            )
            .await?;
        records
            .into_iter()
            .map(PluginInstallation::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::conversion_error("plugin installation", e))
    }

    async fn create_plugin_installation_for_component(
        &self,
        owner: &Owner,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, PluginError> {
        let owner: Owner::Row = owner.clone().into();
        let target = ComponentPluginInstallationTarget {
            component_id: component_id.clone(),
            component_version,
        }
        .into();
        let installation = installation.with_generated_id();
        let record = PluginInstallationRecord {
            installation_id: installation.id.0,
            plugin_name: installation.name.clone(),
            plugin_version: installation.version.clone(),
            priority: installation.priority,
            parameters: serde_json::to_vec(&installation.parameters).map_err(|e| {
                PluginError::conversion_error("plugin installation parameters", e.to_string())
            })?,
            target,
            owner,
        };
        self.component_plugin_installations_repo
            .create(&record)
            .await?;
        Ok(installation)
    }

    async fn update_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError> {
        let owner = owner.clone().into();
        let target = ComponentPluginInstallationTarget {
            component_id: component_id.clone(),
            component_version,
        }
        .into();
        self.component_plugin_installations_repo
            .update(
                &owner,
                &target,
                &installation_id.0,
                update.priority,
                serde_json::to_vec(&update.parameters).map_err(|e| {
                    PluginError::conversion_error("plugin installation parameters", e.to_string())
                })?,
            )
            .await?;

        Ok(())
    }

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(), PluginError> {
        let owner = owner.clone().into();
        let target = ComponentPluginInstallationTarget {
            component_id: component_id.clone(),
            component_version,
        }
        .into();
        self.component_plugin_installations_repo
            .delete(&owner, &target, &installation_id.0)
            .await?;
        Ok(())
    }
}