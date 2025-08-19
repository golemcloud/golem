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

use crate::model::agent::AgentType;
use crate::model::component::ComponentName;
use crate::model::component::{
    ComponentFileOptions, ComponentFilePath, ComponentRevision, ComponentType,
};
use crate::model::component_metadata::DynamicLinkedInstance;
use crate::model::plugin::ComponentTransformerDefinition;
use crate::model::plugin::OplogProcessorDefinition;
use crate::model::plugin::{PluginInstallationAction, PluginScope};
use crate::{declare_structs, declare_unions};
use std::collections::{BTreeMap, HashMap};

declare_structs! {
    pub struct CreateComponentRequestMetadata {
        pub component_name: ComponentName,
        pub component_type: Option<ComponentType>,
        pub file_options: Option<BTreeMap<ComponentFilePath, ComponentFileOptions>>,
        pub dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        pub env: Option<BTreeMap<String, String>>,
        pub agent_types: Option<Vec<AgentType>>
    }

    pub struct UpdateComponentRequestMetadata {
        pub previous_version: ComponentRevision,
        pub component_type: Option<ComponentType>,
        pub removed_files: Option<Vec<ComponentFilePath>>,
        pub new_file_options: Option<BTreeMap<ComponentFilePath, ComponentFileOptions>>,
        pub dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        pub env: Option<BTreeMap<String, String>>,
        pub agent_types: Option<Vec<AgentType>>,
        pub plugin_installation_actions: Option<Vec<PluginInstallationAction>>,
    }

    pub struct CreatePluginRequest {
        pub name: String,
        pub version: String,
        pub description: String,
        pub icon: Vec<u8>,
        pub homepage: String,
        pub specs: PluginTypeSpecificCreation,
        pub scope: PluginScope,
    }

    pub struct CreateLibraryPluginRequestMetadata {
        pub name: String,
        pub version: String,
        pub description: String,
        pub homepage: String,
        pub scope: PluginScope
    }

    pub struct CreateAppPluginRequestMetadata {
        pub name: String,
        pub version: String,
        pub description: String,
        pub homepage: String,
        pub scope: PluginScope
    }

}

declare_unions! {
    pub enum PluginTypeSpecificCreation {
        ComponentTransformer(ComponentTransformerDefinition),
        OplogProcessor(OplogProcessorDefinition),
    }
}
