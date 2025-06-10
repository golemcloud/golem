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

use super::plugin::PluginOwnerRow;
use crate::model::component::ComponentOwner;
use crate::model::{AccountId, ProjectId};
use crate::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder, Type};
use std::fmt::Display;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentOwnerRow {
    pub account_id: String,
    pub project_id: Uuid,
}

impl Display for ComponentOwnerRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.account_id, self.project_id)
    }
}

impl From<ComponentOwner> for ComponentOwnerRow {
    fn from(owner: ComponentOwner) -> Self {
        Self {
            account_id: owner.account_id.value,
            project_id: owner.project_id.0,
        }
    }
}

impl TryFrom<ComponentOwnerRow> for ComponentOwner {
    type Error = String;

    fn try_from(value: ComponentOwnerRow) -> Result<Self, Self::Error> {
        Ok(Self {
            account_id: AccountId {
                value: value.account_id,
            },
            project_id: ProjectId(value.project_id),
        })
    }
}

impl<DB: Database> RowMeta<DB> for ComponentOwnerRow
where
    String: for<'q> Encode<'q, DB> + Type<DB>,
{
    // NOTE: We could store account_id and project_id in separate columns, but this abstraction was
    // introduced when the `components` table already used the generic "namespace" column so
    // we need to keep that to be able to join the tables.

    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("namespace");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        builder.push("namespace = ");
        let namespace_string = self.to_string();
        builder.push_bind(namespace_string);
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.to_string());
    }
}

impl From<ComponentOwnerRow> for PluginOwnerRow {
    fn from(value: ComponentOwnerRow) -> Self {
        PluginOwnerRow {
            account_id: value.account_id,
        }
    }
}
