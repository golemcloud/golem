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

use crate::model::plugin::ComponentPluginInstallationTarget;
use crate::model::ComponentId;
use crate::repo::RowMeta;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, QueryBuilder};
use std::fmt::Display;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentPluginInstallationRow {
    pub component_id: Uuid,
    pub component_version: i64,
}

impl Display for ComponentPluginInstallationRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.component_id, self.component_version)
    }
}

impl<DB: Database> RowMeta<DB> for ComponentPluginInstallationRow
where
    Uuid: for<'q> Encode<'q, DB> + sqlx::Type<DB>,
    i64: for<'q> Encode<'q, DB> + sqlx::Type<DB>,
{
    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("component_id");
        builder.push("component_version");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        builder.push("component_id = ");
        builder.push_bind(self.component_id);
        builder.push(" AND component_version = ");
        builder.push_bind(self.component_version);
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.component_id);
        builder.push_bind(self.component_version);
    }
}

impl TryFrom<ComponentPluginInstallationRow> for ComponentPluginInstallationTarget {
    type Error = String;

    fn try_from(value: ComponentPluginInstallationRow) -> Result<Self, Self::Error> {
        Ok(ComponentPluginInstallationTarget {
            component_id: ComponentId(value.component_id),
            component_version: value.component_version as u64,
        })
    }
}

impl From<ComponentPluginInstallationTarget> for ComponentPluginInstallationRow {
    fn from(value: ComponentPluginInstallationTarget) -> Self {
        ComponentPluginInstallationRow {
            component_id: value.component_id.0,
            component_version: value.component_version as i64,
        }
    }
}
