use crate::model::{
    AccountId, ComponentId, ComponentVersion, Empty, HasAccountId, PluginInstallationId,
};
use crate::repo::RowMeta;
use async_trait::async_trait;
use http_02::Uri;
use poem_openapi::types::{
    ParseError, ParseFromJSON, ParseFromParameter, ParseResult, ToJSON, Type,
};
use poem_openapi::{Enum, Object, Union};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Postgres, Sqlite};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentPluginScope {
    pub component_id: ComponentId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
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

impl ParseFromParameter for DefaultPluginScope {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        if value == "global" {
            Ok(Self::global())
        } else if let Some(id_part) = value.strip_prefix("component:") {
            let component_id = ComponentId::try_from(id_part);
            match component_id {
                Ok(component_id) => Ok(Self::component(component_id)),
                Err(err) => Err(ParseError::<Self>::custom(err)),
            }
        } else {
            Err(ParseError::<Self>::custom("Unexpected representation of plugin scope - must be 'global' or 'component:<component_id>'".to_string()))
        }
    }
}

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
            Some(golem_api_grpc::proto::golem::component::default_plugin_scope::Scope::Global(
                _,
            )) => Ok(Self::global()),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub name: String,
    pub version: String,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

impl From<PluginInstallation> for golem_api_grpc::proto::golem::component::PluginInstallation {
    fn from(plugin_installation: PluginInstallation) -> Self {
        golem_api_grpc::proto::golem::component::PluginInstallation {
            id: Some(plugin_installation.id.into()),
            name: plugin_installation.name,
            version: plugin_installation.version,
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
            name: proto.name,
            version: proto.version,
            priority: proto.priority,
            parameters: proto.parameters,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PluginInstallationCreation {
    pub name: String,
    pub version: String,
    pub priority: i32,
    pub parameters: HashMap<String, String>,
}

impl PluginInstallationCreation {
    pub fn with_generated_id(self) -> PluginInstallation {
        PluginInstallation {
            id: PluginInstallationId::new_v4(),
            name: self.name,
            version: self.version,
            priority: self.priority,
            parameters: self.parameters,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct PluginInstallationUpdate {
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
    + Type
    + ParseFromJSON
    + ToJSON
    + Send
    + Sync
    + 'static
{
    type Row: RowMeta<sqlx::Sqlite>
        + RowMeta<sqlx::Postgres>
        + for<'r> sqlx::FromRow<'r, SqliteRow>
        + for<'r> sqlx::FromRow<'r, PgRow>
        + From<Self>
        + TryInto<Self, Error = String>
        + Clone
        + Display
        + Send
        + Sync
        + Unpin
        + 'static;

    fn table_name() -> &'static str;
}

pub trait PluginOwner:
    Debug
    + Display
    + FromStr<Err = String>
    + HasAccountId
    + Clone
    + PartialEq
    + Serialize
    + for<'de> Deserialize<'de>
    + Type
    + ParseFromJSON
    + ToJSON
    + Send
    + Sync
    + 'static
{
    type Row: RowMeta<sqlx::Sqlite>
        + RowMeta<sqlx::Postgres>
        + for<'r> sqlx::FromRow<'r, SqliteRow>
        + for<'r> sqlx::FromRow<'r, PgRow>
        + From<Self>
        + TryInto<Self, Error = String>
        + Clone
        + Debug
        + Display
        + Send
        + Sync
        + Unpin
        + 'static;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
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

impl HasAccountId for DefaultPluginOwner {
    fn account_id(&self) -> AccountId {
        AccountId::placeholder()
    }
}

impl PluginOwner for DefaultPluginOwner {
    type Row = crate::repo::plugin::DefaultPluginOwnerRow;
}

#[derive(Debug, Clone, PartialEq, Serialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
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

impl From<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>
    for golem_api_grpc::proto::golem::component::PluginDefinition
{
    fn from(value: PluginDefinition<DefaultPluginOwner, DefaultPluginScope>) -> Self {
        golem_api_grpc::proto::golem::component::PluginDefinition {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: Some(value.specs.into()),
            scope: Some(value.scope.into()),
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
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
            scope: value.scope.ok_or("Missing plugin scope")?.try_into()?,
            owner: DefaultPluginOwner,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PluginDefinitionWithoutOwner<Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificDefinition,
    pub scope: Scope,
}

impl<Scope: PluginScope> PluginDefinitionWithoutOwner<Scope> {
    pub fn with_owner<Owner: PluginOwner>(self, owner: Owner) -> PluginDefinition<Owner, Scope> {
        PluginDefinition {
            name: self.name,
            version: self.version,
            description: self.description,
            icon: self.icon,
            homepage: self.homepage,
            specs: self.specs,
            scope: self.scope,
            owner,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Enum)]
#[repr(i8)]
pub enum PluginType {
    ComponentTransformer = 0,
    OplogProcessor = 1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PluginTypeSpecificDefinition {
    ComponentTransformer(ComponentTransformerDefinition),
    OplogProcessor(OplogProcessorDefinition),
}

impl PluginTypeSpecificDefinition {
    pub fn plugin_type(&self) -> PluginType {
        match self {
            PluginTypeSpecificDefinition::ComponentTransformer(_) => {
                PluginType::ComponentTransformer
            }
            PluginTypeSpecificDefinition::OplogProcessor(_) => PluginType::OplogProcessor,
        }
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentTransformerDefinition {
    pub provided_wit_package: Option<String>,
    pub json_schema: Option<String>,
    pub validate_url: String,
    pub transform_url: String,
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

impl ComponentTransformerDefinition {
    pub fn validate_url(&self) -> &Uri {
        todo!()
    }

    pub fn transform_url(&self) -> &Uri {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct OplogProcessorDefinition {
    pub component_id: ComponentId,
    pub component_version: ComponentVersion,
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

#[async_trait]
pub trait PluginScope:
    Debug
    + Clone
    + PartialEq
    + Serialize
    + for<'de> Deserialize<'de>
    + Type
    + ParseFromJSON
    + ToJSON
    + Send
    + Sync
    + 'static
{
    type Row: RowMeta<Sqlite>
        + RowMeta<Postgres>
        + for<'r> sqlx::FromRow<'r, SqliteRow>
        + for<'r> sqlx::FromRow<'r, PgRow>
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
    type Row = crate::repo::plugin::DefaultPluginScopeRow;

    type RequestContext = ();

    async fn accessible_scopes(&self, _context: ()) -> Result<Vec<Self>, String> {
        Ok(match self {
            DefaultPluginScope::Global(_) => vec![self.clone()],
            DefaultPluginScope::Component(_) => vec![Self::global(), self.clone()],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
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
    type Row = crate::repo::plugin_installation::ComponentPluginInstallationRow;

    fn table_name() -> &'static str {
        "component_plugin_installation"
    }
}
