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

use crate::model::{Component, ComponentConstraints};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use conditional_trait_gen::{trait_gen, when};
use futures::future::try_join_all;
use golem_common::model::component::ComponentOwner;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::plugin::{ComponentPluginInstallationTarget, PluginOwner};
use golem_common::model::{
    ComponentFilePath, ComponentFilePermissions, ComponentId, ComponentType, InitialComponentFile,
    InitialComponentFileKey,
};
use golem_common::repo::plugin_installation::ComponentPluginInstallationRow;
use golem_service_base::model::{ComponentName, VersionedComponentId};
use golem_service_base::repo::plugin_installation::{
    DbPluginInstallationRepoQueries, PluginInstallationRecord, PluginInstallationRepoQueries,
};
use golem_service_base::repo::RepoError;
use sqlx::{Database, Pool, Row};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;
use tracing::{debug, error};
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
        let function_constraints: FunctionConstraintCollection =
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
pub trait ComponentRepo<Owner: ComponentOwner>: Debug {
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

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError>;

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError>;

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError>;

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError>;

    async fn get_constraint(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<FunctionConstraintCollection>, RepoError>;

    async fn get_installed_plugins(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        version: u64,
    ) -> Result<
        Vec<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>,
        RepoError,
    >;

    async fn install_plugin(
        &self,
        record: &PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>,
    ) -> Result<u64, RepoError>;

    async fn uninstall_plugin(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        plugin_installation_id: &Uuid,
    ) -> Result<u64, RepoError>;

    async fn update_plugin_installation(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        plugin_installation_id: &Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> Result<u64, RepoError>;
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

    fn logged<R>(message: &'static str, result: Result<R, RepoError>) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!("{}", message),
            Err(error) => error!(error = error.to_string(), "{message}"),
        }
        result
    }

    fn logged_with_id<R>(
        message: &'static str,
        component_id: &Uuid,
        result: Result<R, RepoError>,
    ) -> Result<R, RepoError> {
        match &result {
            Ok(_) => debug!(component_id = component_id.to_string(), "{}", message),
            Err(error) => error!(
                component_id = component_id.to_string(),
                error = error.to_string(),
                "{message}"
            ),
        }
        result
    }
}

#[async_trait]
impl<Owner: ComponentOwner, Repo: ComponentRepo<Owner> + Send + Sync> ComponentRepo<Owner>
    for LoggedComponentRepo<Owner, Repo>
{
    async fn create(&self, component: &ComponentRecord<Owner>) -> Result<(), RepoError> {
        let result = self.repo.create(component).await;
        Self::logged_with_id("create", &component.component_id, result)
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
    ) -> Result<ComponentRecord<Owner>, RepoError> {
        let result = self
            .repo
            .update(
                owner,
                namespace,
                component_id,
                data,
                metadata,
                component_type,
                files,
            )
            .await;
        Self::logged_with_id("update", component_id, result)
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
        let result = self
            .repo
            .activate(
                namespace,
                component_id,
                component_version,
                object_store_key,
                transformed_object_store_key,
                updated_metadata,
            )
            .await;
        Self::logged_with_id("activate", component_id, result)
    }

    async fn get(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let result = self.repo.get(namespace, component_id).await;
        Self::logged_with_id("get", component_id, result)
    }

    async fn get_all(&self, namespace: &str) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let result = self.repo.get_all(namespace).await;
        Self::logged("get_all", result)
    }

    async fn get_latest_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let result = self.repo.get_latest_version(namespace, component_id).await;
        Self::logged_with_id("get_latest_version", component_id, result)
    }

    async fn get_by_version(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let result = self
            .repo
            .get_by_version(namespace, component_id, version)
            .await;
        Self::logged_with_id("get_by_version", component_id, result)
    }

    async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let result = self.repo.get_by_name(namespace, name).await;
        Self::logged("get_by_name", result)
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        let result = self.repo.get_id_by_name(namespace, name).await;
        Self::logged("get_id_by_name", result)
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        let result = self.repo.get_namespace(component_id).await;
        Self::logged_with_id("get_namespace", component_id, result)
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        let result = self.repo.delete(namespace, component_id).await;
        Self::logged_with_id("delete", component_id, result)
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError> {
        let result = self
            .repo
            .create_or_update_constraint(component_constraint_record)
            .await;
        Self::logged("create_component_constraint", result)
    }

    async fn get_constraint(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<FunctionConstraintCollection>, RepoError> {
        let result = self.repo.get_constraint(namespace, component_id).await;
        Self::logged("get_component_constraint", result)
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
        let result = self
            .repo
            .get_installed_plugins(owner, component_id, version)
            .await;
        Self::logged_with_id("get_installed_plugins", component_id, result)
    }

    async fn install_plugin(
        &self,
        record: &PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>,
    ) -> Result<u64, RepoError> {
        let result = self.repo.install_plugin(record).await;
        Self::logged_with_id("install_plugin", &record.target.component_id, result)
    }

    async fn uninstall_plugin(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        plugin_installation_id: &Uuid,
    ) -> Result<u64, RepoError> {
        let result = self
            .repo
            .uninstall_plugin(owner, component_id, plugin_installation_id)
            .await;
        Self::logged_with_id("uninstall_plugin", component_id, result)
    }

    async fn update_plugin_installation(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        plugin_installation_id: &Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> Result<u64, RepoError> {
        let result = self
            .repo
            .update_plugin_installation(
                owner,
                component_id,
                plugin_installation_id,
                new_priority,
                new_parameters,
            )
            .await;
        Self::logged_with_id("update_plugin_installation", component_id, result)
    }
}

pub struct DbComponentRepo<DB: Database, Owner: ComponentOwner> {
    db_pool: Arc<Pool<DB>>,
    plugin_installation_queries: Arc<
        dyn PluginInstallationRepoQueries<DB, Owner::PluginOwner, ComponentPluginInstallationTarget>
            + Send
            + Sync,
    >,
}

impl<DB: Database + Sync, Owner: ComponentOwner> DbComponentRepo<DB, Owner>
where
    DbPluginInstallationRepoQueries<DB>:
        PluginInstallationRepoQueries<DB, Owner::PluginOwner, ComponentPluginInstallationTarget>,
{
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        let plugin_installation_queries = Arc::new(DbPluginInstallationRepoQueries::<DB>::new());
        Self {
            db_pool,
            plugin_installation_queries,
        }
    }
}

impl<Owner: ComponentOwner, DB: Database> Debug for DbComponentRepo<DB, Owner> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbComponentRepo")
            .field("db_pool", &self.db_pool)
            .finish()
    }
}

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
impl<Owner: ComponentOwner> DbComponentRepo<sqlx::Postgres, Owner> {
    async fn get_files(
        &self,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Vec<FileRecord>, RepoError> {
        sqlx::query_as::<_, FileRecord>(
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
        .bind(component_id)
        .bind(version as i64)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
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
        Ok(query.build_query_as::<PluginInstallationRecord<
            Owner::PluginOwner,
            ComponentPluginInstallationTarget,
        >>()
            .fetch_all(self.db_pool.deref())
            .await?)
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

#[trait_gen(sqlx::Postgres -> sqlx::Postgres, sqlx::Sqlite)]
#[async_trait]
impl<Owner: ComponentOwner> ComponentRepo<Owner> for DbComponentRepo<sqlx::Postgres, Owner> {
    async fn create(&self, component: &ComponentRecord<Owner>) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query("SELECT namespace, name FROM components WHERE component_id = $1")
            .bind(component.component_id)
            .fetch_optional(&mut *transaction)
            .await?;

        if let Some(result) = result {
            let namespace: String = result.get("namespace");
            let name: String = result.get("name");
            if namespace != component.namespace || name != component.name {
                transaction.rollback().await?;
                return Err(RepoError::Internal(
                    "Component namespace and name invalid".to_string(),
                ));
            }
        } else {
            let result = sqlx::query(
                r#"
                  INSERT INTO components
                    (namespace, component_id, name)
                  VALUES
                    ($1, $2, $3)
                   "#,
            )
            .bind(component.namespace.clone())
            .bind(component.component_id)
            .bind(component.name.clone())
            .execute(&mut *transaction)
            .await;

            if let Err(err) = result {
                // Without this explicit rollback, sqlite seems to be remain locked when a next
                // incoming request comes in.
                transaction.rollback().await?;
                return Err(err.into());
            }
        }

        sqlx::query(
            r#"
              INSERT INTO component_versions
                (component_id, version, size, metadata, created_at, component_type, available, object_store_key, transformed_object_store_key)
              VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
            .execute(&mut *transaction)
            .await?;

        for file in &component.files {
            sqlx::query(
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
            .bind(file.file_permissions.clone())
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
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
    ) -> Result<ComponentRecord<Owner>, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_optional(&mut *transaction)
            .await?;

        if let Some(result) = result {
            let existing_namespace: String = result.get("namespace");
            if existing_namespace != namespace {
                transaction.rollback().await?;
                Err(RepoError::Internal(
                    "Component namespace invalid".to_string(),
                ))
            } else {
                let now = Utc::now();
                let new_version = if let Some(component_type) = component_type {
                    sqlx::query(
                        r#"
                              WITH prev AS (SELECT component_id, version, object_store_key, transformed_object_store_key
                                   FROM component_versions WHERE component_id = $1
                                   ORDER BY version DESC
                                   LIMIT 1)
                              INSERT INTO component_versions
                              SELECT prev.component_id, prev.version + 1, $2, $3, $4, $5, FALSE, prev.object_store_key, prev.transformed_object_store_key FROM prev
                              RETURNING *
                              "#,
                    )
                        .bind(component_id)
                        .bind(data.len() as i32)
                        .bind(now)
                        .bind(metadata)
                        .bind(component_type)
                        .fetch_one(&mut *transaction)
                        .await?
                        .get("version")
                } else {
                    sqlx::query(
                        r#"
                              WITH prev AS (SELECT component_id, version, component_type, object_store_key, transformed_object_store_key
                                   FROM component_versions WHERE component_id = $1
                                   ORDER BY version DESC
                                   LIMIT 1)
                              INSERT INTO component_versions
                              SELECT prev.component_id, prev.version + 1, $2, $3, $4, prev.component_type, FALSE, prev.object_store_key, prev.transformed_object_store_key FROM prev
                              RETURNING *
                              "#,
                    )
                        .bind(component_id)
                        .bind(data.len() as i32)
                        .bind(now)
                        .bind(metadata)
                        .fetch_one(&mut *transaction)
                        .await?
                        .get("version")
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

                let existing_installations =
                    query
                        .build_query_as::<PluginInstallationRecord<
                            Owner::PluginOwner,
                            ComponentPluginInstallationTarget,
                        >>()
                        .fetch_all(&mut *transaction)
                        .await?;

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

                    query.build().execute(&mut *transaction).await?;
                }

                let files = if let Some(files) = files {
                    files
                } else {
                    // Copying the previous file set
                    sqlx::query_as::<_, FileRecord>(
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
                    .bind(old_target.component_version)
                    .fetch_all(&mut *transaction)
                    .await?
                };

                // Inserting the new file set
                for file in files {
                    sqlx::query(
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
                    .bind(&file.file_permissions)
                    .execute(&mut *transaction)
                    .await?;
                }

                transaction.commit().await?;

                let component = self
                    .get_by_version(namespace, component_id, new_version as u64)
                    .await?;

                component.ok_or(RepoError::Internal(
                    "Could not re-get newly created component version".to_string(),
                ))
            }
        } else {
            transaction.rollback().await?;
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
        sqlx::query(
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
            .bind(transformed_object_store_key)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    #[when(sqlx::Postgres -> get)]
    async fn get_postgres(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let components = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key as transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2
                "#,
        )
        .bind(component_id)
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(sqlx::Sqlite -> get)]
    async fn get_sqlite(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let components = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2
                "#,
        )
        .bind(component_id)
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(sqlx::Postgres -> get_all)]
    async fn get_all_postgres(
        &self,
        namespace: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let components = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1
                "#,
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(sqlx::Sqlite -> get_all)]
    async fn get_all_sqlite(
        &self,
        namespace: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let components = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1
                "#,
        )
        .bind(namespace)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(sqlx::Postgres -> get_latest_version)]
    async fn get_latest_version_postgres(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let component = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2 AND cv.available = TRUE
                ORDER BY cv.version DESC
                LIMIT 1
                "#,
        )
        .bind(component_id)
        .bind(namespace)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(sqlx::Sqlite -> get_latest_version)]
    async fn get_latest_version_sqlite(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let component = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND c.namespace = $2 AND cv.available = TRUE
                ORDER BY cv.version
                DESC LIMIT 1
                "#,
        )
        .bind(component_id)
        .bind(namespace)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(sqlx::Postgres -> get_by_version)]
    async fn get_by_version_postgres(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let component = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND cv.version = $2 AND c.namespace = $3
                "#,
        )
        .bind(component_id)
        .bind(version as i64)
        .bind(namespace)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(sqlx::Sqlite -> get_by_version)]
    async fn get_by_version_sqlite(
        &self,
        namespace: &str,
        component_id: &Uuid,
        version: u64,
    ) -> Result<Option<ComponentRecord<Owner>>, RepoError> {
        let component = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.component_id = $1 AND cv.version = $2 AND c.namespace = $3
                "#,
        )
        .bind(component_id)
        .bind(version as i64)
        .bind(namespace)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        Ok(self
            .add_installed_plugins(self.add_files(component).await?)
            .await?
            .pop())
    }

    #[when(sqlx::Postgres -> get_by_name)]
    async fn get_by_name_postgres(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let components = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.name = $2
                "#,
        )
        .bind(namespace)
        .bind(name)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    #[when(sqlx::Sqlite -> get_by_name)]
    async fn get_by_name_sqlite(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ComponentRecord<Owner>>, RepoError> {
        let components = sqlx::query_as::<_, ComponentRecord<Owner>>(
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
                    cv.transformed_object_store_key AS transformed_object_store_key
                FROM components c
                    JOIN component_versions cv ON c.component_id = cv.component_id
                WHERE c.namespace = $1 AND c.name = $2
                "#,
        )
        .bind(namespace)
        .bind(name)
        .fetch_all(self.db_pool.deref())
        .await
        .map_err::<RepoError, _>(|e| e.into())?;

        self.add_installed_plugins(self.add_files(components).await?)
            .await
    }

    async fn get_id_by_name(&self, namespace: &str, name: &str) -> Result<Option<Uuid>, RepoError> {
        let result =
            sqlx::query("SELECT component_id FROM components WHERE namespace = $1 AND name = $2")
                .bind(namespace)
                .bind(name)
                .fetch_optional(self.db_pool.deref())
                .await?;

        Ok(result.map(|x| x.get("component_id")))
    }

    async fn get_namespace(&self, component_id: &Uuid) -> Result<Option<String>, RepoError> {
        let result = sqlx::query("SELECT namespace FROM components WHERE component_id = $1")
            .bind(component_id)
            .fetch_optional(self.db_pool.deref())
            .await?;

        Ok(result.map(|x| x.get("namespace")))
    }

    async fn delete(&self, namespace: &str, component_id: &Uuid) -> Result<(), RepoError> {
        // TODO: delete plugin installations

        let mut transaction = self.db_pool.begin().await?;
        sqlx::query(
            r#"
                DELETE FROM component_versions
                WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
            "#
        )
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query(
            r#"
                DELETE FROM component_files
                WHERE component_id IN (SELECT component_id FROM components WHERE namespace = $1 AND component_id = $2)
            "#
        )
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("DELETE FROM components WHERE namespace = $1 AND component_id = $2")
            .bind(namespace)
            .bind(component_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(())
    }

    async fn create_or_update_constraint(
        &self,
        component_constraint_record: &ComponentConstraintsRecord,
    ) -> Result<(), RepoError> {
        let component_constraint_record = component_constraint_record.clone();
        let mut transaction = self.db_pool.begin().await?;

        let existing_record = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1
                "#,
        )
        .bind(component_constraint_record.component_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|e| RepoError::Internal(e.to_string()))?;

        if let Some(existing_record) = existing_record {
            let existing_worker_calls_used =
                constraint_serde::deserialize(&existing_record.constraints)
                    .map_err(RepoError::Internal)?;
            let new_worker_calls_used =
                constraint_serde::deserialize(&component_constraint_record.constraints)
                    .map_err(RepoError::Internal)?;

            // This shouldn't happen as it is validated in service layers.
            // However, repo gives us more transactional guarantee.
            let merged_worker_calls = FunctionConstraintCollection::try_merge(vec![
                existing_worker_calls_used,
                new_worker_calls_used,
            ])
            .map_err(RepoError::Internal)?;

            // Serialize the merged result back to store in the database
            let merged_constraint_data: Vec<u8> = constraint_serde::serialize(&merged_worker_calls)
                .map_err(RepoError::Internal)?
                .into();

            // Update the existing record in the database
            sqlx::query(
                r#"
                 UPDATE
                   component_constraints
                    SET constraints = $1
                    WHERE namespace = $2 AND component_id = $3
                    "#,
            )
            .bind(merged_constraint_data)
            .bind(component_constraint_record.namespace)
            .bind(component_constraint_record.component_id)
            .execute(&mut *transaction)
            .await
            .map_err(RepoError::from)?;
        } else {
            sqlx::query(
                r#"
              INSERT INTO component_constraints
                (namespace, component_id, constraints)
              VALUES
                ($1, $2, $3)
               "#,
            )
            .bind(component_constraint_record.namespace)
            .bind(component_constraint_record.component_id)
            .bind(component_constraint_record.constraints)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        Ok(())
    }

    async fn get_constraint(
        &self,
        namespace: &str,
        component_id: &Uuid,
    ) -> Result<Option<FunctionConstraintCollection>, RepoError> {
        let existing_record = sqlx::query_as::<_, ComponentConstraintsRecord>(
            r#"
                SELECT
                    namespace,
                    component_id,
                    constraints
                FROM component_constraints WHERE component_id = $1 AND namespace = $2
                "#,
        )
        .bind(component_id)
        .bind(namespace)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| RepoError::Internal(e.to_string()))?;

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

        Ok(query
            .build_query_as::<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>()
            .fetch_all(self.db_pool.deref())
            .await?)
    }

    async fn install_plugin(
        &self,
        record: &PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>,
    ) -> Result<u64, RepoError> {
        let component_id = record.target.component_id;

        let mut transaction = self.db_pool.begin().await?;

        let new_version = sqlx::query(
            r#"
              WITH prev AS (SELECT component_id, version, size, metadata, created_at, component_type, available, object_store_key, transformed_object_store_key
                   FROM component_versions WHERE component_id = $1
                   ORDER BY version DESC
                   LIMIT 1)
              INSERT INTO component_versions
              SELECT prev.component_id, prev.version + 1, prev.size, $2, prev.metadata, prev.component_type, FALSE, prev.object_store_key, prev.transformed_object_store_key FROM prev
              RETURNING *
              "#,
        )
            .bind(component_id)
            .bind(Utc::now())
            .fetch_one(&mut *transaction)
            .await?
            .get("version");

        debug!("install_plugin cloned old component version into version {new_version}");

        let old_target = ComponentPluginInstallationRow {
            component_id,
            component_version: new_version - 1,
        };
        let mut query = self
            .plugin_installation_queries
            .get_all(&record.owner, &old_target);

        let existing_installations = query
            .build_query_as::<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>()
            .fetch_all(&mut *transaction)
            .await?;

        let new_target = ComponentPluginInstallationRow {
            component_id,
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

            debug!("install_plugin copying installation {old_id} as {new_id}");
        }
        debug!(
            "install_plugin adding new installation as {}",
            record.installation_id
        );
        new_installations.push(PluginInstallationRecord {
            target: new_target.clone(),
            ..record.clone()
        });

        for installation in new_installations {
            let mut query = self.plugin_installation_queries.create(&installation);

            query.build().execute(&mut *transaction).await?;
        }

        transaction.commit().await?;

        Ok(new_version as u64)
    }

    async fn uninstall_plugin(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        plugin_installation_id: &Uuid,
    ) -> Result<u64, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let new_version = sqlx::query(
            r#"
              WITH prev AS (SELECT component_id, version, size, metadata, created_at, component_type, available, object_store_key, transformed_object_store_key
                   FROM component_versions WHERE component_id = $1
                   ORDER BY version DESC
                   LIMIT 1)
              INSERT INTO component_versions
              SELECT prev.component_id, prev.version + 1, prev.size, $2, prev.metadata, prev.component_type, FALSE, prev.object_store_key, prev.transformed_object_store_key FROM prev
              RETURNING *
              "#,
        )
            .bind(component_id)
            .bind(Utc::now())
            .fetch_one(&mut *transaction)
            .await?
            .get("version");

        debug!("uninstall_plugin cloned old component version into version {new_version}");

        let old_target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: new_version - 1,
        };
        let mut query = self.plugin_installation_queries.get_all(owner, &old_target);

        let existing_installations = query
            .build_query_as::<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>()
            .fetch_all(&mut *transaction)
            .await?;

        let new_target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: new_version,
        };

        let mut new_installations = Vec::new();
        for installation in existing_installations {
            let old_id = installation.installation_id;

            if &old_id != plugin_installation_id {
                let new_id = Uuid::new_v4();
                let new_installation = PluginInstallationRecord {
                    installation_id: new_id,
                    target: new_target.clone(),
                    ..installation
                };
                new_installations.push(new_installation);

                debug!("uninstall_plugin copying installation {old_id} as {new_id}");
            }
        }

        for installation in new_installations {
            let mut query = self.plugin_installation_queries.create(&installation);

            query.build().execute(&mut *transaction).await?;
        }

        transaction.commit().await?;

        Ok(new_version as u64)
    }

    async fn update_plugin_installation(
        &self,
        owner: &<<Owner as ComponentOwner>::PluginOwner as PluginOwner>::Row,
        component_id: &Uuid,
        plugin_installation_id: &Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> Result<u64, RepoError> {
        let mut transaction = self.db_pool.begin().await?;

        let new_version = sqlx::query(
            r#"
              WITH prev AS (SELECT component_id, version, size, metadata, created_at, component_type, available, object_store_key, transformed_object_store_key
                   FROM component_versions WHERE component_id = $1
                   ORDER BY version DESC
                   LIMIT 1)
              INSERT INTO component_versions
              SELECT prev.component_id, prev.version + 1, prev.size, $2, prev.metadata, prev.component_type, FALSE, prev.object_store_key, prev.transformed_object_store_key FROM prev
              RETURNING *
              "#,
        )
            .bind(component_id)
            .bind(Utc::now())
            .fetch_one(&mut *transaction)
            .await?
            .get("version");

        debug!(
            "update_plugin_installation cloned old component version into version {new_version}"
        );

        let old_target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: new_version - 1,
        };
        let mut query = self.plugin_installation_queries.get_all(owner, &old_target);

        let existing_installations = query
            .build_query_as::<PluginInstallationRecord<Owner::PluginOwner, ComponentPluginInstallationTarget>>()
            .fetch_all(&mut *transaction)
            .await?;

        let new_target = ComponentPluginInstallationRow {
            component_id: *component_id,
            component_version: new_version,
        };

        let mut new_installations = Vec::new();
        for installation in existing_installations {
            let old_id = installation.installation_id;

            if &old_id != plugin_installation_id {
                let new_id = Uuid::new_v4();
                let new_installation = PluginInstallationRecord {
                    installation_id: new_id,
                    target: new_target.clone(),
                    ..installation
                };
                new_installations.push(new_installation);

                debug!("update_plugin_installation copying installation {old_id} as {new_id}");
            } else {
                let new_id = Uuid::new_v4();
                let new_installation = PluginInstallationRecord {
                    installation_id: new_id,
                    target: new_target.clone(),
                    priority: new_priority,
                    parameters: new_parameters.clone(),
                    ..installation
                };
                new_installations.push(new_installation);

                debug!(
                    "update_plugin_installation copying modified installation {old_id} as {new_id}"
                );
            }
        }

        for installation in new_installations {
            let mut query = self.plugin_installation_queries.create(&installation);

            query.build().execute(&mut *transaction).await?;
        }

        transaction.commit().await?;

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
    use golem_common::model::component_constraint::FunctionConstraintCollection;
    use prost::Message;

    pub const SERIALIZATION_VERSION_V1: u8 = 1u8;

    pub fn serialize(value: &FunctionConstraintCollection) -> Result<Bytes, String> {
        let proto_value: FunctionConstraintCollectionProto =
            FunctionConstraintCollectionProto::from(value.clone());

        let mut bytes = BytesMut::new();
        bytes.put_u8(SERIALIZATION_VERSION_V1);
        bytes.extend_from_slice(&proto_value.encode_to_vec());
        Ok(bytes.freeze())
    }

    pub fn deserialize(bytes: &[u8]) -> Result<FunctionConstraintCollection, String> {
        let (version, data) = bytes.split_at(1);

        match version[0] {
            SERIALIZATION_VERSION_V1 => {
                let proto_value: FunctionConstraintCollectionProto = Message::decode(data)
                    .map_err(|e| format!("Failed to deserialize value: {e}"))?;

                let value = FunctionConstraintCollection::try_from(proto_value.clone())?;

                Ok(value)
            }
            _ => Err("Unsupported serialization version".to_string()),
        }
    }
}
