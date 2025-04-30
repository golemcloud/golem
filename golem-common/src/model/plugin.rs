use super::{PluginId, PoemMultipartTypeRequirements};
use crate::model::{
    AccountId, ComponentId, ComponentVersion, Empty, PluginInstallationId, PoemTypeRequirements,
};
use async_trait::async_trait;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum DefaultPluginScope {
    Global(Empty),
    Component(ComponentPluginScope),
}

impl DefaultPluginScope {
    pub fn global() -> Self {
        DefaultPluginScope::Global(Empty {})
    }

    pub fn component(component_id: ComponentId) -> Self {
        DefaultPluginScope::Component(ComponentPluginScope { component_id })
    }

    pub fn valid_in_component(&self, component_id: &ComponentId) -> bool {
        match self {
            DefaultPluginScope::Global(_) => true,
            DefaultPluginScope::Component(scope) => &scope.component_id == component_id,
        }
    }
}

impl Default for DefaultPluginScope {
    fn default() -> Self {
        DefaultPluginScope::global()
    }
}

impl Display for DefaultPluginScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultPluginScope::Global(_) => write!(f, "global"),
            DefaultPluginScope::Component(scope) => write!(f, "component:{}", scope.component_id),
        }
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ParseFromParameter for DefaultPluginScope {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        if value == "global" {
            Ok(Self::global())
        } else if let Some(id_part) = value.strip_prefix("component:") {
            let component_id = ComponentId::try_from(id_part);
            match component_id {
                Ok(component_id) => Ok(Self::component(component_id)),
                Err(err) => Err(poem_openapi::types::ParseError::<Self>::custom(err)),
            }
        } else {
            Err(poem_openapi::types::ParseError::<Self>::custom("Unexpected representation of plugin scope - must be 'global' or 'component:<component_id>'".to_string()))
        }
    }
}

#[cfg(feature = "poem")]
impl poem_openapi::types::ParseFromMultipartField for DefaultPluginScope {
    async fn parse_from_multipart(
        field: Option<poem::web::Field>,
    ) -> poem_openapi::types::ParseResult<Self> {
        use poem_openapi::types::ParseFromParameter;
        match field {
            Some(field) => {
                let s = field.text().await?;
                Self::parse_from_parameter(&s)
            }
            None => Err(poem_openapi::types::ParseError::expected_input()),
        }
    }
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

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct DefaultPluginOwner;

impl Display for DefaultPluginOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "default")
    }
}

impl FromStr for DefaultPluginOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "default" {
            Ok(DefaultPluginOwner)
        } else {
            Err("Failed to parse empty namespace".to_string())
        }
    }
}

impl PluginOwner for DefaultPluginOwner {
    #[cfg(feature = "sql")]
    type Row = crate::repo::plugin::DefaultPluginOwnerRow;

    fn account_id(&self) -> AccountId {
        AccountId::placeholder()
    }
}

impl Serialize for DefaultPluginOwner {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = serializer.serialize_struct("DefaultPluginOwner", 0)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for DefaultPluginOwner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UnitVisitor;

        impl<'de> Visitor<'de> for UnitVisitor {
            type Value = DefaultPluginOwner;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("struct DefaultPluginOwner")
            }

            fn visit_map<A>(self, _map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                Ok(DefaultPluginOwner)
            }
        }

        deserializer.deserialize_struct("DefaultPluginOwner", &[], UnitVisitor)
    }
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

#[async_trait]
impl PluginScope for DefaultPluginScope {
    #[cfg(feature = "sql")]
    type Row = crate::repo::plugin::DefaultPluginScopeRow;

    type RequestContext = ();

    async fn accessible_scopes(&self, _context: ()) -> Result<Vec<Self>, String> {
        Ok(match self {
            DefaultPluginScope::Global(_) => vec![self.clone()],
            DefaultPluginScope::Component(_) => vec![Self::global(), self.clone()],
        })
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
    type Row = crate::repo::plugin_installation::ComponentPluginInstallationRow;

    #[cfg(feature = "sql")]
    fn table_name() -> &'static str {
        "component_plugin_installation"
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::plugin::{
        AppPluginDefinition, ComponentTransformerDefinition, DefaultPluginOwner,
        DefaultPluginScope, LibraryPluginDefinition, OplogProcessorDefinition, PluginDefinition,
        PluginInstallation, PluginTypeSpecificDefinition, PluginWasmFileKey,
    };

    impl From<DefaultPluginScope> for golem_api_grpc::proto::golem::component::DefaultPluginScope {
        fn from(scope: DefaultPluginScope) -> Self {
            match scope {
                DefaultPluginScope::Global(_) => golem_api_grpc::proto::golem::component::DefaultPluginScope {
                    scope: Some(golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Global(
                        golem_api_grpc::proto::golem::common::Empty {},
                    )),
                },
                DefaultPluginScope::Component(scope) => golem_api_grpc::proto::golem::component::DefaultPluginScope {
                    scope: Some(golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Component(
                        golem_api_grpc::proto::golem::component::ComponentPluginScope {
                            component_id: Some(scope.component_id.into()),
                        },
                    )),
                },
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::DefaultPluginScope> for DefaultPluginScope {
        type Error = String;

        fn try_from(
            proto: golem_api_grpc::proto::golem::component::DefaultPluginScope,
        ) -> Result<Self, Self::Error> {
            match proto.scope {
                Some(
                    golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Global(_),
                ) => Ok(Self::global()),
                Some(
                    golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Component(
                        proto,
                    ),
                ) => Ok(Self::component(
                    proto
                        .component_id
                        .ok_or("Missing component_id".to_string())?
                        .try_into()?,
                )),
                None => Err("Missing scope".to_string()),
            }
        }
    }

    impl From<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>
        for golem_api_grpc::proto::golem::component::PluginDefinition
    {
        fn from(value: PluginDefinition<DefaultPluginOwner, DefaultPluginScope>) -> Self {
            golem_api_grpc::proto::golem::component::PluginDefinition {
                id: Some(value.id.into()),
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                specs: Some(value.specs.into()),
                scope: Some(value.scope.into()),
                deleted: value.deleted,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginDefinition>
        for PluginDefinition<DefaultPluginOwner, DefaultPluginScope>
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::PluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(PluginDefinition {
                id: value.id.ok_or("Missing plugin id")?.try_into()?,
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
                scope: value.scope.ok_or("Missing plugin scope")?.try_into()?,
                owner: DefaultPluginOwner,
                deleted: value.deleted,
            })
        }
    }

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

#[cfg(test)]
mod tests {
    use crate::model::plugin::DefaultPluginOwner;
    use poem_openapi::types::ToJSON;
    use test_r::test;

    #[test]
    fn default_plugin_owner_serialization_poem_serde_equivalence() {
        let owner = DefaultPluginOwner;
        let serialized = owner.to_json_string();
        let deserialized: DefaultPluginOwner = serde_json::from_str(&serialized).unwrap();
        assert_eq!(owner, deserialized);
    }
}
