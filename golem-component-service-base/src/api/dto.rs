use crate::model::plugin as local_plugin_model;
use crate::model::plugin::PluginWasmFileReference;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::plugin::{PluginOwner, PluginScope};
use golem_common::model::{
    plugin as common_plugin_model, ComponentType, InitialComponentFile, PluginInstallationId,
};
use golem_service_base::model::ComponentName;
use golem_service_base::poem::TempFileUpload;
use golem_service_base::replayable_stream::ReplayableStream;
use poem_openapi::types::Binary;
use poem_openapi::{Multipart, Object};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PluginTypeSpecificCreation {
    ComponentTransformer(common_plugin_model::ComponentTransformerDefinition),
    OplogProcessor(common_plugin_model::OplogProcessorDefinition),
}

impl PluginTypeSpecificCreation {
    pub fn widen(self) -> local_plugin_model::PluginTypeSpecificCreation {
        match self {
            Self::ComponentTransformer(inner) => {
                local_plugin_model::PluginTypeSpecificCreation::ComponentTransformer(inner)
            }
            Self::OplogProcessor(inner) => {
                local_plugin_model::PluginTypeSpecificCreation::OplogProcessor(inner)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinitionCreation<Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificCreation,
    pub scope: Scope,
}

impl<Scope: PluginScope> From<PluginDefinitionCreation<Scope>>
    for local_plugin_model::PluginDefinitionCreation<Scope>
{
    fn from(value: PluginDefinitionCreation<Scope>) -> Self {
        local_plugin_model::PluginDefinitionCreation {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: value.specs.widen(),
            scope: value.scope,
        }
    }
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct LibraryPluginDefinitionCreation<Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Binary<Vec<u8>>,
    pub homepage: String,
    pub scope: Scope,
    pub wasm: TempFileUpload,
}

impl<Scope: PluginScope> From<LibraryPluginDefinitionCreation<Scope>>
    for local_plugin_model::PluginDefinitionCreation<Scope>
{
    fn from(value: LibraryPluginDefinitionCreation<Scope>) -> Self {
        local_plugin_model::PluginDefinitionCreation {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon.0,
            homepage: value.homepage,
            specs: local_plugin_model::PluginTypeSpecificCreation::Library(
                local_plugin_model::LibraryPluginCreation {
                    data: PluginWasmFileReference::Data(value.wasm.boxed()),
                },
            ),
            scope: value.scope,
        }
    }
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct AppPluginDefinitionCreation<Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Binary<Vec<u8>>,
    pub homepage: String,
    pub scope: Scope,
    pub wasm: TempFileUpload,
}

impl<Scope: PluginScope> From<AppPluginDefinitionCreation<Scope>>
    for local_plugin_model::PluginDefinitionCreation<Scope>
{
    fn from(value: AppPluginDefinitionCreation<Scope>) -> Self {
        local_plugin_model::PluginDefinitionCreation {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon.0,
            homepage: value.homepage,
            specs: local_plugin_model::PluginTypeSpecificCreation::App(
                local_plugin_model::AppPluginCreation {
                    data: PluginWasmFileReference::Data(value.wasm.boxed()),
                },
            ),
            scope: value.scope,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PluginTypeSpecificDefinition {
    ComponentTransformer(common_plugin_model::ComponentTransformerDefinition),
    OplogProcessor(common_plugin_model::OplogProcessorDefinition),
    Library(LibraryPluginDefinition),
    App(AppPluginDefinition),
}

impl From<common_plugin_model::PluginTypeSpecificDefinition> for PluginTypeSpecificDefinition {
    fn from(value: common_plugin_model::PluginTypeSpecificDefinition) -> Self {
        match value {
            common_plugin_model::PluginTypeSpecificDefinition::ComponentTransformer(value) => {
                Self::ComponentTransformer(value)
            }
            common_plugin_model::PluginTypeSpecificDefinition::OplogProcessor(value) => {
                Self::OplogProcessor(value)
            }
            common_plugin_model::PluginTypeSpecificDefinition::App(value) => {
                Self::App(value.into())
            }
            common_plugin_model::PluginTypeSpecificDefinition::Library(value) => {
                Self::Library(value.into())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct LibraryPluginDefinition {}

impl From<common_plugin_model::LibraryPluginDefinition> for LibraryPluginDefinition {
    fn from(_value: common_plugin_model::LibraryPluginDefinition) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AppPluginDefinition {}

impl From<common_plugin_model::AppPluginDefinition> for AppPluginDefinition {
    fn from(_value: common_plugin_model::AppPluginDefinition) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinition<Owner: PluginOwner, Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificDefinition,
    pub scope: Scope,
    pub owner: Owner,
}

impl<Owner: PluginOwner, Scope: PluginScope>
    From<common_plugin_model::PluginDefinition<Owner, Scope>> for PluginDefinition<Owner, Scope>
{
    fn from(value: common_plugin_model::PluginDefinition<Owner, Scope>) -> Self {
        Self {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: value.specs.into(),
            scope: value.scope,
            owner: value.owner,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub plugin_name: String,
    pub plugin_version: String,
    /// Whether the referenced plugin is still registered. If false, the installation will still work but the plugin will not show up when listing plugins.
    pub plugin_registered: bool,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

impl PluginInstallation {
    pub fn from_model<Owner: PluginOwner, Scope: PluginScope>(
        model: common_plugin_model::PluginInstallation,
        plugin_definition: common_plugin_model::PluginDefinition<Owner, Scope>,
    ) -> Self {
        Self {
            id: model.id,
            plugin_name: plugin_definition.name,
            plugin_version: plugin_definition.version,
            plugin_registered: !plugin_definition.deleted,
            priority: model.priority,
            parameters: model.parameters,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
    pub env: HashMap<String, String>,
}
