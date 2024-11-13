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

use crate::model::{ComponentOwner, DefaultComponentOwner};
use crate::repo::plugin_installation::PluginInstallationRecord;
use crate::repo::RowMeta;
use golem_common::model::plugin::DefaultPluginScope;
use golem_common::model::{ComponentId, ComponentVersion, PluginInstallationId};
use http::Uri;
use poem_openapi::types::{ParseFromJSON, ToJSON, Type};
use poem_openapi::{Enum, Object, Union};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Postgres, Sqlite};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Serialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PluginDefinition<Owner: ComponentOwner, Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificDefinition,
    pub scope: Scope,
    pub owner: Owner,
}

impl From<PluginDefinition<DefaultComponentOwner, DefaultPluginScope>>
    for golem_api_grpc::proto::golem::component::PluginDefinition
{
    fn from(value: PluginDefinition<DefaultComponentOwner, DefaultPluginScope>) -> Self {
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
    for PluginDefinition<DefaultComponentOwner, DefaultPluginScope>
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
            owner: DefaultComponentOwner,
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
    pub fn with_owner<Owner: ComponentOwner>(self, owner: Owner) -> PluginDefinition<Owner, Scope> {
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
    async fn accessible_scopes(&self, context: &Self::RequestContext) -> Vec<Self>;
}

#[async_trait]
impl PluginScope for DefaultPluginScope {
    type Row = crate::repo::plugin::DefaultPluginScopeRow;

    type RequestContext = ();

    async fn accessible_scopes(&self, _context: &()) -> Vec<Self> {
        match self {
            DefaultPluginScope::Global(_) => vec![self.clone()],
            DefaultPluginScope::Component(_) => vec![Self::global(), self.clone()],
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

impl PluginInstallation {
    pub fn try_into<Owner: ComponentOwner, Target: PluginInstallationTarget>(
        self,
        owner: Owner::Row,
        target: Target::Row,
    ) -> Result<PluginInstallationRecord<Owner, Target>, String> {
        PluginInstallationRecord::try_from(self, owner, target)
    }
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
