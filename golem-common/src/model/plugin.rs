use crate::model::{AccountId, ComponentId, Empty, HasAccountId, PluginInstallationId};
use crate::repo::RowMeta;
use poem_openapi::types::{
    ParseError, ParseFromJSON, ParseFromParameter, ParseResult, ToJSON, Type,
};
use poem_openapi::{Object, Union};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
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
}

impl Default for DefaultPluginScope {
    fn default() -> Self {
        DefaultPluginScope::global()
    }
}

impl Display for DefaultPluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
