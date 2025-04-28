// Copyright 2024-2025 Golem Cloud
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

use chrono::Utc;
use golem_common::model::component::{ComponentOwner, DefaultComponentOwner, VersionedComponentId};
use golem_common::model::component_constraint::{
    FunctionConstraints, FunctionSignature, FunctionUsageConstraint,
};
use golem_common::model::component_metadata::{
    ComponentMetadata, ComponentProcessingError, DynamicLinkedInstance,
};
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::InitialComponentFile;
use golem_common::model::{ComponentFilePathWithPermissions, ComponentId, ComponentType};
use golem_service_base::model::ComponentName;
use rib::WorkerFunctionsInRib;
use std::collections::HashMap;
use std::fmt::Debug;
use std::time::SystemTime;
use tempfile::NamedTempFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component<Owner: ComponentOwner> {
    pub owner: Owner,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<Utc>,
    pub component_type: ComponentType,
    pub object_store_key: Option<String>,
    pub transformed_object_store_key: Option<String>,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
}

impl<Owner: ComponentOwner> Component<Owner> {
    pub fn new(
        component_id: ComponentId,
        component_name: ComponentName,
        component_type: ComponentType,
        data: &[u8],
        files: Vec<InitialComponentFile>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        owner: Owner,
    ) -> Result<Component<Owner>, ComponentProcessingError> {
        let mut metadata = ComponentMetadata::analyse_component(data)?;
        metadata.dynamic_linking = dynamic_linking;

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.clone(),
            version: 0,
        };

        Ok(Component {
            owner,
            component_name,
            component_size: data.len() as u64,
            metadata,
            created_at: Utc::now(),
            object_store_key: Some(versioned_component_id.to_string()),
            transformed_object_store_key: Some(versioned_component_id.to_string()),
            versioned_component_id,
            component_type,
            files,
            installed_plugins,
        })
    }

    pub fn user_object_store_key(&self) -> String {
        format!(
            "{}:user",
            self.object_store_key
                .as_ref()
                .unwrap_or(&self.versioned_component_id.to_string())
        )
    }

    pub fn protected_object_store_key(&self) -> String {
        format!(
            "{}:protected",
            self.transformed_object_store_key
                .as_ref()
                .unwrap_or(&self.versioned_component_id.to_string())
        )
    }

    pub fn owns_stored_object(&self) -> bool {
        self.object_store_key == Some(self.versioned_component_id.to_string())
    }
}

impl<Owner: ComponentOwner> From<Component<Owner>> for golem_service_base::model::Component {
    fn from(value: Component<Owner>) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: Some(value.created_at),
            component_type: Some(value.component_type),
            files: value.files,
            installed_plugins: value.installed_plugins,
        }
    }
}

impl From<Component<DefaultComponentOwner>> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component<DefaultComponentOwner>) -> Self {
        let component_type: golem_api_grpc::proto::golem::component::ComponentType =
            value.component_type.into();
        Self {
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            account_id: None,
            project_id: None,
            created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                value.created_at,
            ))),
            component_type: Some(component_type.into()),
            files: value.files.into_iter().map(|file| file.into()).collect(),
            installed_plugins: value
                .installed_plugins
                .into_iter()
                .map(|plugin| plugin.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentConstraints<Owner: ComponentOwner> {
    pub owner: Owner,
    pub component_id: ComponentId,
    pub constraints: FunctionConstraints,
}

impl<Owner: ComponentOwner> ComponentConstraints<Owner> {
    pub fn function_signatures(&self) -> Vec<FunctionSignature> {
        let constraints = &self.constraints;

        constraints
            .constraints
            .iter()
            .map(|x| x.function_signature.clone())
            .collect()
    }
}

impl<Owner: ComponentOwner> ComponentConstraints<Owner> {
    pub fn init(
        owner: &Owner,
        component_id: &ComponentId,
        worker_functions_in_rib: WorkerFunctionsInRib,
    ) -> ComponentConstraints<Owner> {
        ComponentConstraints {
            owner: owner.clone(),
            component_id: component_id.clone(),
            constraints: FunctionConstraints {
                constraints: worker_functions_in_rib
                    .function_calls
                    .iter()
                    .map(FunctionUsageConstraint::from_worker_function_type)
                    .collect(),
            },
        }
    }
}

#[derive(Debug)]
pub struct InitialComponentFilesArchiveAndPermissions {
    pub archive: NamedTempFile,
    pub files: Vec<ComponentFilePathWithPermissions>,
}
