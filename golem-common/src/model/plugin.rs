use super::{PluginId, PoemMultipartTypeRequirements};
use crate::model::{
    AccountId, ComponentId, ComponentVersion, PluginInstallationId, PoemTypeRequirements,
};
use async_trait::async_trait;
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

pub trait PluginOwner:
    Debug
    + Display
    + FromStr<Err = String>
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
        + Debug
        + Display
        + Send
        + Sync
        + Unpin
        + 'static;

    fn account_id(&self) -> AccountId;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinition<Owner: PluginOwner, Scope: PluginScope> {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificDefinition,
    pub scope: Scope,
    pub owner: Owner,
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

#[async_trait]
pub trait PluginScope:
    Debug
    + Clone
    + PartialEq
    + Serialize
    + for<'de> Deserialize<'de>
    + PoemTypeRequirements
    + PoemMultipartTypeRequirements
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
        + Send
        + Sync
        + Unpin
        + 'static;

    /// Context required to calculate the set of `accessible_scopes`
    type RequestContext: Send + Sync + 'static;

    /// Gets all the plugin scopes valid for this given scope
    async fn accessible_scopes(&self, context: Self::RequestContext) -> Result<Vec<Self>, String>;
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
    type Row = crate::repo::plugin_installation::ComponentPluginInstallationRow;

    #[cfg(feature = "sql")]
    fn table_name() -> &'static str {
        "component_plugin_installation"
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::plugin::{
        AppPluginDefinition, ComponentTransformerDefinition, LibraryPluginDefinition,
        OplogProcessorDefinition, PluginInstallation, PluginTypeSpecificDefinition,
        PluginWasmFileKey,
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
}
