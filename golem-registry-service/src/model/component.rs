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

use chrono::Utc;
use golem_common::model::agent::AgentType;
use golem_common::model::component::{ComponentName, VersionedComponentId};
use golem_common::model::component_metadata::{
    ComponentMetadata, ComponentProcessingError, DynamicLinkedInstance,
};
use golem_common::model::diff::Hash;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{
    ComponentFilePathWithPermissions, ComponentFilePermissions, ComponentId, ComponentType, InitialComponentFile, PluginInstallationId
};
use golem_wasm_ast::analysis::AnalysedType;
use poem_openapi::{Object, Union};
use rib::FunctionName;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use tempfile::NamedTempFile;
use uuid::Uuid;

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub environment_id: EnvironmentId,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
    pub env: BTreeMap<String, String>,

    /// Hash of the wasm before any transformations
    pub wasm_hash: golem_common::model::diff::Hash,

    #[oai(skip)]
    pub original_files: Vec<InitialComponentFile>,
    #[oai(skip)]
    pub original_env: BTreeMap<String, String>,
    #[oai(skip)]
    pub object_store_key: String,
    #[oai(skip)]
    pub transformed_object_store_key: String,
}

impl Component {
    pub fn new(
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_name: ComponentName,
        component_type: ComponentType,
        data: &[u8],
        files: Vec<InitialComponentFile>,
        installed_plugins: Vec<PluginInstallation>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        env: BTreeMap<String, String>,
        agent_types: Vec<AgentType>,
        wasm_hash: Hash,
    ) -> Result<Self, ComponentProcessingError> {
        let metadata = ComponentMetadata::analyse_component(data, dynamic_linking, agent_types)?;

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.clone(),
            version: 0,
        };

        Ok(Self {
            environment_id,
            component_name,
            component_size: data.len() as u64,
            metadata,
            created_at: Utc::now(),
            object_store_key: Uuid::new_v4().to_string(),
            transformed_object_store_key: Uuid::new_v4().to_string(),
            versioned_component_id,
            component_type,
            original_files: files.clone(),
            files,
            installed_plugins,
            original_env: env.clone(),
            env,
            wasm_hash,
        })
    }

    pub fn full_object_store_key(&self) -> String {
        format!("{}:user", self.object_store_key)
    }

    pub fn full_transformed_object_store_key(&self) -> String {
        format!("{}:protected", self.transformed_object_store_key)
    }

    pub fn regenerate_object_store_key(&mut self) {
        self.object_store_key = Uuid::new_v4().to_string();
    }

    pub fn regenerate_transformed_object_store_key(&mut self) {
        self.transformed_object_store_key = Uuid::new_v4().to_string();
    }

    pub fn reset_transformations(&mut self) {
        self.env = self.original_env.clone();
        self.files = self.original_files.clone();
    }
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub plugin_name: String,
    pub plugin_version: String,
    /// Whether the referenced plugin is still registered. If false, the installation will still work but the plugin will not show up when listing plugins.
    pub plugin_registered: bool,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

#[derive(Debug)]
pub struct InitialComponentFilesArchiveAndPermissions {
    pub archive: NamedTempFile,
    pub files: Vec<ComponentFilePathWithPermissions>,
}

#[derive(Debug)]
pub struct ConflictReport {
    pub missing_functions: Vec<FunctionName>,
    pub conflicting_functions: Vec<ConflictingFunction>,
}

impl ConflictReport {
    pub fn is_empty(&self) -> bool {
        self.missing_functions.is_empty() && self.conflicting_functions.is_empty()
    }
}

impl Display for ConflictReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Handling missing functions
        writeln!(f, "Missing Functions:")?;
        if self.missing_functions.is_empty() {
            writeln!(f, "  None")?;
        } else {
            for missing_function in &self.missing_functions {
                writeln!(f, "  - {missing_function}")?;
            }
        }

        // Handling conflicting functions
        writeln!(f, "\nFunctions with conflicting types:")?;
        if self.conflicting_functions.is_empty() {
            writeln!(f, "  None")?;
        } else {
            for conflict in &self.conflicting_functions {
                writeln!(f, "{conflict}")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct ConflictingFunction {
    pub function: FunctionName,
    pub parameter_type_conflict: Option<ParameterTypeConflict>,
    pub return_type_conflict: Option<ReturnTypeConflict>,
}

impl Display for ConflictingFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Function: {}", self.function)?;

        match self.parameter_type_conflict {
            Some(ref conflict) => {
                writeln!(f, "  Parameter Type Conflict:")?;
                writeln!(
                    f,
                    "    Existing: {}",
                    convert_to_pretty_types(&conflict.existing)
                )?;
                writeln!(
                    f,
                    "    New:      {}",
                    convert_to_pretty_types(&conflict.new)
                )?;
            }
            None => {
                writeln!(f, "  Parameter Type Conflict: None")?;
            }
        }

        match self.return_type_conflict {
            Some(ref conflict) => {
                writeln!(f, "  Result Type Conflict:")?;
                writeln!(
                    f,
                    "    Existing: {}",
                    convert_to_pretty_type(&conflict.existing)
                )?;
                writeln!(f, "    New:      {}", convert_to_pretty_type(&conflict.new))?;
            }
            None => {
                writeln!(f, "  Result Type Conflict: None")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct ParameterTypeConflict {
    pub existing: Vec<AnalysedType>,
    pub new: Vec<AnalysedType>,
}

#[derive(Debug)]
pub struct ReturnTypeConflict {
    pub existing: Option<AnalysedType>,
    pub new: Option<AnalysedType>,
}

fn convert_to_pretty_types(analysed_types: &[AnalysedType]) -> String {
    let type_names = analysed_types
        .iter()
        .map(|x| {
            rib::TypeName::try_from(x.clone()).map_or("unknown".to_string(), |x| x.to_string())
        })
        .collect::<Vec<_>>();

    type_names.join(", ")
}

fn convert_to_pretty_type(analysed_type: &Option<AnalysedType>) -> String {
    analysed_type
        .as_ref()
        .map(|x| {
            rib::TypeName::try_from(x.clone()).map_or("unknown".to_string(), |x| x.to_string())
        })
        .unwrap_or("unit".to_string())
}

#[derive(Clone, Debug, Object)]
pub struct PreviousVersionComponentFileSource {
    /// path in the filesystem of the previous component version
    pub path_in_previous_version: String,
}

#[derive(Clone, Debug, Object)]
pub struct ArchiveComponentFileSource {
    /// path in the archive that was uploaded as part of this request
    pub path_in_archive: String,
}

#[derive(Clone, Debug, Union)]
#[oai(one_of = true)]
pub enum ComponentFileSource {
    PreviousVersion(PreviousVersionComponentFileSource),
    Archive(ArchiveComponentFileSource),
}

#[derive(Clone, Debug, Object)]
pub struct ComponentFileOptions {
    /// Path of the file in the uploaded archive
    pub permissions: ComponentFilePermissions,
}

impl Default for ComponentFileOptions {
    fn default() -> Self {
        Self {
            permissions: ComponentFilePermissions::ReadOnly
        }
    }
}

#[derive(Clone, Debug, Object)]
pub struct ComponentFileOptionsForUpdate {
    /// Path of the file in the uploaded archive
    pub source: ComponentFileSource,
    pub permissions: ComponentFilePermissions,
}
