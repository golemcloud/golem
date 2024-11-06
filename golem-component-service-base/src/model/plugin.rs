// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::repo::plugin_installation::PluginInstallationRecord;
use crate::repo::RowMeta;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_common::model::{ComponentId, ComponentVersion, PluginInstallationId};
use golem_service_base::auth::DefaultNamespace;
use http::Uri;
use poem_openapi::types::{ParseFromJSON, ToJSON, Type};
use poem_openapi::{Enum, Object, Union};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Postgres, Sqlite};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentTransformerDefinition {
    pub provided_wit_package: Option<String>,
    pub json_schema: Option<String>,
    pub validate_url: String,
    pub transform_url: String,
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

    /// Gets all the plugin scopes valid for this given scope
    fn accessible_scopes(&self) -> Vec<Self>;
}

impl PluginScope for DefaultPluginScope {
    type Row = crate::repo::plugin::DefaultPluginScopeRow;

    fn accessible_scopes(&self) -> Vec<Self> {
        match self {
            DefaultPluginScope::Global(_) => vec![self.clone()],
            DefaultPluginScope::Component(_) => vec![Self::global(), self.clone()],
        }
    }
}

pub trait PluginOwner:
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
    type Row: RowMeta<Sqlite>
        + RowMeta<Postgres>
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

    // Corresponding Namespace type for component services
    type Namespace: Send + Sync + 'static;
}

impl PluginOwner for DefaultPluginOwner {
    type Row = crate::repo::plugin::DefaultPluginOwnerRow;
    type Namespace = DefaultNamespace;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub id: PluginInstallationId,
    pub name: String,
    pub version: String,
    pub priority: i16,
    pub parameters: HashMap<String, String>,
}

impl PluginInstallation {
    pub fn try_into<Owner: PluginOwner, Target: PluginInstallationTarget>(
        self,
        owner: Owner::Row,
        target: Target::Row,
    ) -> Result<PluginInstallationRecord<Owner, Target>, String> {
        PluginInstallationRecord::try_from(self, owner, target)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PluginInstallationCreation {
    pub name: String,
    pub version: String,
    pub priority: i16,
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
    pub priority: i16,
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
    type Row: RowMeta<Sqlite>
        + RowMeta<Postgres>
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
