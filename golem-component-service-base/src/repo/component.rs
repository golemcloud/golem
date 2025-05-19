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

use crate::model::{Component, ComponentConstraints};
use crate::service::component::{ComponentByNameAndVersion, VersionType};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use conditional_trait_gen::{trait_gen, when};
use futures::future::try_join_all;
use golem_common::model::component::{ComponentOwner, VersionedComponentId};
use golem_common::model::component_constraint::{FunctionConstraints, FunctionSignature};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::plugin::{ComponentPluginInstallationTarget, PluginOwner};
use golem_common::model::{
    ComponentFilePath, ComponentFilePermissions, ComponentId, ComponentType, InitialComponentFile,
    InitialComponentFileKey,
};
use golem_common::repo::plugin_installation::ComponentPluginInstallationRow;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::model::ComponentName;
use golem_service_base::repo::plugin_installation::{
    DbPluginInstallationRepoQueries, PluginInstallationRecord, PluginInstallationRepoQueries,
};
use golem_service_base::repo::RepoError;
use sqlx::types::Json;
use sqlx::{Postgres, QueryBuilder, Row, Sqlite};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::result::Result;
use std::sync::Arc;
use tracing::{debug, info_span, Span};
use tracing_futures::Instrument;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentRecord<Owner: ComponentOwner> {
    pub namespace: String,
    pub component_id: Uuid,
    pub name: String,
    pub size: i32,
    pub version: i64,
    pub metadata: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub component_type: i32,
    pub available: bool,
    pub object_store_key: Option<String>,
    pub transformed_object_store_key: Option<String>,
    // one-to-many relationship. Retrieved separately
    #[sqlx(skip)]
    pub files: Vec<FileRecord>,
    // one-to-many relationship. Retrieved separately
    #[sqlx(skip)]
    pub installed_plugins:
        Vec<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
    pub env: Json<HashMap<String, String>>,
}

impl<Owner: ComponentOwner> ComponentRecord<Owner> {
    pub fn try_from_model(value: Component<Owner>, available: bool) -> Result<Self, String> {
        let metadata = record_metadata_serde::serialize(&value.metadata)?;

        let component_owner = value.owner.clone();
        let component_owner_row: Owner::Row = component_owner.into();
        let plugin_owner_row: <Owner::PluginOwner as PluginOwner>::Row = component_owner_row.into();

        let object_store_key = value
            .object_store_key
            .unwrap_or(value.versioned_component_id.to_string());

        Ok(Self {
            namespace: value.owner.to_string(),
            component_id: value.versioned_component_id.component_id.0,
            name: value.component_name.0,
            size: value.component_size as i32,
            version: value.versioned_component_id.version as i64,
            metadata: metadata.into(),
            created_at: value.created_at,
            component_type: value.component_type as i32,
            available,
            object_store_key: Some(object_store_key.clone()),
            transformed_object_store_key: Some(
                value
                    .transformed_object_store_key
                    .unwrap_or(object_store_key),
            ),
            files: value
                .files
                .iter()
                .map(|file| FileRecord {
                    component_id: value.versioned_component_id.component_id.0,
                    version: value.versioned_component_id.version as i64,
                    file_path: file.path.to_string(),
                    file_key: file.key.0.clone(),
                    file_permissions: file.permissions.as_compact_str().to_string(),
                })
                .collect(),
            installed_plugins: value
                .installed_plugins
                .iter()
                .map(|installation| {
                    PluginInstallationRecord::try_from(
                        installation.clone(),
                        plugin_owner_row.clone(),
                        ComponentPluginInstallationRow {
                            component_id: value.versioned_component_id.component_id.0,
                            component_version: value.versioned_component_id.version as i64,
                        },
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            root_package_name: value.metadata.root_package_name,
            root_package_version: value.metadata.root_package_version,
            env: Json(value.env),
        })
    }
}

impl<Owner: ComponentOwner> TryFrom<ComponentRecord<Owner>> for Component<Owner> {
    type Error = String;

    fn try_from(value: ComponentRecord<Owner>) -> Result<Self, Self::Error> {
        let metadata: ComponentMetadata = record_metadata_serde::deserialize(&value.metadata)?;
        let versioned_component_id: VersionedComponentId = VersionedComponentId {
            component_id: ComponentId(value.component_id),
            version: value.version as u64,
        };
        let owner: Owner = value.namespace.parse()?;
        let files = value
            .files
            .into_iter()
            .map(|file| file.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Component {
            owner,
            component_name: ComponentName(value.name),
            component_size: value.size as u64,
            metadata,
            versioned_component_id,
            created_at: value.created_at,
            component_type: ComponentType::try_from(value.component_type)?,
            object_store_key: value.object_store_key,
            transformed_object_store_key: value.transformed_object_store_key,
            files,
            installed_plugins: value
                .installed_plugins
                .into_iter()
                .map(|record| record.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            env: value.env.0,
        })
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ComponentConstraintsRecord {
    pub namespace: String,
    pub component_id: Uuid,
    pub constraints: Vec<u8>,
}

impl<Owner: ComponentOwner> TryFrom<ComponentConstraints<Owner>> for ComponentConstraintsRecord {
    type Error = String;

    fn try_from(value: ComponentConstraints<Owner>) -> Result<Self, Self::Error> {
        let metadata = constraint_serde::serialize(&value.constraints)?;
        Ok(Self {
            namespace: value.owner.to_string(),
            component_id: value.component_id.0,
            constraints: metadata.into(),
        })
    }
}

impl<Owner: ComponentOwner> TryFrom<ComponentConstraintsRecord> for ComponentConstraints<Owner> {
    type Error = String;

    fn try_from(value: ComponentConstraintsRecord) -> Result<Self, Self::Error> {
        let function_constraints: FunctionConstraints =
            constraint_serde::deserialize(&value.constraints)?;
        let owner = value.namespace.parse()?;
        Ok(ComponentConstraints {
            owner,
            component_id: ComponentId(value.component_id),
            constraints: function_constraints,
        })
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct FileRecord {
    pub component_id: Uuid,
    pub version: i64,
    pub file_path: String,
    pub file_key: String,
    pub file_permissions: String,
}

impl FileRecord {
    pub fn from_component_id_and_version_and_file(
        component_id: Uuid,
        version: i64,
        file: &InitialComponentFile,
    ) -> Self {
        Self {
            component_id,
            version,
            file_path: file.path.to_string(),
            file_key: file.key.0.clone(),
            file_permissions: file.permissions.as_compact_str().to_string(),
        }
    }

    pub fn from_component_and_file<Owner: ComponentOwner>(
        component: &Component<Owner>,
        file: &InitialComponentFile,
    ) -> Self {
        Self::from_component_id_and_version_and_file(
            component.versioned_component_id.component_id.0,
            component.versioned_component_id.version as i64,
            file,
        )
    }
}

impl TryFrom<FileRecord> for InitialComponentFile {
    type Error = String;

    fn try_from(value: FileRecord) -> Result<Self, Self::Error> {
        Ok(InitialComponentFile {
            path: ComponentFilePath::from_abs_str(value.file_path.as_str())?,
            key: InitialComponentFileKey(value.file_key),
            permissions: ComponentFilePermissions::from_compact_str(&value.file_permissions)?,
        })
    }
}

#[async_trait]
pub trait ComponentRepo<Owner: ComponentOwner>: Debug + Send + Sync {
    async fn create(&self, component: &ComponentRecord<Owner>) -> Result<(), RepoError>;

    /// Creates a new component version (ignores component.version) and copies the plugin
    /// installations from the previous latest version.
    ///
    /// Returns the updated component.
    async fn update(
        &self,
        owner: &Owner::Row,
        namespace: &str,
        component_id: &Uuid,
        data: Vec<u8>,
        metadata: Vec<u8>,
        component_type: Option<i32>,
        files: Option<Vec<FileRecord>>,
        env: Json<HashMap<String, String>>,
    ) -> Result<ComponentRecord<Owner>, RepoError>;

    /// Activates a component version previously created with `update`.
    ///
    /// Once the version is marked as active, `get_latest_version` will take it into account when
    /// looking for the latest component version.
    async fn activate(
        &self,
        namespace: &str,
        component_id: &Uuid,
        component_version: i64,
        object_store_key: &str,
        transformed_object_store_key: &str,
        updated_metadata: Vec<u8>,
    ) -> Result<(), RepoError>;

    async fn get(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError>;

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord<Owner>>, RepoError>;

    async fn get_latest_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError>;

    async fn get_by_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError>;

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError>;

    async fn get_by_names(
        &self,
        namespace: &str,
        names: &[ComponentByNameAndVersion],
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError>;

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError>;

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError>;

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError>;

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError>;

    async fn delete_constraints(
        &self,
        namespace: &str,
        component_id: &Uuid,
        constraints_to_remove: &[FunctionSignature],
    ) -> Result<(), RepoError>;

    async fn get_constraint(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<FunctionConstraints>, RepoError>;

    async fn get_installed_plugins(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        version: u64,
    ) -> Result<
        Vec<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>,
        RepoError,
    >;

    async fn apply_plugin_installation_changes(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        actions: &[PluginInstallationRepoAction<Owner>],
    ) -> Result<u64, RepoError>;
}

pub enum PluginInstallationRepoAction<Owner: ComponentOwner> {
    Install {
        record: PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>,
    },
    Update {
        plugin_installation_id: Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    },
    Uninstall {
        plugin_installation_id: Uuid,
    },
}

pub struct LoggedComponentRepo<Owner: ComponentOwner, Repo: ComponentRepo<Owner>> {
    repo: Repo,
    _owner: PhantomData<Owner>,
}

impl<Owner: ComponentOwner, Repo: ComponentRepo<Owner>> Debug for LoggedComponentRepo<Owner, Repo> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.repo.fmt(f)
    }
}

impl<Owner: ComponentOwner, Repo: ComponentRepo<Owner>> LoggedComponentRepo<Owner, Repo> {
    pub fn new(repo: Repo) -> Self {
        Self {
            repo,
            _owner: PhantomData,
        }
    }

    fn span(component_id: &Uuid) -> Span {
        let span = info_span!("component repository", component_id = %component_id);
        span
    }
}

#[async_trait]
impl<Owner: ComponentOwner, Repo: ComponentRepo<Owner> + Send + Sync> ComponentRepo<Owner>
    for LoggedComponentRepo<Owner, Repo>
{
    async fn create(&self, component: &ComponentRecord<Owner>) -> Result<(), RepoError> {
        self.repo
            .create(component)
            .instrument(Self::span(&component.component_id))
            .await
    }

    async fn update(
        &self,
        owner: &Owner::Row,
        namespace: &str,
        component_id: &Uuid,
        data: Vec<u8>,
        metadata: Vec<u8>,
        component_type: Option<i32>,
        files: Option<Vec<FileRecord>>,
        env: Json<HashMap<String, String>>,
    ) -> Result<ComponentRecord<Owner>, RepoError> {
        self.repo
            .update(
                owner,
                namespace,
                component_id,
                data,
                metadata,
                component_type,
                files,
                env,
            )
            .instrument(Self::span(component_id))
            .await
    }

    async fn activate(
        &self,
        namespace: &str,
        component_id: &Uuid,
        component_version: i64,
        object_store_key: &str,
        transformed_object_store_key: &str,
        updated_metadata: Vec<u8>,
    ) -> Result<(), RepoError> {
        self.repo
            .activate(
                namespace,
                component_id,
                component_version,
                object_store_key,
                transformed_object_store_key,
                updated_metadata,
            )
            .instrument(Self::span(component_id))
            .await
    }

    async fn get(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        self.repo
            .get(namespace, component_id)
            .instrument(Self::span(component_id))
            .await
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        self.repo.get_all(namespace).await
    }

    async fn get_latest_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        self.repo
            .get_latest_version(namespace, component_id)
            .instrument(Self::span(component_id))
            .await
    }

    async fn get_by_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        self.repo
            .get_by_version(namespace, component_id, version)
            .instrument(Self::span(component_id))
            .await
    }

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        self.repo.get_by_name(namespace, name).await
    }

    async fn get_by_names(
        &self,
        namespace: &str,
        names: &[ComponentByNameAndVersion],
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        self.repo
            .get_by_names(namespace, names)
            .instrument(info_span!("get_by_names", namespace = %namespace))
            .await
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        self.repo.get_id_by_name(namespace, name).await
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        self.repo.get_namespace(component_id).await
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        self.repo.delete(namespace, component_id).await
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError> {
        self.repo
            .create_or_update_constraint(component_constraint_record)
            .instrument(Self::span(&component_constraint_record.component_id))
            .await
    }

    // The only way to delete constraints is to delete through the usage interfaces.
    // This is to avoid surprises. For example: An API might be deployed and is live,
    // so any forceful deletes without deleting API deployment will result in irrecoverable issues.
    async fn delete_constraints(
        &self,
        namespace: &str,
        component_id: &Uuid,
        constraints_to_remove: &[FunctionSignature],
    ) -> Result<(), RepoError> {
        self.repo
            .delete_constraints(namespace, component_id, constraints_to_remove)
            .instrument(Self::span(component_id))
            .await
    }

    async fn get_constraint(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<FunctionConstraints>, RepoError> {
        self.repo
            .get_constraint(namespace, component_id)
            .instrument(Self::span(component_id))
            .await
    }

    async fn get_installed_plugins(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        version: u64,
    ) -> Result<
        Vec<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>,
        RepoError,
    > {
        self.repo
            .get_installed_plugins(owner, component_id, version)
            .instrument(Self::span(component_id))
            .await
    }

    async fn apply_plugin_installation_changes(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        actions: &[PluginInstallationRepoAction<Owner>],
    ) -> Result<u64, RepoError> {
        self.repo
            .apply_plugin_installation_changes(owner, component_id, actions)
            .instrument(Self::span(component_id))
            .await
    }
}

pub struct DbComponentRepo<DB: Pool, Owner: ComponentOwner> {
    db_pool: DB,
    plugin_installation_queries: Arc<
        dyn PluginInstallationRepoQueries<
                DB::Db,
                Owner::PluginOwner,
                ComponentPluginInstallationTarget,
            > + Send
            + Sync,
    >,
}

impl<DB: Pool + Sync, Owner: ComponentOwner> DbComponentRepo<DB, Owner>
where
    DbPluginInstallationRepoQueries<DB::Db>: PluginInstallationRepoQueries<
        DB::Db,
        Owner::PluginOwner,
        ComponentPluginInstallationTarget,
    >,
{
    pub fn new(db_pool: DB) -> Self {
        let plugin_installation_queries =
            Arc::new(DbPluginInstallationRepoQueries::<DB::Db>::new());
        Self {
            db_pool,
            plugin_installation_queries,
        }
    }
}

impl<Owner: ComponentOwner, DB: Pool> Debug for DbComponentRepo<DB, Owner> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbComponentRepo")
            .field("db_pool", &self.db_pool)
            .finish()
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
impl<Owner: ComponentOwner> DbComponentRepo<golem_service_base::db::postgres::PostgresPool, Owner> {
    async fn get_files(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Vec<FileRecord>, RepoError> {
        let query = sqlx::query_as::<_, FileRecord>(
            r#"
            SELECT
                component_id,
                version,
                file_path,
                file_key,
                file_permissions
            FROM component_files
            WHERE component_id = $1 AND version = $2
            ORDER BY file_path
            "#,
        )
        .bind(component_id)
        .bind(version as i64);
        self.db_pool
            .with_ro("component", "get_files")
            .fetch_all(query)
            .await
    }

    async fn add_files(
        &self,
        components: impl IntoIterator<Item = ComponentRecord<Owner>>,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let result = components
            .into_iter()
            .map(|component| async move {
                let files = self
                    .get_files(&component.component_id, component.version as u64)
                    .await?;
                Ok(ComponentRecord { files, ..component })
            })
            .collect::<Vec<_>>();

        try_join_all(result).await
    }

    async fn get_installed_plugins_for_component(
        &self,
        component: &ComponentRecord<Owner>,
    ) -> Result<
        Vec<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>,
        RepoError,
    > {
        let target = ComponentPluginInstallationRow {
            component_id: component.component_id,
            component_version: component.version,
        };

        let owner = Owner::from_str(&component.namespace).map_err(RepoError::Internal)?;
        let owner_row: Owner::Row = owner.into();
        let plugin_owner_row: <Owner::PluginOwner as PluginOwner>::Row = owner_row.into();
        let mut query = self
            .plugin_installation_queries
            .get_all(&plugin_owner_row, &target);
        let query = query
            .build_query_as::<PluginInstallationRecord<
                Owner::PluginOwner,
                ComponentPluginInstallationTarget,
            >>();
        self.db_pool
            .with_ro("component", "get_installed_plugins_for_component")
            .fetch_all(query)
            .await
    }

    async fn add_installed_plugins(
        &self,
        components: impl IntoIterator<Item = ComponentRecord<Owner>>,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let result = components
            .into_iter()
            .map(|component| async move {
                let installed_plugins =
                    self.get_installed_plugins_for_component(&component).await?;
                Ok(ComponentRecord {
                    installed_plugins,
                    ..component
                })
            })
            .collect::<Vec<_>>();

        try_join_all(result).await
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl<Owner: ComponentOwner> ComponentRepo<Owner>
    for DbComponentRepo<golem_service_base::db::postgres::PostgresPool, Owner>
{
    async fn create(&self, component: &ComponentRecord<Owner>) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.with_rw("component", "create").begin().await?;

        let query = sqlx::query("SELECT namespace, name FROM components WHERE component_id = $1")
            .bind(component.component_id);

        let result = transaction.fetch_optional(query).await?;

        if let Some(result) = result {
            let namespace: String = result.get("namespace");
            let name: String = result.get("name");
            if namespace != component.namespace || name != component.name {
                self.db_pool
                    .with_rw("component", "create")
                    .rollback(transaction)
                    .await?;
                return Err(RepoError::Internal(
                    "Component namespace and name invalid".to_string(),
                ));
            }
        } else {
            let query = sqlx::query(
                r#"
                  INSERT INTO components
                    (namespace, component_id, name)
                  VALUES
                    ($1, $2, $3)
                   "#,
            )
            .bind(component.namespace.clone())
            .bind(component.component_id)
            .bind(component.name.clone());

            let result = transaction.execute(query).await;

            if let Err(err) = result {
                // Without this explicit rollback, sqlite seems to be remain locked when a next
                // incoming request comes in.
                self.db_pool
                    .with_rw("component", "create")
                    .rollback(transaction)
                    .await?;
                return Err(err);
            }
        }

        let query = sqlx::query(
            r#"
              INSERT INTO component_versions
                (component_id, version, size, metadata, created_at, component_type, available, object_store_key, transformed_object_store_key, root_package_name, root_package_version, env)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
               "#,
        )
            .bind(component.component_id)
            .bind(component.version)
            .bind(component.size)
            .bind(&component.metadata)
            .bind(component.created_at)
            .bind(component.component_type)
            .bind(component.available)
            .bind(&component.object_store_key)
            .bind(&component.transformed_object_store_key)
            .bind(&component.root_package_name)
            .bind(&component.root_package_version)
            .bind(&component.env);

        transaction.execute(query).await?;

        for file in &component.files {
            let query = sqlx::query(
                r#"
                  INSERT INTO component_files
                    (component_id, version, file_path, file_key, file_permissions)
                  VALUES
                    ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(component.component_id)
            .bind(component.version)
            .bind(file.file_path.clone())
            .bind(file.file_key.clone())
            .bind(file.file_permissions.clone());

            transaction.execute(query).await?;
        }

        self.db_pool
            .with_rw("component", "create")
            .commit(transaction)
            .await?;
        Ok(())
    }

    async fn update(
        &self,
        owner: &Owner::Row,
        namespace: &str,
        component_id: &Uuid,
        data: Vec<u8>,
        metadata: Vec<u8>,
        component_type: Option<i32>,
        files: Option<Vec<FileRecord>>,
        env: Json<HashMap<String, String>>,
    ) -> Result<ComponentRecord<Owner>, RepoError> {
        let mut transaction = self.db_pool.with_rw("component", "update").begin().await?;

        let query = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id);

        let result = transaction.fetch_optional(query).await?;

        if let Some(result) = result {
            let existing_namespace: String = result.get("namespace");
            if existing_namespace != namespace {
                self.db_pool
                    .with_rw("component", "update")
                    .rollback(transaction)
                    .await?;
                Err(RepoError::Internal(
                    "Component namespace invalid".to_string(),
                ))
            } else {
                let now = Utc::now();
                let new_version = if let Some(component_type) = component_type {
                    let query = sqlx::query(
                        r#"
                              WITH prev AS (SELECT component_id, version, object_store_key, transformed_object_store_key, root_package_name, root_package_version
                                   FROM component_versions WHERE component_id = $1
                                   ORDER BY version DESC
                                   LIMIT 1)
                              INSERT INTO component_versions
                              SELECT prev.component_id, prev.version + 1, $2, $3, $4, $5, FALSE, prev.object_store_key, prev.transformed_object_store_key, prev.root_package_name, prev.root_package_version, $6 FROM prev
                              RETURNING *
                              "#,
                    )
                        .bind(component_id)
                        .bind(data.len() as i32)
                        .bind(now)
                        .bind(metadata)
                        .bind(component_type)
                        .bind(env);

                    transaction.fetch_one(query).await?.get("version")
                } else {
                    let query = sqlx::query(
                        r#"
                              WITH prev AS (SELECT component_id, version, component_type, object_store_key, transformed_object_store_key, root_package_name, root_package_version
                                   FROM component_versions WHERE component_id = $1
                                   ORDER BY version DESC
                                   LIMIT 1)
                              INSERT INTO component_versions
                              SELECT prev.component_id, prev.version + 1, $2, $3, $4, prev.component_type, FALSE, prev.object_store_key, prev.transformed_object_store_key, prev.root_package_name, prev.root_package_version, $5 FROM prev
                              RETURNING *
                              "#,
                    )
                        .bind(component_id)
                        .bind(data.len() as i32)
                        .bind(now)
                        .bind(metadata)
                        .bind(env);

                    transaction.fetch_one(query).await?.get("version")
                };

                debug!("update created new component version {new_version}");

                let old_target = ComponentPluginInstallationRow {
                    component_id: *component_id,
                    component_version: new_version - 1,
                };

                let plugin_owner = owner.clone().into();
                let mut query = self
                    .plugin_installation_queries
                    .get_all(&plugin_owner, &old_target);
                let query =
                    query.build_query_as::<PluginInstallationRecord<
                        Owner::PluginOwner,
                        ComponentPluginInstallationTarget,
                    >>();

                let existing_installations = transaction.fetch_all(query).await?;

                let new_target = ComponentPluginInstallationRow {
                    component_id: *component_id,
                    component_version: new_version,
                };

                let mut new_installations = Vec::new();
                for installation in existing_installations {
                    let old_id = installation.installation_id;
                    let new_id = Uuid::new_v4();
                    let new_installation = PluginInstallationRecord {
                        installation_id: new_id,
                        target: new_target.clone(),
                        ..installation
                    };
                    new_installations.push(new_installation);

                    debug!("update copying installation {old_id} as {new_id}");
                }

                for installation in new_installations {
                    let mut query = self.plugin_installation_queries.create(&installation);
                    let query = query.build();
                    transaction.execute(query).await?;
                }

                let files = if let Some(files) = files {
                    files
                } else {
                    // Copying the previous file set
                    let query = sqlx::query_as::<_, FileRecord>(
                        r#"
                            SELECT
                                component_id,
                                version,
                                file_path,
                                file_key,
                                file_permissions
                            FROM component_files
                            WHERE component_id = $1 AND version = $2
                            "#,
                    )
                    .bind(old_target.component_id)
                    .bind(old_target.component_version);

                    transaction.fetch_all(query).await?
                };

                // Inserting the new file set
                for file in files {
                    let query = sqlx::query(
                        r#"
                          INSERT INTO component_files
                            (component_id, version, file_path, file_key, file_permissions)
                          VALUES
                            ($1, $2, $3, $4, $5)
                        "#,
                    )
                    .bind(component_id)
                    .bind(new_version)
                    .bind(&file.file_path)
                    .bind(&file.file_key)
                    .bind(&file.file_permissions);

                    transaction.execute(query).await?;
                }

                self.db_pool
                    .with_rw("component", "update")
                    .commit(transaction)
                    .await?;

                let component = self
                    .get_by_version(namespace, component_id, new_version as u64)
                    .await?;

                component.ok_or(RepoError::Internal(
                    "Could not re-get newly created component version".to_string(),
                ))
            }
        } else {
            self.db_pool
                .with_rw("component", "update")
                .rollback(transaction)
                .await?;
            Err(RepoError::Internal(
                "Component not found for update".to_string(),
            ))
        }
    }

    async fn activate(
        &self,
        namespace: &str,
        component_id: &Uuid,
        component_version: i64,
        object_store_key: &str,
        transformed_object_store_key: &str,
        updated_metadata: Vec<u8>,
    ) -> Result<(), RepoError> {
        let query = sqlx::query(
            r#"
              UPDATE component_versions
              SET available = TRUE, object_store_key = $4, metadata = $5, transformed_object_store_key = $6
              WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
                    AND version = $3
            "#,
        ).bind(namespace)
            .bind(component_id)
            .bind(component_version)
            .bind(object_store_key)
            .bind(updated_metadata)
            .bind(transformed_object_store_key);

        self.db_pool
            .with_rw("component", "activate")
            .execute(query)
            .await?;

        Ok(())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get)]
    async fn get_postgres(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key as transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2
                ORDER BY cv.version
                "#,
        )
        .bind(component_id)
        .bind(namespace);

        let components = self
            .db_pool
            .with("component", "get")
            .fetch_all(query)
            .await?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get)]
    async fn get_sqlite(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2
                ORDER BY cv.version
                "#,
        )
        .bind(component_id)
        .bind(namespace);

        let components = self
            .db_pool
            .with_ro("component", "get")
            .fetch_all(query)
            .await?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_all)]
    async fn get_all_postgres(
        &self,
        namespace: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1
                ORDER BY cv.component_id, cv.version
                "#,
        )
        .bind(namespace);

        let components = self
            .db_pool
            .with("component", "get_all")
            .fetch_all(query)
            .await?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_all)]
    async fn get_all_sqlite(
        &self,
        namespace: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1
                ORDER BY cv.component_id, cv.version
                "#,
        )
        .bind(namespace);

        let components = self
            .db_pool
            .with_ro("component", "get_all")
            .fetch_all(query)
            .await?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_latest_version)]
    async fn get_latest_version_postgres(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2 AND cv.available = TRUE
                ORDER BY cv.version DESC
                LIMIT 1
                "#,
        )
        .bind(component_id)
        .bind(namespace);

        let component = self
            .db_pool
            .with("component", "get_latest_version")
            .fetch_optional_as(query)
            .await?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_latest_version)]
    async fn get_latest_version_sqlite(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2 AND cv.available = TRUE
                ORDER BY cv.version
                DESC LIMIT 1
                "#,
        )
        .bind(component_id)
        .bind(namespace);

        let component = self
            .db_pool
            .with_ro("component", "get_latest_version")
            .fetch_optional_as(query)
            .await?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_version)]
    async fn get_by_version_postgres(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND cv.version = $2 AND c.namespace = $3
                "#,
        )
        .bind(component_id)
        .bind(version as i64)
        .bind(namespace);

        let component = self
            .db_pool
            .with("component", "get_by_version")
            .fetch_optional_as(query)
            .await?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_version)]
    async fn get_by_version_sqlite(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND cv.version = $2 AND c.namespace = $3
                "#,
        )
        .bind(component_id)
        .bind(version as i64)
        .bind(namespace);

        let component = self
            .db_pool
            .with_ro("component", "get_by_version")
            .fetch_optional_as(query)
            .await?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_name)]
    async fn get_by_name_postgres(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at::timestamptz AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.name = $2
                ORDER BY cv.version
                "#,
        )
        .bind(namespace)
        .bind(name);

        let components = self
            .db_pool
            .with("component", "get_by_name")
            .fetch_all(query)
            .await?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(golem_service_base::db::postgres::PostgresPool -> get_by_names)]
    async fn get_by_names_postgres(
        &self,
        namespace: &str,
        components: &[ComponentByNameAndVersion],
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
            r#"
        WITH input_components(name, version, is_latest) AS (
        "#,
        );

        // Build VALUES (...) clause safely
        query_builder.push_values(components, |mut b, comp| {
            let name = comp.component_name.to_string();
            match &comp.version_type {
                VersionType::Latest => {
                    b.push_bind(name)
                        .push_bind(Option::<i64>::None)
                        .push_bind(true);
                }
                VersionType::Exact(ver) => {
                    b.push_bind(name)
                        .push_bind(Some(*ver as i64))
                        .push_bind(false);
                }
            }
        });

        query_builder.push(
            r#")
        ,
        exact_matches AS (
            SELECT
                c.namespace,
                c.name,
                c.component_id,
                cv.version,
                cv.size,
                cv.metadata,
                cv.created_at::timestamptz,
                cv.component_type,
                cv.available,
                cv.object_store_key,
                cv.transformed_object_store_key,
                cv.root_package_name,
                cv.root_package_version,
                cv.env
            FROM components c
            JOIN component_versions cv ON c.component_id = cv.component_id
            JOIN input_components ic ON ic.name = c.name
            WHERE ic.is_latest = FALSE
              AND ic.version = cv.version
              AND c.namespace = "#,
        );
        query_builder.push_bind(namespace);
        query_builder.push(
            r#"
        ),
        latest_matches AS (
            SELECT DISTINCT ON (c.name)
                c.namespace,
                c.name,
                c.component_id,
                cv.version,
                cv.size,
                cv.metadata,
                cv.created_at::timestamptz,
                cv.component_type,
                cv.available,
                cv.object_store_key,
                cv.transformed_object_store_key,
                cv.root_package_name,
                cv.root_package_version,
                cv.env
            FROM components c
            JOIN component_versions cv ON c.component_id = cv.component_id
            JOIN input_components ic ON ic.name = c.name
            WHERE ic.is_latest = TRUE
              AND cv.available = TRUE
              AND c.namespace = "#,
        );
        query_builder.push_bind(namespace);
        query_builder.push(
            r#"
            ORDER BY c.name, cv.version DESC
        )
        SELECT * FROM exact_matches
        UNION
        SELECT * FROM latest_matches
        ORDER BY name, version
        "#,
        );

        let query = query_builder.build_query_as::<ComponentRecord<Owner>>();

        let components = self
            .db_pool
            .with("component", "get_by_names")
            .fetch_all(query)
            .await?;

        let components = self
            .add_installed_plugins(self.add_files(components).await?)
            .await?;

        Ok(components)
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_names)]
    async fn get_by_names_sqlite(
        &self,
        namespace: &str,
        components: &[ComponentByNameAndVersion],
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let mut query_builder: QueryBuilder<Sqlite> = QueryBuilder::new(
            r#"
        WITH input_components(name, version, is_latest) AS (
        "#,
        );

        // Build the VALUES clause
        query_builder.push_values(components, |mut b, item| {
            b.push_bind(item.component_name.0.as_str());
            match &item.version_type {
                VersionType::Exact(version) => {
                    b.push_bind(Some(*version as i64));
                    b.push_bind(0); // is_latest = false
                }
                VersionType::Latest => {
                    b.push_bind::<Option<&str>>(None);
                    b.push_bind(1); // is_latest = true
                }
            }
        });

        query_builder.push(
            r#"
        ),
        exact_matches AS (
            SELECT
                c.namespace,
                c.name,
                c.component_id,
                cv.version,
                cv.size,
                cv.metadata,
                cv.created_at,
                cv.component_type,
                cv.available,
                cv.object_store_key,
                cv.transformed_object_store_key,
                cv.root_package_name,
                cv.root_package_version,
                cv.env
            FROM components c
            JOIN component_versions cv ON c.component_id = cv.component_id
            JOIN input_components ic ON ic.name = c.name
            WHERE ic.is_latest = 0
              AND ic.version = cv.version
              AND c.namespace = ?
        ),
        latest_matches AS (
            SELECT
                c.namespace,
                c.name,
                c.component_id,
                cv.version,
                cv.size,
                cv.metadata,
                cv.created_at,
                cv.component_type,
                cv.available,
                cv.object_store_key,
                cv.transformed_object_store_key,
                cv.root_package_name,
                cv.root_package_version,
                cv.env
            FROM component_versions cv
            JOIN components c ON c.component_id = cv.component_id
            JOIN input_components ic ON ic.name = c.name
            WHERE ic.is_latest = 1
              AND cv.available = 1
              AND c.namespace = ?
              AND cv.version = (
                  SELECT MAX(cv2.version)
                  FROM component_versions cv2
                  WHERE cv2.component_id = cv.component_id
                    AND cv2.available = 1
              )
        )
        SELECT * FROM exact_matches
        UNION
        SELECT * FROM latest_matches
        ORDER BY name, version
        "#,
        );

        let query = query_builder
            .build_query_as::<ComponentRecord<Owner>>()
            .bind(namespace)
            .bind(namespace);

        let records = self
            .db_pool
            .with_ro("component", "get_by_names_sqlite")
            .fetch_all(query)
            .await?;

        let records = self
            .add_installed_plugins(self.add_files(records).await?)
            .await?;

        Ok(records)
    }

    #[when(golem_service_base::db::sqlite::SqlitePool -> get_by_name)]
    async fn get_by_name_sqlite(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let query = sqlx::query_as::<_, ComponentRecord<Owner>>(
            r#"
                SELECT
                    c.namespace AS namespace,
                    c.name AS name,
                    c.component_id AS component_id,
                    cv.version AS version,
                    cv.size AS size,
                    cv.metadata AS metadata,
                    cv.created_at AS created_at,
                    cv.component_type AS component_type,
                    cv.available AS available,
                    cv.object_store_key AS object_store_key,
                    cv.transformed_object_store_key AS transformed_object_store_key,
                    cv.root_package_name AS root_package_name,
                    cv.root_package_version AS root_package_version,
                    cv.env
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.name = $2
                ORDER BY cv.version
                "#,
        )
        .bind(namespace)
        .bind(name);

        let components = self
            .db_pool
            .with_ro("component", "get_by_name")
            .fetch_all(query)
            .await?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        let query =
            sqlx::query("SELECT component_id FROM components WHERE namespace = $1 AND name = $2")
                .bind(namespace)
                .bind(name);

        let result = self
            .db_pool
            .with_ro("component", "get_id_by_name")
            .fetch_optional(query)
            .await?;

        Ok(result.map(|x| x.get("component_id")))
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        let query = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id);

        let result = self
            .db_pool
            .with_ro("component", "get_namespace")
            .fetch_optional(query)
            .await?;

        Ok(result.map(|x| x.get("namespace")))
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        // TODO: delete plugin installations

        let mut transaction = self.db_pool.with_rw("component", "delete").begin().await?;
        let query = sqlx::query(
            r#"
                DELETE FROM component_versions
                WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
            "#
        )
            .bind(namespace)
            .bind(component_id);

        transaction.execute(query).await?;

        let query = sqlx::query(
            r#"
                DELETE FROM component_files
                WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
            "#
        )
            .bind(namespace)
            .bind(component_id);

        transaction.execute(query).await?;

        let query =
            sqlx::query("DELETE FROM components WHERE namespace = $1 AND component_id = $2")
                .bind(namespace)
                .bind(component_id);

        transaction.execute(query).await?;

        self.db_pool
            .with_rw("component", "delete")
            .commit(transaction)
            .await?;
        Ok(())
    }

    async fn delete_constraints(
        &self,
        namespace: &str,
        component_id: &Uuid,
        constraints: &[FunctionSignature],
    ) -> Result<(), RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("component", "delete_constraints")
            .begin()
            .await?;

        let query = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1
                "#,
        )
        .bind(component_id);

        let existing_constraints_record = transaction.fetch_optional_as(query).await?;

        if let Some(existing_record) = existing_constraints_record {
            let existing_constraints: FunctionConstraints =
                constraint_serde::deserialize(&existing_record.constraints)
                    .map_err(RepoError::Internal)?;

            let new_constraints: Option<FunctionConstraints> =
                existing_constraints.remove_constraints(constraints);

            match new_constraints {
                None => {
                    let query = sqlx::query(
                        r#"
                         DELETE FROM component_constraints
                          WHERE namespace = $1 AND component_id = $2
                          "#,
                    )
                    .bind(namespace)
                    .bind(component_id);

                    transaction.execute(query).await?;
                }

                Some(new_constraints) => {
                    let new_constraints: Vec<u8> = constraint_serde::serialize(&new_constraints)
                        .map_err(RepoError::Internal)?
                        .into();

                    let query = sqlx::query(
                        r#"
                         UPDATE
                           component_constraints
                            SET constraints = $1
                            WHERE namespace = $2 AND component_id = $3
                            "#,
                    )
                    .bind(new_constraints)
                    .bind(namespace)
                    .bind(component_id);

                    transaction.execute(query).await?;
                }
            }
        }

        self.db_pool
            .with_rw("component", "delete_constraints")
            .commit(transaction)
            .await?;

        Ok(())
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError> {
        let component_constraint_record = component_constraint_record.clone();
        let mut transaction = self
            .db_pool
            .with_rw("component", "create_or_update_constraint")
            .begin()
            .await?;

        let query = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1
                "#,
        )
        .bind(component_constraint_record.component_id);

        let existing_record = transaction.fetch_optional_as(query).await?;

        if let Some(existing_record) = existing_record {
            let existing_constraints = constraint_serde::deserialize(&existing_record.constraints)
                .map_err(RepoError::Internal)?;

            let new_constraints =
                constraint_serde::deserialize(&component_constraint_record.constraints)
                    .map_err(RepoError::Internal)?;

            let merged_worker_calls =
                FunctionConstraints::try_merge(vec![existing_constraints, new_constraints])
                    .map_err(RepoError::Internal)?;

            let merged_constraint_data: Vec<u8> = constraint_serde::serialize(&merged_worker_calls)
                .map_err(RepoError::Internal)?
                .into();

            let query = sqlx::query(
                r#"
                 UPDATE
                   component_constraints
                    SET constraints = $1
                    WHERE namespace = $2 AND component_id = $3
                    "#,
            )
            .bind(merged_constraint_data)
            .bind(component_constraint_record.namespace)
            .bind(component_constraint_record.component_id);

            transaction.execute(query).await?;
        } else {
            let query = sqlx::query(
                r#"
              INSERT INTO component_constraints
                (namespace, component_id, constraints)
              VALUES
                ($1, $2, $3)
               "#,
            )
            .bind(component_constraint_record.namespace)
            .bind(component_constraint_record.component_id)
            .bind(component_constraint_record.constraints);

            transaction.execute(query).await?;
        }

        self.db_pool
            .with_rw("component", "create_or_update_constraint")
            .commit(transaction)
            .await?;

        Ok(())
    }

    async fn get_constraint(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<FunctionConstraints>, RepoError> {
        let query = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1 AND namespace = $2
                "#,
        )
        .bind(component_id)
        .bind(namespace);

        let existing_record = self
            .db_pool
            .with_ro("component", "get_constraint")
            .fetch_optional_as(query)
            .await?;

        if let Some(existing_record) = existing_record {
            let existing_worker_calls_used =
                constraint_serde::deserialize(&existing_record.constraints)
                    .map_err(RepoError::Internal)?;
            Ok(Some(existing_worker_calls_used))
        } else {
            Ok(None)
        }
    }

    async fn get_installed_plugins(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        version: u64,
    ) -> Result<
        Vec<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>,
        RepoError,
    > {
        let target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: version as i64,
        };
        let mut query = self.plugin_installation_queries.get_all(owner, &target);
        let query = query
            .build_query_as::<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>();

        self.db_pool
            .with_ro("component", "get_installed_plugins")
            .fetch_all(query)
            .await
    }

    async fn apply_plugin_installation_changes(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        actions: &[PluginInstallationRepoAction<Owner>],
    ) -> Result<u64, RepoError> {
        let mut transaction = self
            .db_pool
            .with_rw("component", "apply_plugin_installation_changes")
            .begin()
            .await?;

        let query = sqlx::query(
            r#"
              WITH prev AS (SELECT component_id, version, size, metadata, created_at, component_type, available, object_store_key, transformed_object_store_key, root_package_name, root_package_version, env
                   FROM component_versions WHERE component_id = $1
                   ORDER BY version DESC
                   LIMIT 1)
              INSERT INTO component_versions
              SELECT prev.component_id, prev.version + 1, prev.size, $2, prev.metadata, prev.component_type, FALSE, prev.object_store_key, prev.transformed_object_store_key, prev.root_package_name, prev.root_package_version, prev.env FROM prev
              RETURNING *
              "#,
        ).bind(component_id).bind(Utc::now());

        let new_version = transaction.fetch_one(query).await?.get("version");

        debug!(
            "apply_plugin_installation_changes cloned old component version into version {new_version}"
        );

        let old_target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: new_version - 1,
        };
        let mut query = self.plugin_installation_queries.get_all(owner, &old_target);
        let query = query.build_query_as::<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>();

        let existing_installations = transaction.fetch_all(query).await?;

        let new_target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: new_version,
        };

        let mut new_installations = Vec::new();

        let mut to_delete = HashSet::new();
        let mut to_update = HashMap::new();
        for action in actions {
            match action {
                PluginInstallationRepoAction::Install { record } => {
                    debug!(
                        "apply_plugin_installation_changes adding new installation as {}",
                        record.installation_id
                    );
                    new_installations.push(PluginInstallationRecord {
                        target: new_target.clone(),
                        ..record.clone()
                    });
                }
                PluginInstallationRepoAction::Uninstall {
                    plugin_installation_id,
                } => {
                    to_delete.insert(*plugin_installation_id);
                }
                PluginInstallationRepoAction::Update {
                    plugin_installation_id,
                    new_priority,
                    new_parameters,
                } => {
                    to_update.insert(
                        *plugin_installation_id,
                        (*new_priority, new_parameters.clone()),
                    );
                }
            }
        }

        for installation in existing_installations {
            let old_id = installation.installation_id;

            if !to_delete.contains(&old_id) {
                if let Some((new_priority, new_parameters)) = to_update.get(&old_id) {
                    let new_id = Uuid::new_v4();
                    let new_installation = PluginInstallationRecord {
                        installation_id: new_id,
                        target: new_target.clone(),
                        priority: *new_priority,
                        parameters: new_parameters.clone(),
                        ..installation
                    };
                    new_installations.push(new_installation);

                    debug!(
                        "apply_plugin_installation_changes copying modified installation {old_id} as {new_id}"
                    );
                } else {
                    let new_id = Uuid::new_v4();
                    let new_installation = PluginInstallationRecord {
                        installation_id: new_id,
                        target: new_target.clone(),
                        ..installation
                    };
                    new_installations.push(new_installation);

                    debug!("apply_plugin_installation_changes copying installation {old_id} as {new_id}");
                }
            } else {
                debug!("apply_plugin_installation_changes deleting installation {old_id}");
            }
        }

        for installation in new_installations {
            let mut query = self.plugin_installation_queries.create(&installation);
            let query = query.build();
            transaction.execute(query).await?;
        }

        self.db_pool
            .with_rw("component", "apply_plugin_installation_changes")
            .commit(transaction)
            .await?;

        Ok(new_version as u64)
    }
}

pub mod record_metadata_serde {
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::component::ComponentMetadata as ComponentMetadataProto;
    use golem_common::model::component_metadata::ComponentMetadata;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &ComponentMetadata) -> Result<Bytes, String> {
        let proto_value: ComponentMetadataProto = ComponentMetadataProto::from(value.clone());
        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<ComponentMetadata, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: ComponentMetadataProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;
                let value = ComponentMetadata::try_from(proto_value)?;
                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}

pub mod constraint_serde {
    use bytes::{BufMut, Bytes, BytesMut};
    use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
    use golem_common::model::component_constraint::FunctionConstraints;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &FunctionConstraints) -> Result<Bytes, String> {
        let proto_value: FunctionConstraintCollectionProto =
            FunctionConstraintCollectionProto::from(value.clone());

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<FunctionConstraints, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: FunctionConstraintCollectionProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                let value = FunctionConstraints::try_from(proto_value.clone())?;

                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
