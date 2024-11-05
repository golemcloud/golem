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

use crate::model::{
    ComponentPluginInstallationTarget, PluginInstallation, PluginInstallationTarget, PluginOwner,
};
use crate::repo::RowMeta;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::{ComponentId, PluginInstallationId};
use golem_service_base::repo::RepoError;
use poem_openapi::__private::serde_json;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, Pool, QueryBuilder};
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PluginInstallationRecord<Owner: PluginOwner, Target: PluginInstallationTarget> {
    pub installation_id: Uuid,
    pub plugin_name: String,
    pub plugin_version: String,
    pub priority: i16,
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
            plugin_name: installation.name,
            plugin_version: installation.version,
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
            name: value.plugin_name,
            version: value.plugin_version,
            priority: value.priority,
            parameters,
        })
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentPluginInstallationRow {
    pub component_id: Uuid,
    pub component_version: i64,
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

#[async_trait]
pub trait PluginInstallationRepo<Owner: PluginOwner, Target: PluginInstallationTarget> {
    async fn get_all(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
    ) -> Result<Vec<PluginInstallationRecord<Owner, Target>>, RepoError>;

    async fn create(
        &self,
        record: &PluginInstallationRecord<Owner, Target>,
    ) -> Result<(), RepoError>;

    async fn update(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
        id: &Uuid,
        new_priority: i16,
        new_parameters: Vec<u8>,
    ) -> Result<(), RepoError>;

    async fn delete(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
        id: &Uuid,
    ) -> Result<(), RepoError>;
}

pub struct LoggedPluginInstallationRepo<
    Owner: PluginOwner,
    Target: PluginInstallationTarget,
    Repo: PluginInstallationRepo<Owner, Target>,
> {
    repo: Repo,
    _owner: PhantomData<Owner>,
    _target: PhantomData<Target>,
}

impl<
        Owner: PluginOwner,
        Target: PluginInstallationTarget,
        Repo: PluginInstallationRepo<Owner, Target>,
    > LoggedPluginInstallationRepo<Owner, Target, Repo>
{
    pub fn new(repo: Repo) -> Self {
        Self {
            repo,
            _owner: PhantomData,
            _target: PhantomData,
        }
    }

    fn logged<R>(message: &'static str, result: Result<R, RepoError>) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!("{}", message),
            Err(error) => error!(error = error.to_string(), "{message}"),
        }
        result
    }
}

#[async_trait]
impl<
        Owner: PluginOwner,
        Target: PluginInstallationTarget,
        Repo: PluginInstallationRepo<Owner, Target> + Sync,
    > PluginInstallationRepo<Owner, Target> for LoggedPluginInstallationRepo<Owner, Target, Repo>
{
    async fn get_all(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
    ) -> Result<Vec<PluginInstallationRecord<Owner, Target>>, RepoError> {
        let result = self.repo.get_all(owner, target).await;
        Self::logged("get_all", result)
    }

    async fn create(
        &self,
        record: &PluginInstallationRecord<Owner, Target>,
    ) -> Result<(), RepoError> {
        let result = self.repo.create(record).await;
        Self::logged("create", result)
    }

    async fn update(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
        id: &Uuid,
        new_priority: i16,
        new_parameters: Vec<u8>,
    ) -> Result<(), RepoError> {
        let result = self
            .repo
            .update(owner, target, id, new_priority, new_parameters)
            .await;
        Self::logged("update", result)
    }

    async fn delete(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
        id: &Uuid,
    ) -> Result<(), RepoError> {
        let result = self.repo.delete(owner, target, id).await;
        Self::logged("delete", result)
    }
}

pub struct DbPluginInstallationRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbPluginInstallationRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
#[async_trait]
impl<Owner: PluginOwner, Target: PluginInstallationTarget> PluginInstallationRepo<Owner, Target>
    for DbPluginInstallationRepo<sqlx::Postgres>
{
    async fn get_all(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
    ) -> Result<Vec<PluginInstallationRecord<Owner, Target>>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("installation_id");
        column_list.push("plugin_name");
        column_list.push("plugin_version");
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

        debug!("Generated query for get_all: {}", query.sql());

        Ok(query
            .build_query_as::<PluginInstallationRecord<Owner, Target>>()
            .fetch_all(self.db_pool.deref())
            .await?)
    }

    async fn create(
        &self,
        record: &PluginInstallationRecord<Owner, Target>,
    ) -> Result<(), RepoError> {
        let mut query = QueryBuilder::new("INSERT INTO ");
        query.push(Target::table_name());
        query.push(" (");

        let mut column_list = query.separated(", ");

        column_list.push("installation_id");
        column_list.push("plugin_name");
        column_list.push("plugin_version");
        column_list.push("priority");
        column_list.push("parameters");

        Target::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        query.push(") VALUES (");

        let mut value_list = query.separated(", ");
        value_list.push_bind(record.installation_id);
        value_list.push_bind(&record.plugin_name);
        value_list.push_bind(&record.plugin_version);
        value_list.push_bind(record.priority);
        value_list.push_bind(&record.parameters);
        record.target.push_bind(&mut value_list);
        record.owner.push_bind(&mut value_list);
        query.push(")");

        debug!("Generated query for create: {}", query.sql());

        query.build().execute(self.db_pool.deref()).await?;

        Ok(())
    }

    async fn update(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
        id: &Uuid,
        new_priority: i16,
        new_parameters: Vec<u8>,
    ) -> Result<(), RepoError> {
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

        debug!("Generated query for update: {}", query.sql());

        query.build().execute(self.db_pool.deref()).await?;

        Ok(())
    }

    async fn delete(
        &self,
        owner: &Owner::Row,
        target: &Target::Row,
        id: &Uuid,
    ) -> Result<(), RepoError> {
        let mut query = QueryBuilder::new("DELETE FROM ");
        query.push(Target::table_name());
        query.push(" WHERE installation_id = ");
        query.push_bind(id);
        query.push(" AND ");
        owner.add_where_clause(&mut query);
        query.push(" AND ");
        target.add_where_clause(&mut query);

        debug!("Generated query for delete: {}", query.sql());

        query.build().execute(self.db_pool.deref()).await?;

        Ok(())
    }
}
