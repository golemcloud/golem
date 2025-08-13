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
use crate::error::ComponentError;
use crate::model::plugin::{
    PluginDefinitionCreation, PluginTypeSpecificCreation, PluginWasmFileReference,
};
use crate::repo::plugin::{PluginRecord, PluginRepo};
use golem_common::model::component::VersionedComponentId;
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::plugin::{
    AppPluginDefinition, LibraryPluginDefinition, OplogProcessorDefinition,
    PluginTypeSpecificDefinition, PluginWasmFileKey,
};
use golem_common::model::plugin::{PluginOwner, PluginScope};
use golem_common::model::PluginId;
use golem_common::repo::{PluginOwnerRow, PluginScopeRow};
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_wasm_ast::analysis::AnalysedExport;
use std::fmt::Debug;
use std::sync::Arc;

const OPLOG_PROCESSOR_INTERFACE: &str = "golem:api/oplog-processor";
const OPLOG_PROCESSOR_VERSION_PREFIX: &str = "1.";

#[derive(Debug)]
pub struct PluginService {
    plugin_repo: Arc<dyn PluginRepo>,
    plugin_wasm_files: Arc<PluginWasmFilesService>,
    component_service: Arc<dyn ComponentService>,
}

impl PluginService {
    pub fn new(
        plugin_repo: Arc<dyn PluginRepo>,
        library_plugin_files: Arc<PluginWasmFilesService>,
        component_service: Arc<dyn ComponentService>,
    ) -> Self {
        Self {
            plugin_repo,
            plugin_wasm_files: library_plugin_files,
            component_service,
        }
    }

    fn decode_plugin_definitions(
        records: Vec<PluginRecord>,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        records
            .into_iter()
            .map(PluginDefinition::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ComponentError::conversion_error("plugin", e))
    }

    async fn store_plugin_wasm(
        &self,
        data: &PluginWasmFileReference,
        owner: &PluginOwner,
    ) -> Result<PluginWasmFileKey, ComponentError> {
        match data {
            PluginWasmFileReference::BlobStorage(key) => Ok(key.clone()),
            PluginWasmFileReference::Data(stream) => {
                let key = self
                    .plugin_wasm_files
                    .put_if_not_exists(&owner.account_id, stream)
                    .await
                    .map_err(ComponentError::BlobStorageError)?;
                Ok(key)
            }
        }
    }

    async fn check_oplog_processor_plugin(
        &self,
        plugin_owner: &PluginOwner,
        definition: &OplogProcessorDefinition,
    ) -> Result<(), ComponentError> {
        let owner = self
            .component_service
            .get_owner(&definition.component_id)
            .await?;

        // Check that the component is visible from this plugin
        let owner = match owner {
            Some(inner) if PluginOwner::from(inner.clone()) == *plugin_owner => inner,
            _ => Err(ComponentError::UnknownComponentId(
                definition.component_id.clone(),
            ))?,
        };

        let versioned_component_id = VersionedComponentId {
            component_id: definition.component_id.clone(),
            version: definition.component_version,
        };

        let component = self
            .component_service
            .get_by_version(&versioned_component_id, &owner)
            .await?;

        let component = if let Some(component) = component {
            component
        } else {
            Err(ComponentError::UnknownComponentId(
                definition.component_id.clone(),
            ))?
        };

        let implements_oplog_processor_interface = component
            .metadata
            .exports()
            .iter()
            .any(is_valid_oplog_processor_implementation);

        if !implements_oplog_processor_interface {
            Err(ComponentError::InvalidOplogProcessorPlugin)?
        }

        Ok(())
    }

    pub async fn list_plugins(
        &self,
        owner: &PluginOwner,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let owner_record: PluginOwnerRow = owner.clone().into();
        let records = self.plugin_repo.get_all(&owner_record).await?;
        Self::decode_plugin_definitions(records)
    }

    pub async fn list_plugin_versions(
        &self,
        owner: &PluginOwner,
        name: &str,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let owner_record: PluginOwnerRow = owner.clone().into();
        let records = self
            .plugin_repo
            .get_all_with_name(&owner_record, name)
            .await?;
        Self::decode_plugin_definitions(records)
    }

    pub async fn list_plugins_for_scopes(
        &self,
        owner: PluginOwner,
        scopes: Vec<PluginScope>,
    ) -> Result<Vec<PluginDefinition>, ComponentError> {
        let owner_record: PluginOwnerRow = owner.clone().into();

        let scope_rows = scopes
            .into_iter()
            .map(|scope| scope.into())
            .collect::<Vec<PluginScopeRow>>();

        let records = self
            .plugin_repo
            .get_for_scope(&owner_record, &scope_rows)
            .await?;

        Self::decode_plugin_definitions(records)
    }

    pub async fn create_plugin(
        &self,
        owner: &PluginOwner,
        plugin: PluginDefinitionCreation,
    ) -> Result<PluginDefinition, ComponentError> {
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

    pub async fn get(
        &self,
        owner: &PluginOwner,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginDefinition>, ComponentError> {
        let owner_record: PluginOwnerRow = owner.clone().into();
        let record = self.plugin_repo.get(&owner_record, name, version).await?;
        record
            .map(PluginDefinition::try_from)
            .transpose()
            .map_err(|e| ComponentError::conversion_error("plugin", e))
    }

    pub async fn get_by_id(
        &self,
        id: &PluginId,
    ) -> Result<Option<PluginDefinition>, ComponentError> {
        let record = self.plugin_repo.get_by_id(&id.0).await?;
        record
            .map(PluginDefinition::try_from)
            .transpose()
            .map_err(|e| ComponentError::conversion_error("plugin", e))
    }

    pub async fn delete(
        &self,
        owner: &PluginOwner,
        name: &str,
        version: &str,
    ) -> Result<(), ComponentError> {
        let owner_record: PluginOwnerRow = owner.clone().into();

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
