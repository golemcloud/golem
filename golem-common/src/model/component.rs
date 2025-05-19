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

use crate::base_model::{ComponentId, ComponentVersion};
use crate::model::plugin::{DefaultPluginOwner, PluginOwner};
use crate::model::{AccountId, PoemTypeRequirements};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use std::str::FromStr;

pub trait ComponentOwner:
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
        + Into<<Self::PluginOwner as PluginOwner>::Row>
        + Clone
        + Display
        + Send
        + Sync
        + Unpin
        + 'static;

    type PluginOwner: PluginOwner + From<Self>;

    fn account_id(&self) -> AccountId;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DefaultComponentOwner;

impl Display for DefaultComponentOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "default")
    }
}

impl FromStr for DefaultComponentOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "default" {
            Ok(DefaultComponentOwner)
        } else {
            Err("Failed to parse empty namespace".to_string())
        }
    }
}

impl From<DefaultComponentOwner> for DefaultPluginOwner {
    fn from(_value: DefaultComponentOwner) -> Self {
        DefaultPluginOwner
    }
}

impl ComponentOwner for DefaultComponentOwner {
    #[cfg(feature = "sql")]
    type Row = crate::repo::component::DefaultComponentOwnerRow;
    type PluginOwner = DefaultPluginOwner;

    fn account_id(&self) -> AccountId {
        AccountId::placeholder()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]

pub struct VersionedComponentId {
    pub component_id: ComponentId,
    pub version: ComponentVersion,
}

impl Display for VersionedComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.component_id, self.version)
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::component::VersionedComponentId;

    impl TryFrom<golem_api_grpc::proto::golem::component::VersionedComponentId>
        for VersionedComponentId
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::VersionedComponentId,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: value
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                version: value.version,
            })
        }
    }

    impl From<VersionedComponentId> for golem_api_grpc::proto::golem::component::VersionedComponentId {
        fn from(value: VersionedComponentId) -> Self {
            Self {
                component_id: Some(value.component_id.into()),
                version: value.version,
            }
        }
    }
}
