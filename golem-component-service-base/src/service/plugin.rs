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

use super::component::ComponentService;
use crate::model::plugin::{
    PluginDefinitionCreation, PluginTypeSpecificCreation, PluginWasmFileReference,
};
use crate::repo::plugin::{PluginRecord, PluginRepo};
use crate::service::component::ComponentError;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::component_error;
use golem_common::model::component::{ComponentOwner, VersionedComponentId};
use golem_common::model::plugin::{
    AppPluginDefinition, LibraryPluginDefinition, OplogProcessorDefinition, PluginDefinition,
    PluginOwner, PluginScope, PluginTypeSpecificDefinition, PluginWasmFileKey,
};
use golem_common::model::{ComponentId, PluginId};
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_wasm_ast::analysis::AnalysedExport;
use std::fmt::Debug;
use std::sync::Arc;

const OPLOG_PROCESSOR_INTERFACE: &str = "golem:api/oplog-processor";
const OPLOG_PROCESSOR_VERSION_PREFIX: &str = "1.";

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
    #[error("Failed to get available scopes: {error}")]
    FailedToGetAvailableScopes { error: String },
    #[error("Blob Storage error: {0}")]
    BlobStorageError(String),
    #[error("Plugin not found: {plugin_name}@{plugin_version}")]
    PluginNotFound {
        plugin_name: String,
        plugin_version: String,
    },
    #[error("Plugin {plugin_name}@{plugin_version} {details}")]
    InvalidScope {
        plugin_name: String,
        plugin_version: String,
        details: String,
    },
    #[error("Plugin does not implement golem:api/oplog-processor")]
    InvalidOplogProcessorPlugin,
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
            Self::FailedToGetAvailableScopes { .. } => self.to_string(),
            Self::PluginNotFound { .. } => self.to_string(),
            Self::InvalidScope { .. } => self.to_string(),
            Self::BlobStorageError(_) => self.to_string(),
            Self::InvalidOplogProcessorPlugin => self.to_string(),
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
            PluginError::FailedToGetAvailableScopes { .. } => Self {
                error: Some(component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
            PluginError::PluginNotFound { .. } => Self {
                error: Some(component_error::Error::NotFound(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
            PluginError::InvalidScope { .. } => Self {
                error: Some(component_error::Error::Unauthorized(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
            PluginError::BlobStorageError { .. } => Self {
                error: Some(component_error::Error::InternalError(ErrorBody {
                    error: value.to_safe_string(),
                })),
            },
            PluginError::InvalidOplogProcessorPlugin => Self {
                error: Some(component_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                })),
            },
        }
    }
}

#[async_trait]
pub trait PluginService<Owner: PluginOwner, Scope: PluginScope>: Debug + Send + Sync {
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
        request_context: Scope::RequestContext,
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
        owner: &Owner,
        plugin: PluginDefinitionCreation<Scope>,
    ) -> Result<PluginDefinition<Owner, Scope>, PluginError>;

    /// Gets a registered plugin belonging to a given `owner`, identified by its `name` and `version`
    async fn get(
        &self,
        owner: &Owner,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<Owner, Scope>>, PluginError>;

    /// Get a plugin by id for a given owner. Returns the plugin even if it was unregistered / deleted.
    async fn get_by_id(
        &self,
        owner: &Owner,
        id: &PluginId,
    ) -> Result<Option<PluginDefinition<Owner, Scope>>, PluginError>;

    /// Deletes a registered plugin belonging to a given `owner`, identified by its `name` and `version`
    async fn delete(&self, owner: &Owner, name: &str, version: &str) -> Result<(), PluginError>;
}

#[derive(Debug)]
pub struct PluginServiceDefault<Owner: ComponentOwner, Scope: PluginScope> {
    plugin_repo: Arc<dyn PluginRepo<Owner::PluginOwner, Scope>>,
    plugin_wasm_files: Arc<PluginWasmFilesService>,
    component_service: Arc<dyn ComponentService<Owner>>,
}

impl<Owner: ComponentOwner, Scope: PluginScope> PluginServiceDefault<Owner, Scope> {
    pub fn new(
        plugin_repo: Arc<dyn PluginRepo<Owner::PluginOwner, Scope>>,
        library_plugin_files: Arc<PluginWasmFilesService>,
        component_service: Arc<dyn ComponentService<Owner>>,
    ) -> Self {
        Self {
            plugin_repo,
            plugin_wasm_files: library_plugin_files,
            component_service,
        }
    }

    fn decode_plugin_definitions(
        records: Vec<PluginRecord<Owner::PluginOwner, Scope>>,
    ) -> Result<Vec<PluginDefinition<Owner::PluginOwner, Scope>>, PluginError> {
        records
            .into_iter()
            .map(PluginDefinition::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::conversion_error("plugin", e))
    }

    async fn store_plugin_wasm(
        &self,
        data: &PluginWasmFileReference,
        owner: &Owner::PluginOwner,
    ) -> Result<PluginWasmFileKey, PluginError> {
        match data {
            PluginWasmFileReference::BlobStorage(key) => Ok(key.clone()),
            PluginWasmFileReference::Data(stream) => {
                let key = self
                    .plugin_wasm_files
                    .put_if_not_exists(&owner.account_id(), stream)
                    .await
                    .map_err(PluginError::BlobStorageError)?;
                Ok(key)
            }
        }
    }

    async fn check_oplog_processor_plugin(
        &self,
        plugin_owner: &Owner::PluginOwner,
        definition: &OplogProcessorDefinition,
    ) -> Result<(), PluginError> {
        let owner = self
            .component_service
            .get_owner(&definition.component_id)
            .await
            .map_err(PluginError::InternalComponentError)?;

        // Check that the component is visible from this plugin
        let owner = match owner {
            Some(inner) if Owner::PluginOwner::from(inner.clone()) == *plugin_owner => inner,
            _ => Err(PluginError::ComponentNotFound {
                component_id: definition.component_id.clone(),
            })?,
        };

        let versioned_component_id = VersionedComponentId {
            component_id: definition.component_id.clone(),
            version: definition.component_version,
        };

        let component = self
            .component_service
            .get_by_version(&versioned_component_id, &owner)
            .await
            .map_err(PluginError::InternalComponentError)?;

        let component = if let Some(component) = component {
            component
        } else {
            Err(PluginError::ComponentNotFound {
                component_id: definition.component_id.clone(),
            })?
        };

        let implements_oplog_processor_interface = component
            .metadata
            .exports
            .iter()
            .any(is_valid_oplog_processor_implementation);

        if !implements_oplog_processor_interface {
            Err(PluginError::InvalidOplogProcessorPlugin)?
        }

        Ok(())
    }
}

#[async_trait]
impl<Owner: ComponentOwner, Scope: PluginScope> PluginService<Owner::PluginOwner, Scope>
    for PluginServiceDefault<Owner, Scope>
{
    async fn list_plugins(
        &self,
        owner: &Owner::PluginOwner,
    ) -> Result<Vec<PluginDefinition<Owner::PluginOwner, Scope>>, PluginError> {
        let owner_record: <Owner::PluginOwner as PluginOwner>::Row = owner.clone().into();
        let records = self.plugin_repo.get_all(&owner_record).await?;
        Self::decode_plugin_definitions(records)
    }

    async fn list_plugins_for_scope(
        &self,
        owner: &Owner::PluginOwner,
        scope: &Scope,
        request_context: Scope::RequestContext,
    ) -> Result<Vec<PluginDefinition<Owner::PluginOwner, Scope>>, PluginError> {
        let owner_record: <Owner::PluginOwner as PluginOwner>::Row = owner.clone().into();

        let valid_scopes = scope
            .accessible_scopes(request_context)
            .await
            .map_err(|error| PluginError::FailedToGetAvailableScopes { error })?
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
        owner: &Owner::PluginOwner,
        name: &str,
    ) -> Result<Vec<PluginDefinition<Owner::PluginOwner, Scope>>, PluginError> {
        let owner_record: <Owner::PluginOwner as PluginOwner>::Row = owner.clone().into();
        let records = self
            .plugin_repo
            .get_all_with_name(&owner_record, name)
            .await?;
        Self::decode_plugin_definitions(records)
    }

    async fn create_plugin(
        &self,
        owner: &Owner::PluginOwner,
        plugin: PluginDefinitionCreation<Scope>,
    ) -> Result<PluginDefinition<Owner::PluginOwner, Scope>, PluginError> {
        let type_specific_definition = match &plugin.specs {
            PluginTypeSpecificCreation::OplogProcessor(inner) => {
                self.check_oplog_processor_plugin(owner, inner).await?;
                PluginTypeSpecificDefinition::OplogProcessor(inner.clone())
            }
            PluginTypeSpecificCreation::ComponentTransformer(inner) => {
                PluginTypeSpecificDefinition::ComponentTransformer(inner.clone())
            }
            PluginTypeSpecificCreation::App(inner) => {
                let blob_storage_key = self.store_plugin_wasm(&inner.data, owner).await?;
                PluginTypeSpecificDefinition::App(AppPluginDefinition { blob_storage_key })
            }
            PluginTypeSpecificCreation::Library(inner) => {
                let blob_storage_key = self.store_plugin_wasm(&inner.data, owner).await?;
                PluginTypeSpecificDefinition::Library(LibraryPluginDefinition { blob_storage_key })
            }
        };

        let definition =
            plugin.into_definition(PluginId::new_v4(), owner.clone(), type_specific_definition);
        self.plugin_repo.create(&definition.clone().into()).await?;
        Ok(definition)
    }

    async fn get(
        &self,
        owner: &Owner::PluginOwner,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition<Owner::PluginOwner, Scope>>, PluginError> {
        let owner_record: <Owner::PluginOwner as PluginOwner>::Row = owner.clone().into();
        let record = self.plugin_repo.get(&owner_record, name, version).await?;
        record
            .map(PluginDefinition::try_from)
            .transpose()
            .map_err(|e| PluginError::conversion_error("plugin", e))
    }

    async fn get_by_id(
        &self,
        owner: &Owner::PluginOwner,
        id: &PluginId,
    ) -> Result<Option<PluginDefinition<Owner::PluginOwner, Scope>>, PluginError> {
        let owner_record: <Owner::PluginOwner as PluginOwner>::Row = owner.clone().into();
        let record = self.plugin_repo.get_by_id(&owner_record, &id.0).await?;
        record
            .map(PluginDefinition::try_from)
            .transpose()
            .map_err(|e| PluginError::conversion_error("plugin", e))
    }

    async fn delete(
        &self,
        owner: &Owner::PluginOwner,
        name: &str,
        version: &str,
    ) -> Result<(), PluginError> {
        let owner_record: <Owner::PluginOwner as PluginOwner>::Row = owner.clone().into();

        self.plugin_repo
            .delete(&owner_record, name, version)
            .await?;
        Ok(())
    }
}

fn is_valid_oplog_processor_implementation(analyzed_export: &AnalysedExport) -> bool {
    match analyzed_export {
        AnalysedExport::Instance(inner) => {
            let parts = inner.name.split("@").collect::<Vec<_>>();

            parts.len() == 2 && {
                let interface_name = parts[0];
                let version = parts[1];
                interface_name == OPLOG_PROCESSOR_INTERFACE
                    && version.starts_with(OPLOG_PROCESSOR_VERSION_PREFIX)
            }
        }
        _ => false,
    }
}
