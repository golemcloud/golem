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

use super::component::ComponentOwner;
use super::{Empty, PluginId, ProjectId};
use crate::model::{
    AccountId, ComponentId, ComponentVersion, PluginInstallationId, PoemTypeRequirements,
};
use core::fmt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentPluginScope {
    pub component_id: ComponentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub plugin_id: PluginId,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PluginInstallationAction {
    Install(PluginInstallationCreation),
    Update(PluginInstallationUpdateWithId),
    Uninstall(PluginUninstallation),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginUninstallation {
    pub installation_id: PluginInstallationId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationCreation {
    pub name: String,
    pub version: String,
    /// Plugins will be applied in order of increasing priority
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationUpdate {
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationUpdateWithId {
    pub installation_id: PluginInstallationId,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

pub trait PluginInstallationTarget:
    Debug
    + Display
    + Clone
    + PartialEq
    + Serialize
    + for<'de> Deserialize<'de>
    + PoemTypeRequirements
    + Send
    + Sync
    + 'static
{
    #[cfg(feature = "sql")]
    type Row: crate::repo::RowMeta<sqlx::Sqlite>
        + crate::repo::RowMeta<sqlx::Postgres>
        + for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>
        + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>
        + From<Self>
        + TryInto<Self, Error = String>
        + Clone
        + Display
        + Send
        + Sync
        + Unpin
        + 'static;

    #[cfg(feature = "sql")]
    fn table_name() -> &'static str;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginOwner {
    pub account_id: AccountId,
}

impl Display for PluginOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.account_id)
    }
}

impl FromStr for PluginOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            account_id: AccountId::from(s),
        })
    }
}

impl From<ComponentOwner> for PluginOwner {
    fn from(value: ComponentOwner) -> Self {
        Self {
            account_id: value.account_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinition {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificDefinition,
    pub scope: PluginScope,
    pub owner: PluginOwner,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
#[repr(i8)]
pub enum PluginType {
    ComponentTransformer = 0,
    OplogProcessor = 1,
    Library = 2,
    App = 3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PluginTypeSpecificDefinition {
    ComponentTransformer(ComponentTransformerDefinition),
    OplogProcessor(OplogProcessorDefinition),
    Library(LibraryPluginDefinition),
    App(AppPluginDefinition),
}

impl PluginTypeSpecificDefinition {
    pub fn plugin_type(&self) -> PluginType {
        match self {
            PluginTypeSpecificDefinition::ComponentTransformer(_) => {
                PluginType::ComponentTransformer
            }
            PluginTypeSpecificDefinition::OplogProcessor(_) => PluginType::OplogProcessor,
            PluginTypeSpecificDefinition::Library(_) => PluginType::Library,
            PluginTypeSpecificDefinition::App(_) => PluginType::App,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentTransformerDefinition {
    pub provided_wit_package: Option<String>,
    pub json_schema: Option<String>,
    pub validate_url: String,
    pub transform_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct OplogProcessorDefinition {
    pub component_id: ComponentId,
    pub component_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
pub struct PluginWasmFileKey(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LibraryPluginDefinition {
    pub blob_storage_key: PluginWasmFileKey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AppPluginDefinition {
    pub blob_storage_key: PluginWasmFileKey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProjectPluginScope {
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
pub enum PluginScope {
    Global(Empty),
    Component(ComponentPluginScope),
    Project(ProjectPluginScope),
}

impl PluginScope {
    pub fn global() -> Self {
        Self::Global(Empty {})
    }

    pub fn component(component_id: ComponentId) -> Self {
        Self::Component(ComponentPluginScope { component_id })
    }

    pub fn project(project_id: ProjectId) -> Self {
        Self::Project(ProjectPluginScope { project_id })
    }

    pub fn valid_in_component(&self, component_id: &ComponentId, project_id: &ProjectId) -> bool {
        match self {
            Self::Global(_) => true,
            Self::Component(scope) => &scope.component_id == component_id,
            Self::Project(scope) => &scope.project_id == project_id,
        }
    }
}

impl Default for PluginScope {
    fn default() -> Self {
        PluginScope::global()
    }
}

impl Display for PluginScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global(_) => write!(f, "global"),
            Self::Component(scope) => write!(f, "component:{}", scope.component_id),
            Self::Project(scope) => write!(f, "project:{}", scope.project_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentPluginInstallationTarget {
    pub component_id: ComponentId,
    pub component_version: ComponentVersion,
}

impl Display for ComponentPluginInstallationTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.component_id, self.component_version)
    }
}

impl PluginInstallationTarget for ComponentPluginInstallationTarget {
    #[cfg(feature = "sql")]
    type Row = crate::repo::ComponentPluginInstallationRow;

    #[cfg(feature = "sql")]
    fn table_name() -> &'static str {
        "component_plugin_installation"
    }
}

#[cfg(feature = "poem")]
mod poem {
    use super::{ComponentId, PluginScope, ProjectId};
    use poem::web::Field;
    use poem_openapi::types::{
        ParseError, ParseFromMultipartField, ParseFromParameter, ParseResult,
    };

    impl ParseFromParameter for PluginScope {
        fn parse_from_parameter(value: &str) -> ParseResult<Self> {
            if value == "global" {
                Ok(Self::global())
            } else if let Some(id_part) = value.strip_prefix("component:") {
                let component_id = ComponentId::try_from(id_part);
                match component_id {
                    Ok(component_id) => Ok(Self::component(component_id)),
                    Err(err) => Err(ParseError::custom(err)),
                }
            } else if let Some(id_part) = value.strip_prefix("project:") {
                let project_id = ProjectId::try_from(id_part);
                match project_id {
                    Ok(project_id) => Ok(Self::project(project_id)),
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
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::{
        AppPluginDefinition, ComponentTransformerDefinition, LibraryPluginDefinition,
        OplogProcessorDefinition, PluginDefinition, PluginInstallation, PluginOwner, PluginScope,
        PluginTypeSpecificDefinition, PluginWasmFileKey,
    };

    impl From<PluginInstallation> for golem_api_grpc::proto::golem::component::PluginInstallation {
        fn from(plugin_installation: PluginInstallation) -> Self {
            golem_api_grpc::proto::golem::component::PluginInstallation {
                id: Some(plugin_installation.id.into()),
                plugin_id: Some(plugin_installation.plugin_id.into()),
                priority: plugin_installation.priority,
                parameters: plugin_installation.parameters,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginInstallation> for PluginInstallation {
        type Error = String;

        fn try_from(
            proto: golem_api_grpc::proto::golem::component::PluginInstallation,
        ) -> Result<Self, Self::Error> {
            Ok(PluginInstallation {
                id: proto.id.ok_or("Missing id")?.try_into()?,
                plugin_id: proto.plugin_id.ok_or("Missing plugin id")?.try_into()?,
                priority: proto.priority,
                parameters: proto.parameters,
            })
        }
    }

    impl From<PluginTypeSpecificDefinition>
        for golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition
    {
        fn from(value: PluginTypeSpecificDefinition) -> Self {
            match value {
                PluginTypeSpecificDefinition::ComponentTransformer(value) => golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition {
                    definition: Some(golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::ComponentTransformer(value.into()))
                },
                PluginTypeSpecificDefinition::OplogProcessor(value) => golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition {
                    definition: Some(golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::OplogProcessor(value.into()))
                },
                PluginTypeSpecificDefinition::Library(value) => golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition {
                    definition: Some(golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::Library(value.into()))
                },
                PluginTypeSpecificDefinition::App(value) => golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition {
                    definition: Some(golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::App(value.into()))
                }
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition>
        for PluginTypeSpecificDefinition
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition,
        ) -> Result<Self, Self::Error> {
            match value.definition.ok_or("Missing plugin type specific definition")? {
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::ComponentTransformer(value) => Ok(PluginTypeSpecificDefinition::ComponentTransformer(value.try_into()?)),
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::OplogProcessor(value) => Ok(PluginTypeSpecificDefinition::OplogProcessor(value.try_into()?)),
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::Library(value) => Ok(PluginTypeSpecificDefinition::Library(value.try_into()?)),
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::App(value) => Ok(PluginTypeSpecificDefinition::App(value.try_into()?))
            }
        }
    }

    impl From<ComponentTransformerDefinition>
        for golem_api_grpc::proto::golem::component::ComponentTransformerDefinition
    {
        fn from(value: ComponentTransformerDefinition) -> Self {
            golem_api_grpc::proto::golem::component::ComponentTransformerDefinition {
                provided_wit_package: value.provided_wit_package,
                json_schema: value.json_schema,
                validate_url: value.validate_url,
                transform_url: value.transform_url,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ComponentTransformerDefinition>
        for ComponentTransformerDefinition
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ComponentTransformerDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(ComponentTransformerDefinition {
                provided_wit_package: value.provided_wit_package,
                json_schema: value.json_schema,
                validate_url: value.validate_url,
                transform_url: value.transform_url,
            })
        }
    }

    impl From<OplogProcessorDefinition>
        for golem_api_grpc::proto::golem::component::OplogProcessorDefinition
    {
        fn from(value: OplogProcessorDefinition) -> Self {
            golem_api_grpc::proto::golem::component::OplogProcessorDefinition {
                component_id: Some(value.component_id.into()),
                component_version: value.component_version,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::OplogProcessorDefinition>
        for OplogProcessorDefinition
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::OplogProcessorDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(OplogProcessorDefinition {
                component_id: value
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_version: value.component_version,
            })
        }
    }

    impl From<LibraryPluginDefinition>
        for golem_api_grpc::proto::golem::component::LibraryPluginDefinition
    {
        fn from(value: LibraryPluginDefinition) -> Self {
            golem_api_grpc::proto::golem::component::LibraryPluginDefinition {
                blob_storage_key: value.blob_storage_key.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::LibraryPluginDefinition>
        for LibraryPluginDefinition
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::LibraryPluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(LibraryPluginDefinition {
                blob_storage_key: PluginWasmFileKey(value.blob_storage_key),
            })
        }
    }

    impl From<AppPluginDefinition> for golem_api_grpc::proto::golem::component::AppPluginDefinition {
        fn from(value: AppPluginDefinition) -> Self {
            golem_api_grpc::proto::golem::component::AppPluginDefinition {
                blob_storage_key: value.blob_storage_key.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::AppPluginDefinition> for AppPluginDefinition {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::AppPluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(AppPluginDefinition {
                blob_storage_key: PluginWasmFileKey(value.blob_storage_key),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginDefinition> for PluginDefinition {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::PluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                id: value.id.ok_or("Missing plugin id")?.try_into()?,
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
                scope: value.scope.ok_or("Missing plugin scope")?.try_into()?,
                owner: PluginOwner {
                    account_id: value.account_id.ok_or("Missing plugin owner")?.into(),
                },
                deleted: value.deleted,
            })
        }
    }

    impl From<PluginDefinition> for golem_api_grpc::proto::golem::component::PluginDefinition {
        fn from(value: PluginDefinition) -> Self {
            golem_api_grpc::proto::golem::component::PluginDefinition {
                id: Some(value.id.into()),
                name: value.name,
                version: value.version,
                scope: Some(value.scope.into()),
                account_id: Some(value.owner.account_id.into()),
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                specs: Some(value.specs.into()),
                deleted: value.deleted,
            }
        }
    }

    impl From<PluginScope> for golem_api_grpc::proto::golem::component::PluginScope {
        fn from(scope: PluginScope) -> Self {
            match scope {
                PluginScope::Global(_) => golem_api_grpc::proto::golem::component::PluginScope {
                    scope: Some(golem_api_grpc::proto::golem::component::plugin_scope::Scope::Global(
                        golem_api_grpc::proto::golem::common::Empty {},
                    )),
                },
                PluginScope::Component(scope) => golem_api_grpc::proto::golem::component::PluginScope {
                    scope: Some(golem_api_grpc::proto::golem::component::plugin_scope::Scope::Component(
                        golem_api_grpc::proto::golem::component::ComponentPluginScope {
                            component_id: Some(scope.component_id.into()),
                        },
                    )),
                },
                PluginScope::Project(scope) => golem_api_grpc::proto::golem::component::PluginScope {
                    scope: Some(golem_api_grpc::proto::golem::component::plugin_scope::Scope::Project(
                        golem_api_grpc::proto::golem::component::ProjectPluginScope {
                            project_id: Some(scope.project_id.into()),
                        },
                    )),
                },
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginScope> for PluginScope {
        type Error = String;

        fn try_from(
            proto: golem_api_grpc::proto::golem::component::PluginScope,
        ) -> Result<Self, Self::Error> {
            match proto.scope {
                Some(
                    golem_api_grpc::proto::golem::component::plugin_scope::Scope::Global(_),
                ) => Ok(Self::global()),
                Some(
                    golem_api_grpc::proto::golem::component::plugin_scope::Scope::Component(
                        scope,
                    ),
                ) => Ok(Self::component(
                    scope
                        .component_id
                        .ok_or("Missing component_id")?
                        .try_into()?,
                )),
                Some(
                    golem_api_grpc::proto::golem::component::plugin_scope::Scope::Project(
                        scope,
                    ),
                ) => Ok(Self::project(
                    scope.project_id.ok_or("Missing project_id")?.try_into()?,
                )),
                None => Err("Missing scope".to_string()),
            }
        }
    }
}
