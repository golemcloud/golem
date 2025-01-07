// Copyright 2024-2025 Golem Cloud
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

use crate::model::plugin::{DefaultPluginOwner, PluginOwner};
use crate::model::{AccountId, HasAccountId, PoemTypeRequirements};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use std::str::FromStr;

pub trait ComponentOwner:
    Debug
    + Display
    + FromStr<Err = String>
    + HasAccountId
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

impl HasAccountId for DefaultComponentOwner {
    fn account_id(&self) -> AccountId {
        AccountId::placeholder()
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
}
