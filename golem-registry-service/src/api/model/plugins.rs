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

use poem_openapi::{Multipart, NewType, Object, Union};
use golem_common_next::model::{ComponentId, ComponentVersion, Empty, PluginId, ProjectId};
use poem_openapi::types::{Binary, ParseError, ParseFromMultipartField, ParseFromParameter, ParseResult};
use poem::web::Field;
use golem_service_base_next::poem::TempFileUpload;

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct PluginDefinition {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificDefinition,
    pub scope: PluginScope,
    pub deleted: bool,
}

#[derive(Debug, Clone, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum PluginTypeSpecificDefinition {
    ComponentTransformer(ComponentTransformerDefinition),
    OplogProcessor(OplogProcessorDefinition),
    Library(LibraryPluginDefinition),
    App(AppPluginDefinition),
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct ComponentTransformerDefinition {
    pub provided_wit_package: Option<String>,
    pub json_schema: Option<String>,
    pub validate_url: String,
    pub transform_url: String,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct OplogProcessorDefinition {
    pub component_id: ComponentId,
    pub component_version: ComponentVersion,
}

#[derive(Debug, Clone, NewType)]
pub struct PluginWasmFileKey(pub String);

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct LibraryPluginDefinition {
    pub blob_storage_key: PluginWasmFileKey,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct AppPluginDefinition {
    pub blob_storage_key: PluginWasmFileKey,
}

#[derive(Debug, Clone, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum PluginScope {
    Global(Empty),
    Project(ProjectPluginScope),
    Component(ComponentPluginScope),
}

impl ParseFromParameter for PluginScope {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        if value == "global" {
            Ok(Self::Global(Empty {  }))
        } else if let Some(id_part) = value.strip_prefix("component:") {
            let component_id = ComponentId::try_from(id_part);
            match component_id {
                Ok(component_id) => Ok(Self::Component(ComponentPluginScope { component_id })),
                Err(err) => Err(ParseError::custom(err)),
            }
        } else if let Some(id_part) = value.strip_prefix("project:") {
            let project_id = ProjectId::try_from(id_part);
            match project_id {
                Ok(project_id) => Ok(Self::Project(ProjectPluginScope { project_id })),
                Err(err) => Err(ParseError::custom(err)),
            }
        } else {
            Err(ParseError::custom("Unexpected representation of plugin scope - must be 'global', 'component:<component_id>' or 'project:<project_id>'"))
        }
    }
}

impl ParseFromMultipartField for PluginScope {
    async fn parse_from_multipart(field: Option<Field>) -> ParseResult<Self> {
        use poem_openapi::types::ParseFromParameter;
        match field {
            Some(field) => {
                let s = field.text().await?;
                Self::parse_from_parameter(&s)
            }
            None => Err(ParseError::expected_input()),
        }
    }
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct ProjectPluginScope {
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct ComponentPluginScope {
    pub component_id: ComponentId,
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct CreatePluginRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificCreation,
    pub scope: PluginScope,
}

#[derive(Debug, Clone, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum PluginTypeSpecificCreation {
    ComponentTransformer(ComponentTransformerDefinition),
    OplogProcessor(OplogProcessorDefinition),
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateLibraryPluginRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Binary<Vec<u8>>,
    pub homepage: String,
    pub scope: PluginScope,
    pub wasm: TempFileUpload,
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateAppPluginRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Binary<Vec<u8>>,
    pub homepage: String,
    pub scope: PluginScope,
    pub wasm: TempFileUpload,
}
