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

use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentType;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::InitialComponentFile;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component::{ComponentName, InstalledPlugin};
use golem_common::model::component_metadata::{
    ComponentMetadata, ComponentProcessingError, DynamicLinkedInstance,
};
use golem_common::model::diff::{self, Hash};
use golem_common::model::environment::EnvironmentId;
use golem_wasm::analysis::AnalysedType;
use rib::FunctionName;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub struct NewComponentRevision {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub original_files: Vec<InitialComponentFile>,
    pub files: Vec<InitialComponentFile>,
    pub original_env: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub wasm_hash: Hash,
    pub object_store_key: String,
    pub installed_plugins: Vec<InstalledPlugin>,

    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
    pub agent_types: Vec<AgentType>,
}

impl NewComponentRevision {
    pub fn new(
        component_id: ComponentId,
        component_revision: ComponentRevision,
        environment_id: EnvironmentId,
        component_name: ComponentName,
        files: Vec<InitialComponentFile>,
        env: BTreeMap<String, String>,
        wasm_hash: Hash,
        object_store_key: String,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        installed_plugins: Vec<InstalledPlugin>,
        agent_types: Vec<AgentType>,
    ) -> Self {
        Self {
            component_id,
            component_revision,
            environment_id,
            component_name,
            original_files: files.clone(),
            files,
            original_env: env.clone(),
            env,
            wasm_hash,
            object_store_key,
            installed_plugins,
            dynamic_linking,
            agent_types,
        }
    }

    pub fn with_transformed_component(
        self,
        transformed_object_store_key: String,
        transformed_data: &[u8],
    ) -> Result<FinalizedComponentRevision, ComponentProcessingError> {
        let metadata = ComponentMetadata::analyse_component(
            transformed_data,
            self.dynamic_linking,
            self.agent_types,
        )?;

        Ok(FinalizedComponentRevision {
            component_id: self.component_id,
            component_revision: self.component_revision,
            environment_id: self.environment_id,
            component_name: self.component_name,
            original_files: self.original_files,
            files: self.files,
            original_env: self.original_env,
            env: self.env,
            wasm_hash: self.wasm_hash,
            object_store_key: self.object_store_key,
            installed_plugins: self.installed_plugins,

            transformed_object_store_key,
            metadata,
            component_size: transformed_data.len() as u64,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FinalizedComponentRevision {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub original_files: Vec<InitialComponentFile>,
    pub files: Vec<InitialComponentFile>,
    pub original_env: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub wasm_hash: golem_common::model::diff::Hash,
    pub object_store_key: String,
    pub installed_plugins: Vec<InstalledPlugin>,

    pub transformed_object_store_key: String,
    pub metadata: ComponentMetadata,
    pub component_size: u64,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub id: ComponentId,
    pub revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub hash: diff::Hash,
    pub application_id: ApplicationId,
    pub account_id: AccountId,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<InstalledPlugin>,
    pub env: BTreeMap<String, String>,

    /// Hash of the wasm before any transformations
    pub wasm_hash: diff::Hash,

    pub original_files: Vec<InitialComponentFile>,
    pub original_env: BTreeMap<String, String>,
    pub object_store_key: String,
    pub transformed_object_store_key: String,
}

impl Component {
    pub fn into_new_revision(self) -> anyhow::Result<NewComponentRevision> {
        Ok(NewComponentRevision {
            component_id: self.id,
            component_revision: self.revision.next()?,
            environment_id: self.environment_id,
            component_name: self.component_name,
            original_files: self.original_files,
            files: self.files,
            original_env: self.original_env,
            env: self.env,
            wasm_hash: self.wasm_hash,
            object_store_key: self.object_store_key,
            installed_plugins: self.installed_plugins,

            agent_types: self.metadata.agent_types().to_vec(),
            dynamic_linking: self.metadata.dynamic_linking().clone(),
        })
    }
}

impl From<Component> for golem_common::model::component::ComponentDto {
    fn from(value: Component) -> Self {
        Self {
            id: value.id,
            revision: value.revision,
            environment_id: value.environment_id,
            application_id: value.application_id,
            account_id: value.account_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: value.created_at,
            original_files: value.original_files,
            files: value.files,
            installed_plugins: value.installed_plugins,
            original_env: value.original_env,
            env: value.env,
            wasm_hash: value.wasm_hash,
            hash: value.hash,
        }
    }
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
