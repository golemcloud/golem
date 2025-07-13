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

use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::plugin::{
    AppPluginDefinition, ComponentTransformerDefinition, LibraryPluginDefinition,
    OplogProcessorDefinition, PluginDefinition, PluginTypeSpecificDefinition, PluginWasmFileKey,
};
use golem_common::model::{ComponentId, PluginId};
use golem_common::repo::{PluginOwnerRow, PluginScopeRow, RowMeta};
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use sqlx::QueryBuilder;
use std::fmt::{Debug, Formatter};
use tracing::{debug, info_span, Instrument, Span};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct PluginRecord {
    id: Uuid,
    name: String,
    version: String,
    description: String,
    icon: Vec<u8>,
    homepage: String,
    plugin_type: i16,
    #[sqlx(flatten)]
    scope: PluginScopeRow,
    #[sqlx(flatten)]
    owner: PluginOwnerRow,

    // for ComponentTransformer plugin type
    provided_wit_package: Option<String>,
    json_schema: Option<String>,
    validate_url: Option<String>,
    transform_url: Option<String>,

    // for OplogProcessor plugin type
    component_id: Option<Uuid>,
    component_version: Option<i64>,

    // for LibraryPlugin plugin type
    blob_storage_key: Option<String>,

    #[allow(dead_code)]
    deleted: bool,
}

impl From<PluginDefinition> for PluginRecord {
    fn from(value: PluginDefinition) -> Self {
        Self {
            id: value.id.0,
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

            blob_storage_key: match &value.specs {
                PluginTypeSpecificDefinition::Library(def) => Some(def.blob_storage_key.0.clone()),
                PluginTypeSpecificDefinition::App(def) => Some(def.blob_storage_key.0.clone()),
                _ => None,
            },

            deleted: false,
        }
    }
}

impl TryFrom<PluginRecord> for PluginDefinition {
    type Error = String;

    fn try_from(value: PluginRecord) -> Result<Self, Self::Error> {
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
            2 => PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
                blob_storage_key: PluginWasmFileKey(
                    value
                        .blob_storage_key
                        .ok_or("blob_storage_key is required for LibraryPlugin rows")?,
                ),
            }),
            3 => PluginTypeSpecificDefinition::App(AppPluginDefinition {
                blob_storage_key: PluginWasmFileKey(
                    value
                        .blob_storage_key
                        .ok_or("blob_storage_key is required for AppPlugin rows")?,
                ),
            }),
            other => return Err(format!("Invalid plugin type: {other}")),
        };

        Ok(PluginDefinition {
            id: PluginId(value.id),
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs,
            scope: value.scope.try_into()?,
            owner: value.owner.try_into()?,
            deleted: value.deleted,
        })
    }
}

#[async_trait]
pub trait PluginRepo: Debug + Send + Sync {
    async fn get_all(&self, owner: &PluginOwnerRow) -> Result<Vec<PluginRecord>, RepoError>;

    async fn get_for_scope(
        &self,
        owner: &PluginOwnerRow,
        scope: &[PluginScopeRow],
    ) -> Result<Vec<PluginRecord>, RepoError>;

    async fn get_all_with_name(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
    ) -> Result<Vec<PluginRecord>, RepoError>;

    async fn create(&self, record: &PluginRecord) -> Result<(), RepoError>;

    async fn get(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginRecord>, RepoError>;

    async fn get_by_id(&self, id: &Uuid) -> Result<Option<PluginRecord>, RepoError>;

    async fn delete(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
        version: &str,
    ) -> Result<(), RepoError>;
}

pub struct LoggedPluginRepo<Repo> {
    repo: Repo,
}

impl<Repo> LoggedPluginRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span(plugin_name: &str, plugin_version: &str) -> Span {
        info_span!(
            "plugin repository",
            plugin_name = plugin_name,
            plugin_version = plugin_version
        )
    }
}

impl<Repo: PluginRepo> Debug for LoggedPluginRepo<Repo> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.repo.fmt(f)
    }
}

#[async_trait]
impl<Repo: PluginRepo> PluginRepo for LoggedPluginRepo<Repo> {
    async fn get_all(&self, owner: &PluginOwnerRow) -> Result<Vec<PluginRecord>, RepoError> {
        self.repo.get_all(owner).await
    }

    async fn get_for_scope(
        &self,
        owner: &PluginOwnerRow,
        scope: &[PluginScopeRow],
    ) -> Result<Vec<PluginRecord>, RepoError> {
        self.repo.get_for_scope(owner, scope).await
    }

    async fn get_all_with_name(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
    ) -> Result<Vec<PluginRecord>, RepoError> {
        self.repo
            .get_all_with_name(owner, name)
            .instrument(Self::span(name, "*"))
            .await
    }

    async fn create(&self, record: &PluginRecord) -> Result<(), RepoError> {
        self.repo
            .create(record)
            .instrument(Self::span(&record.name, &record.version))
            .await
    }

    async fn get(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginRecord>, RepoError> {
        self.repo
            .get(owner, name, version)
            .instrument(Self::span(name, version))
            .await
    }

    async fn get_by_id(&self, id: &Uuid) -> Result<Option<PluginRecord>, RepoError> {
        self.repo
            .get_by_id(id)
            .instrument(info_span!("plugin repository", plugin_id = id.to_string()))
            .await
    }

    async fn delete(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
        version: &str,
    ) -> Result<(), RepoError> {
        self.repo
            .delete(owner, name, version)
            .instrument(Self::span(name, version))
            .await
    }
}

pub struct DbPluginRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbPluginRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

impl<DB: Pool> Debug for DbPluginRepo<DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbPluginRepo")
            .field("db_pool", &self.db_pool)
            .finish()
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
    )]
#[async_trait]
impl PluginRepo for DbPluginRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn get_all(&self, owner: &PluginOwnerRow) -> Result<Vec<PluginRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("id");
        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        PluginScopeRow::add_column_list(&mut column_list);
        PluginOwnerRow::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("blob_storage_key");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");

        owner.add_where_clause(&mut query);

        self.db_pool
            .with_ro("plugin", "get_all")
            .fetch_all(query.build_query_as::<PluginRecord>())
            .await
    }

    async fn get_for_scope(
        &self,
        owner: &PluginOwnerRow,
        scopes: &[PluginScopeRow],
    ) -> Result<Vec<PluginRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("id");
        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        PluginScopeRow::add_column_list(&mut column_list);
        PluginOwnerRow::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("blob_storage_key");
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

        self.db_pool
            .with_ro("plugin", "get_for_scope")
            .fetch_all(query.build_query_as::<PluginRecord>())
            .await
    }

    async fn get_all_with_name(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
    ) -> Result<Vec<PluginRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("id");
        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        PluginScopeRow::add_column_list(&mut column_list);
        PluginOwnerRow::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("blob_storage_key");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");

        owner.add_where_clause(&mut query);

        query.push(" AND name = ");
        query.push_bind(name);

        debug!("Built query for get_all_with_name: {}", query.sql());

        self.db_pool
            .with_ro("plugin", "get_all_with_name")
            .fetch_all(query.build_query_as::<PluginRecord>())
            .await
    }

    async fn create(&self, record: &PluginRecord) -> Result<(), RepoError> {
        let mut query = QueryBuilder::new("INSERT INTO plugins (");

        let mut column_list = query.separated(", ");

        column_list.push("id");
        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        PluginScopeRow::add_column_list(&mut column_list);
        PluginOwnerRow::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("blob_storage_key");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(") VALUES (");

        let mut value_list = query.separated(", ");
        value_list.push_bind(record.id);
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
        value_list.push_bind(&record.blob_storage_key);
        value_list.push_bind(record.component_id);
        value_list.push_bind(record.component_version);
        value_list.push_bind(false);

        query.push(")");

        debug!("Built query for create: {}", query.sql());

        self.db_pool
            .with_rw("plugin", "create")
            .execute(query.build())
            .await?;

        Ok(())
    }

    async fn get(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
        version: &str,
    ) -> Result<Option<PluginRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("id");
        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        PluginScopeRow::add_column_list(&mut column_list);
        PluginOwnerRow::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("blob_storage_key");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        query.push(" FROM plugins WHERE NOT deleted AND ");
        owner.add_where_clause(&mut query);

        query.push(" AND name = ");
        query.push_bind(name);
        query.push(" AND version = ");
        query.push_bind(version);

        self.db_pool
            .with_ro("plugin", "get")
            .fetch_optional_as(query.build_query_as::<PluginRecord>())
            .await
    }

    async fn get_by_id(&self, id: &Uuid) -> Result<Option<PluginRecord>, RepoError> {
        let mut query = QueryBuilder::new("SELECT ");

        let mut column_list = query.separated(", ");

        column_list.push("id");
        column_list.push("name");
        column_list.push("version");
        column_list.push("description");
        column_list.push("icon");
        column_list.push("homepage");
        column_list.push("plugin_type");

        PluginScopeRow::add_column_list(&mut column_list);
        PluginOwnerRow::add_column_list(&mut column_list);

        column_list.push("provided_wit_package");
        column_list.push("json_schema");
        column_list.push("validate_url");
        column_list.push("transform_url");
        column_list.push("blob_storage_key");
        column_list.push("component_id");
        column_list.push("component_version");
        column_list.push("deleted");

        // Important: do not filter out deleted plugins here.
        query.push(" FROM plugins WHERE ");
        query.push(" id = ");
        query.push_bind(id);

        self.db_pool
            .with_ro("plugin", "get_by_id")
            .fetch_optional_as(query.build_query_as::<PluginRecord>())
            .await
    }

    async fn delete(
        &self,
        owner: &PluginOwnerRow,
        name: &str,
        version: &str,
    ) -> Result<(), RepoError> {
        let mut query = QueryBuilder::new("UPDATE plugins SET deleted = TRUE WHERE name = ");

        query.push_bind(name);
        query.push(" AND version = ");
        query.push_bind(version);
        query.push(" AND ");
        owner.add_where_clause(&mut query);

        self.db_pool
            .with_rw("plugin", "delete")
            .execute(query.build())
            .await?;

        Ok(())
    }
}
