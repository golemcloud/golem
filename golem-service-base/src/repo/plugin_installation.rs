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

use conditional_trait_gen::trait_gen;
use golem_common::model::plugin::PluginOwner;
use golem_common::model::plugin::{PluginInstallation, PluginInstallationTarget};
use golem_common::model::{PluginId, PluginInstallationId};
use golem_common::repo::RowMeta;
use sqlx::{Database, QueryBuilder};
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;
use tracing::debug;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PluginInstallationRecord<Owner: PluginOwner, Target: PluginInstallationTarget> {
    pub installation_id: Uuid,
    pub plugin_id: Uuid,
    pub priority: i32,
    pub parameters: Vec<u8>,
    #[sqlx(flatten)]
    pub target: Target::Row,
    #[sqlx(flatten)]
    pub owner: Owner::Row,
}

impl<Owner: PluginOwner, Target: PluginInstallationTarget> PluginInstallationRecord<Owner, Target> {
    pub fn try_from(
        installation: PluginInstallation,
        owner: Owner::Row,
        target: Target::Row,
    ) -> Result<Self, String> {
        Ok(PluginInstallationRecord {
            installation_id: installation.id.0,
            plugin_id: installation.plugin_id.0,
            priority: installation.priority,
            parameters: serde_json::to_vec(&installation.parameters)
                .map_err(|e| format!("Failed to serialize plugin installation parameters: {e}"))?,
            target,
            owner,
        })
    }
}

impl<Owner: PluginOwner, Target: PluginInstallationTarget>
    TryFrom<PluginInstallationRecord<Owner, Target>> for PluginInstallation
{
    type Error = String;

    fn try_from(value: PluginInstallationRecord<Owner, Target>) -> Result<Self, Self::Error> {
        let parameters: HashMap<String, String> = serde_json::from_str(
            std::str::from_utf8(&value.parameters).map_err(|e| e.to_string())?,
        )
        .map_err(|err| {
            format!("Invalid parameter JSON in component plugin installation record: {err}")
        })?;

        Ok(PluginInstallation {
            id: PluginInstallationId(value.installation_id),
            plugin_id: PluginId(value.plugin_id),
            priority: value.priority,
            parameters,
        })
    }
}

/// Interface for generating the queries for a plugin installation repo - it is not the interface
/// for the actual repo, as for components (or any other plugin installation target that requires
/// immutable installations) the actual implementation needs to be in transactional context of
/// the target repo.
pub trait PluginInstallationRepoQueries<
    DB: Database,
    Owner: PluginOwner,
    Target: PluginInstallationTarget,
>
{
    fn get_all<'a>(&self, owner: &'a Owner::Row, target: &'a Target::Row) -> QueryBuilder<'a, DB>;

    fn create<'a>(
        &self,
        record: &'a PluginInstallationRecord<Owner, Target>,
    ) -> QueryBuilder<'a, DB>;

    fn update<'a>(
        &self,
        owner: &'a Owner::Row,
        target: &'a Target::Row,
        id: &'a Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> QueryBuilder<'a, DB>;

    fn delete<'a>(
        &self,
        owner: &'a Owner::Row,
        target: &'a Target::Row,
        id: &'a Uuid,
    ) -> QueryBuilder<'a, DB>;
}

pub struct DbPluginInstallationRepoQueries<DB: Database> {
    _db: PhantomData<DB>,
}

impl<DB: Database> Default for DbPluginInstallationRepoQueries<DB> {
    fn default() -> Self {
        Self::new()
    }
}

impl<DB: Database> DbPluginInstallationRepoQueries<DB> {
    pub fn new() -> Self {
        Self { _db: PhantomData }
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
impl<Owner: PluginOwner, Target: PluginInstallationTarget>
    PluginInstallationRepoQueries<sqlx::Postgres, Owner, Target>
    for DbPluginInstallationRepoQueries<sqlx::Postgres>
{
    fn get_all<'a>(
        &self,
        owner: &'a Owner::Row,
        target: &'a Target::Row,
    ) -> QueryBuilder<'a, sqlx::Postgres> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("installation_id");
        column_list.push("plugin_id");
        column_list.push("priority");
        column_list.push("parameters");

        Target::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        query.push(" FROM ");
        query.push(Target::table_name());
        query.push(" WHERE ");
        target.add_where_clause(&mut query);
        query.push(" AND ");
        owner.add_where_clause(&mut query);

        debug!(
            plugin_owner = display(owner),
            plugin_target = display(target),
            sql = query.sql(),
            "Generated query for get_all",
        );

        query
    }

    fn create<'a>(
        &self,
        record: &'a PluginInstallationRecord<Owner, Target>,
    ) -> QueryBuilder<'a, sqlx::Postgres> {
        let mut query = QueryBuilder::new("INSERT INTO ");
        query.push(Target::table_name());
        query.push(" (");

        let mut column_list = query.separated(", ");

        column_list.push("installation_id");
        column_list.push("plugin_id");
        column_list.push("priority");
        column_list.push("parameters");

        Target::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        query.push(") VALUES (");

        let mut value_list = query.separated(", ");
        value_list.push_bind(record.installation_id);
        value_list.push_bind(record.plugin_id);
        value_list.push_bind(record.priority);
        value_list.push_bind(&record.parameters);
        record.target.push_bind(&mut value_list);
        record.owner.push_bind(&mut value_list);
        query.push(")");

        debug!(
            plugin_owner = display(&record.owner),
            plugin_target = display(&record.target),
            installation_id = display(&record.installation_id),
            sql = query.sql(),
            "Generated query for create",
        );

        query
    }

    fn update<'a>(
        &self,
        owner: &'a Owner::Row,
        target: &'a Target::Row,
        id: &'a Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> QueryBuilder<'a, sqlx::Postgres> {
        let mut query = QueryBuilder::new("UPDATE ");
        query.push(Target::table_name());
        query.push(" SET priority = ");
        query.push_bind(new_priority);
        query.push(", parameters = ");
        query.push_bind(new_parameters);
        query.push(" WHERE installation_id = ");
        query.push_bind(id);
        query.push(" AND ");
        owner.add_where_clause(&mut query);
        query.push(" AND ");
        target.add_where_clause(&mut query);

        debug!(
            plugin_owner = display(owner),
            plugin_target = display(target),
            installation_id = display(id),
            sql = query.sql(),
            "Generated query for update"
        );

        query
    }

    fn delete<'a>(
        &self,
        owner: &'a Owner::Row,
        target: &'a Target::Row,
        id: &'a Uuid,
    ) -> QueryBuilder<'a, sqlx::Postgres> {
        let mut query = QueryBuilder::new("DELETE FROM ");
        query.push(Target::table_name());
        query.push(" WHERE installation_id = ");
        query.push_bind(id);
        query.push(" AND ");
        owner.add_where_clause(&mut query);
        query.push(" AND ");
        target.add_where_clause(&mut query);

        debug!(
            plugin_owner = display(owner),
            plugin_target = display(target),
            installation_id = display(id),
            sql = query.sql(),
            "Generated query for delete"
        );

        query
    }
}
