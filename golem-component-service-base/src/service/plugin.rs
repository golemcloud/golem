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
    ComponentOwner, ComponentPluginInstallationTarget, PluginDefinition, PluginInstallation,
    PluginInstallationCreation, PluginInstallationUpdate, PluginScope,
};
use crate::repo::component::ComponentRepo;
use crate::repo::plugin::{PluginRecord, PluginRepo};
use crate::repo::plugin_installation::PluginInstallationRecord;
use crate::service::component::ComponentError;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::component::v1::component_error;
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
    #[error("Internal component error: {0}")]
    InternalComponentError(#[from] ComponentError),
    #[error("Component not found: {component_id}")]
    ComponentNotFound { component_id: ComponentId },
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
            Self::InternalComponentError(inner) => inner.to_safe_string(),
            Self::ComponentNotFound { .. } => self.to_string(),
        }
    }
}

impl From<PluginError> for golem_api_grpc::proto::golem::component::v1::ComponentError {
    fn from(value: PluginError) -> Self {
        match value {
            PluginError::InternalRepoError(_) => Self {
                error: Some(component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
            PluginError::InternalConversionError { .. } => Self {
                error: Some(component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
            PluginError::InternalComponentError(component_error) => component_error.into(),
            PluginError::ComponentNotFound { .. } => Self {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
        }
    }
}

#[async_trait]
pub trait PluginService<Owner: ComponentOwner, Scope: PluginScope> {
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
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, PluginError>;

    async fn update_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError>;

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), PluginError>;
}

pub struct PluginServiceDefault<Owner: ComponentOwner, Scope: PluginScope> {
    plugin_repo: Arc<dyn PluginRepo<Owner, Scope> + Send + Sync>,
    component_repo: Arc<dyn ComponentRepo<Owner> + Send + Sync>,
    plugin_scope_request_context: Scope::RequestContext
}

impl<Owner: ComponentOwner, Scope: PluginScope> PluginServiceDefault<Owner, Scope> {
    pub fn new(
        plugin_repo: Arc<dyn PluginRepo<Owner, Scope> + Send + Sync>,
        component_repo: Arc<dyn ComponentRepo<Owner> + Send + Sync>,
        plugin_scope_request_context: Scope::RequestContext
    ) -> Self {
        Self {
            plugin_repo,
            component_repo,
            plugin_scope_request_context
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
impl<Owner: ComponentOwner, Scope: PluginScope> PluginService<Owner, Scope>
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
            .accessible_scopes(&self.plugin_scope_request_context)
            .await
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
            .component_repo
            .get_installed_plugins(&owner_record, &component_id.0, component_version)
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
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, PluginError> {
        let owner: Owner::Row = owner.clone().into();

        let latest = self
            .component_repo
            .get_latest_version(&owner.to_string(), &component_id.0)
            .await?;

        if let Some(latest) = latest {
            let installation = installation.with_generated_id();
            let record = PluginInstallationRecord {
                installation_id: installation.id.0,
                plugin_name: installation.name.clone(),
                plugin_version: installation.version.clone(),
                priority: installation.priority,
                parameters: serde_json::to_vec(&installation.parameters).map_err(|e| {
                    PluginError::conversion_error("plugin installation parameters", e.to_string())
                })?,
                target: ComponentPluginInstallationTarget {
                    component_id: component_id.clone(),
                    component_version: latest.version as u64,
                }
                .into(),
                owner,
            };

            self.component_repo.install_plugin(&record).await?;

            Ok(installation)
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: component_id.clone(),
            })
        }
    }

    async fn update_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError> {
        let owner_record = owner.clone().into();

        let latest = self
            .component_repo
            .get_latest_version(&owner.to_string(), &component_id.0)
            .await?;

        if latest.is_some() {
            self.component_repo
                .update_plugin_installation(
                    &owner_record,
                    &component_id.0,
                    &installation_id.0,
                    update.priority,
                    serde_json::to_vec(&update.parameters).map_err(|e| {
                        PluginError::conversion_error(
                            "plugin installation parameters",
                            e.to_string(),
                        )
                    })?,
                )
                .await?;

            Ok(())
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: component_id.clone(),
            })
        }
    }

    async fn delete_plugin_installation_for_component(
        &self,
        owner: &Owner,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), PluginError> {
        let owner_record = owner.clone().into();
        let latest = self
            .component_repo
            .get_latest_version(&owner.to_string(), &component_id.0)
            .await?;

        if latest.is_some() {
            self.component_repo
                .uninstall_plugin(&owner_record, &component_id.0, &installation_id.0)
                .await?;

            Ok(())
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: component_id.clone(),
            })
        }
    }
}
