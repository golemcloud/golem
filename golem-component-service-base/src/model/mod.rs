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

mod component;
mod plugin;

use crate::repo::RowMeta;
pub use component::{Component, ComponentConstraints};
pub use plugin::*;
use poem_openapi::types::{ParseFromJSON, ToJSON, Type};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{Postgres, Sqlite};
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
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

impl ComponentOwner for DefaultComponentOwner {
    type Row = crate::repo::plugin::DefaultComponentOwnerRow;
}
