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
    ComponentTransformerDefinition, OplogProcessorDefinition, PluginDefinition, PluginOwner,
    PluginScope, PluginTypeSpecificDefinition,
};
use crate::repo::RowMeta;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::plugin::{ComponentPluginScope, DefaultPluginOwner, DefaultPluginScope};
use golem_common::model::ComponentId;
use golem_common::model::Empty;
use golem_service_base::repo::RepoError;
use sqlx::query_builder::Separated;
use sqlx::{Database, Encode, Pool, QueryBuilder, Type};
use std::fmt::Display;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PluginRecord<Owner: PluginOwner, Scope: PluginScope> {
    name: String,
    version: String,
    description: String,
    icon: Vec<u8>,
    homepage: String,
    plugin_type: i16,
    #[sqlx(flatten)]
    scope: Scope::Row,
    #[sqlx(flatten)]
    owner: Owner::Row,

    // for ComponentTransformer plugin type
    provided_wit_package: Option<String>,
    json_schema: Option<String>,
    validate_url: Option<String>,
    transform_url: Option<String>,

    // for OplogProcessor plugin type
    component_id: Option<Uuid>,
    component_version: Option<i64>,

    #[allow(dead_code)] deleted: bool
}

impl<Owner: PluginOwner, Scope: PluginScope> From<PluginDefinition<Owner, Scope>>
    for PluginRecord<Owner, Scope>
{
    fn from(value: PluginDefinition<Owner, Scope>) -> Self {
        Self {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            plugin_type: value.specs.plugin_type() as i16,
            scope: value.scope.into(),
            owner: value.owner.into(),

            provided_wit_package: match &value.specs {
                PluginTypeSpecificDefinition::ComponentTransformer(def) => {
                    def.provided_wit_package.clone()
                }
                _ => None,
            },
            json_schema: match &value.specs {
                PluginTypeSpecificDefinition::ComponentTransformer(def) => def.json_schema.clone(),
                _ => None,
            },
            validate_url: match &value.specs {
                PluginTypeSpecificDefinition::ComponentTransformer(def) => {
                    Some(def.validate_url.clone())
                }
                _ => None,
            },
            transform_url: match &value.specs {
                PluginTypeSpecificDefinition::ComponentTransformer(def) => {
                    Some(def.transform_url.clone())
                }
                _ => None,
            },

            component_id: match &value.specs {
                PluginTypeSpecificDefinition::OplogProcessor(def) => Some(def.component_id.0),
                _ => None,
            },
            component_version: match &value.specs {
                PluginTypeSpecificDefinition::OplogProcessor(def) => {
                    Some(def.component_version as i64)
                }
                _ => None,
            },
            deleted: false
        }
    }
}

impl<Owner: PluginOwner, Scope: PluginScope> TryFrom<PluginRecord<Owner, Scope>>
    for PluginDefinition<Owner, Scope>
{
    type Error = String;

    fn try_from(value: PluginRecord<Owner, Scope>) -> Result<Self, Self::Error> {
        let specs = match value.plugin_type {
            0 => {
                PluginTypeSpecificDefinition::ComponentTransformer(ComponentTransformerDefinition {
                    provided_wit_package: value.provided_wit_package,
                    json_schema: value.json_schema,
                    validate_url: value
                        .validate_url
                        .ok_or("validate_url is required for ComponentTransformer rows")?,
                    transform_url: value
                        .transform_url
                        .ok_or("transform_url is required for ComponentTransformer rows")?,
                })
            }
            1 => PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
                component_id: ComponentId(
                    value
                        .component_id
                        .ok_or("component_id is required for OplogProcessor rows")?,
                ),
                component_version: value
                    .component_version
                    .map(|i| i as u64)
                    .ok_or("component_version is required for OplogProcessor rows")?,
            }),
            other => return Err(format!("Invalid plugin type: {other}")),
        };

        Ok(PluginDefinition {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs,
            scope: value.scope.try_into()?,
            owner: value.owner.try_into()?,
        })
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct DefaultPluginScopeRow {
    scope_component_id: Option<Uuid>,
}

impl From<DefaultPluginScope> for DefaultPluginScopeRow {
    fn from(value: DefaultPluginScope) -> Self {
        match value {
            DefaultPluginScope::Global(_) => Self {
                scope_component_id: None,
            },
            DefaultPluginScope::Component(component) => Self {
                scope_component_id: Some(component.component_id.0),
            },
        }
    }
}

impl TryFrom<DefaultPluginScopeRow> for DefaultPluginScope {
    type Error = String;

    fn try_from(value: DefaultPluginScopeRow) -> Result<Self, Self::Error> {
        match value.scope_component_id {
            Some(component_id) => Ok(DefaultPluginScope::Component(ComponentPluginScope {
                component_id: ComponentId(component_id),
            })),
            None => Ok(DefaultPluginScope::Global(Empty {})),
        }
    }
}

impl<DB: Database> RowMeta<DB> for DefaultPluginScopeRow
where
    Uuid: for<'q> Encode<'q, DB> + Type<DB>,
    Option<Uuid>: for<'q> Encode<'q, DB> + Type<DB>,
{
    fn add_column_list<Sep: Display>(builder: &mut Separated<DB, Sep>) {
        builder.push("scope_component_id");
    }

    fn add_where_clause<'a>(&'a self, builder: &mut QueryBuilder<'a, DB>) {
        if let Some(component_id) = &self.scope_component_id {
            builder.push("scope_component_id = ");
            builder.push_bind(component_id);
        } else {
            builder.push("scope_component_id IS NULL");
        }
    }

    fn push_bind<'a, Sep: Display>(&'a self, builder: &mut Separated<'_, 'a, DB, Sep>) {
        builder.push_bind(self.scope_component_id);
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct DefaultPluginOwnerRow {}

impl From<DefaultPluginOwner> for DefaultPluginOwnerRow {
    fn from(_: DefaultPluginOwner) -> Self {
        Self {}
    }
}

impl TryFrom<DefaultPluginOwnerRow> for DefaultPluginOwner {
    type Error = String;

    fn try_from(_: DefaultPluginOwnerRow) -> Result<Self, Self::Error> {
        Ok(DefaultPluginOwner {})
    }
}

impl<DB: Database> RowMeta<DB> for DefaultPluginOwnerRow {
    fn add_column_list<Sep: Display>(_builder: &mut Separated<DB, Sep>) {}

    fn add_where_clause(&self, builder: &mut QueryBuilder<DB>) {
        builder.push("1 = 1");
    }

    fn push_bind<'a, Sep: Display>(&'a self, _builder: &mut Separated<'_, 'a, DB, Sep>) {}
}

#[async_trait]
pub trait PluginRepo<Owner: PluginOwner, Scope: PluginScope> {
    async fn get_all(
        &self,
        owner: &Owner::Row,
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError>;

    async fn get_for_scope(
        &self,
        owner: &Owner::Row,
        scope: &[Scope::Row],
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError>;

    async fn get_all_with_name(
        &self,
        owner: &Owner::Row,
        name: &str,
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError>;

    async fn create(&self, record: &PluginRecord<Owner, Scope>) -> Result<(), RepoError>;

    async fn get(
        &self,
        owner: &Owner::Row,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginRecord<Owner, Scope>>, RepoError>;

    async fn delete(&self, owner: &Owner::Row, name: &str, version: &str) -> Result<(), RepoError>;
}

pub struct LoggedPluginRepo<Owner: PluginOwner, Scope: PluginScope, Repo: PluginRepo<Owner, Scope>>
{
    repo: Repo,
    _owner: PhantomData<Owner>,
    _scope: PhantomData<Scope>,
}

impl<Owner: PluginOwner, Scope: PluginScope, Repo: PluginRepo<Owner, Scope>>
    LoggedPluginRepo<Owner, Scope, Repo>
{
    pub fn new(repo: Repo) -> Self {
        Self {
            repo,
            _owner: PhantomData,
            _scope: PhantomData,
        }
    }

    fn logged<R>(message: &'static str, result: Result<R, RepoError>) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!("{}", message),
            Err(error) => error!(error = error.to_string(), "{message}"),
        }
        result
    }

    fn logged_with_name<R>(
        message: &'static str,
        plugin_name: &str,
        plugin_version: &str,
        result: Result<R, RepoError>,
    ) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!(
                plugin_name = plugin_name,
                plugin_version = plugin_version,
                "{}",
                message
            ),
            Err(error) => error!(
                plugin_name = plugin_name,
                plugin_version = plugin_version,
                error = error.to_string(),
                "{message}"
            ),
        }
        result
    }
}

#[async_trait]
impl<Owner: PluginOwner, Scope: PluginScope, Repo: PluginRepo<Owner, Scope> + Sync>
    PluginRepo<Owner, Scope> for LoggedPluginRepo<Owner, Scope, Repo>
{
    async fn get_all(
        &self,
        owner: &Owner::Row,
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError> {
        let result = self.repo.get_all(owner).await;
        Self::logged("get_all", result)
    }

    async fn get_for_scope(
        &self,
        owner: &Owner::Row,
        scope: &[Scope::Row],
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError> {
        let result = self.repo.get_for_scope(owner, scope).await;
        Self::logged("get_for_scope", result)
    }

    async fn get_all_with_name(
        &self,
        owner: &Owner::Row,
        name: &str,
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError> {
        let result = self.repo.get_all_with_name(owner, name).await;
        Self::logged_with_name("get_all_with_name", name, "*", result)
    }

    async fn create(&self, record: &PluginRecord<Owner, Scope>) -> Result<(), RepoError> {
        let result = self.repo.create(record).await;
        Self::logged_with_name("create", &record.name, &record.version, result)
    }

    async fn get(
        &self,
        owner: &Owner::Row,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginRecord<Owner, Scope>>, RepoError> {
        let result = self.repo.get(owner, name, version).await;
        Self::logged_with_name("get", name, version, result)
    }

    async fn delete(&self, owner: &Owner::Row, name: &str, version: &str) -> Result<(), RepoError> {
        let result = self.repo.delete(owner, name, version).await;
        Self::logged_with_name("delete", name, version, result)
    }
}

pub struct DbPluginRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbPluginRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
#[async_trait]
impl<Owner: PluginOwner, Scope: PluginScope> PluginRepo<Owner, Scope>
    for DbPluginRepo<sqlx::Postgres>
{
    async fn get_all(
        &self,
        owner: &Owner::Row,
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        Scope::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");

        owner.add_where_clause(&mut query);

        Ok(query
            .build_query_as::<PluginRecord<Owner, Scope>>()
            .fetch_all(self.db_pool.deref())
            .await?)
    }

    async fn get_for_scope(
        &self,
        owner: &Owner::Row,
        scopes: &[Scope::Row],
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        Scope::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");

        owner.add_where_clause(&mut query);

        query.push(" AND (");
        for (idx, scope) in scopes.iter().enumerate() {
            scope.add_where_clause(&mut query);
            if idx < scopes.len() - 1 {
                query.push(" OR ");
            }
        }
        query.push(")");

        debug!("Built query for get_for_scope: {}", query.sql());

        Ok(query
            .build_query_as::<PluginRecord<Owner, Scope>>()
            .fetch_all(self.db_pool.deref())
            .await?)
    }

    async fn get_all_with_name(
        &self,
        owner: &Owner::Row,
        name: &str,
    ) -> Result<Vec<PluginRecord<Owner, Scope>>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        Scope::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");

        owner.add_where_clause(&mut query);

        query.push(" AND name = ");
        query.push_bind(name);

        debug!("Built query for get_all_with_name: {}", query.sql());

        Ok(query
            .build_query_as::<PluginRecord<Owner, Scope>>()
            .fetch_all(self.db_pool.deref())
            .await?)
    }

    async fn create(&self, record: &PluginRecord<Owner, Scope>) -> Result<(), RepoError> {
        let mut query = QueryBuilder::new("INSERT INTO plugins (");

        let mut column_list = query.separated(", ");

        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        Scope::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(") VALUES (");

        let mut value_list = query.separated(", ");
        value_list.push_bind(&record.name);
        value_list.push_bind(&record.version);
        value_list.push_bind(&record.description);
        value_list.push_bind(&record.icon);
        value_list.push_bind(&record.homepage);
        value_list.push_bind(record.plugin_type);

        record.scope.push_bind(&mut value_list);
        record.owner.push_bind(&mut value_list);

        value_list.push_bind(&record.provided_wit_package);
        value_list.push_bind(&record.json_schema);
        value_list.push_bind(&record.validate_url);
        value_list.push_bind(&record.transform_url);
        value_list.push_bind(record.component_id);
        value_list.push_bind(record.component_version);
        value_list.push_bind(false);

        query.push(")");

        debug!("Built query for create: {}", query.sql());

        query.build().execute(self.db_pool.deref()).await?;

        Ok(())
    }

    async fn get(
        &self,
        owner: &Owner::Row,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginRecord<Owner, Scope>>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        Scope::Row::add_column_list(&mut column_list);
        Owner::Row::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");
        owner.add_where_clause(&mut query);

        query.push(" AND name = ");
        query.push_bind(name);
        query.push(" AND version = ");
        query.push_bind(version);

        Ok(query
            .build_query_as::<PluginRecord<Owner, Scope>>()
            .fetch_optional(self.db_pool.deref())
            .await?)
    }

    async fn delete(&self, owner: &Owner::Row, name: &str, version: &str) -> Result<(), RepoError> {
        let mut query = QueryBuilder::new("UPDATE plugins SET deleted = TRUE WHERE name = ");

        query.push_bind(name);
        query.push(" AND version = ");
        query.push_bind(version);
        query.push(" AND ");
        owner.add_where_clause(&mut query);

        query.build().execute(self.db_pool.deref()).await?;

        Ok(())
    }
}
