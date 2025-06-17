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

use crate::model::{Project, ProjectData, ProjectPluginInstallationTarget, ProjectType};
use crate::repo::plugin_installation::ProjectPluginInstallationTargetRow;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::{future, TryFutureExt};
use golem_common::model::ProjectId;
use golem_common::repo::PluginOwnerRow;
use golem_service_base::db::Pool;
use golem_service_base::repo::plugin_installation::{
    DbPluginInstallationRepoQueries, PluginInstallationRecord, PluginInstallationRepoQueries,
};
use golem_service_base::repo::RepoError;
use sqlx::{QueryBuilder, Row};
use std::result::Result;
use std::sync::Arc;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ProjectRecord {
    pub project_id: Uuid,
    pub name: String,
    pub owner_account_id: String,
    pub description: String,
    pub is_default: bool,
}

impl From<ProjectRecord> for Project {
    fn from(value: ProjectRecord) -> Self {
        let project_type = if value.is_default {
            ProjectType::Default
        } else {
            ProjectType::NonDefault
        };
        Project {
            project_id: ProjectId(value.project_id),
            project_data: ProjectData {
                name: value.name,
                owner_account_id: value.owner_account_id.as_str().into(),
                description: value.description,
                default_environment_id: "default".to_string(),
                project_type,
            },
        }
    }
}

impl From<Project> for ProjectRecord {
    fn from(value: Project) -> Self {
        Self {
            project_id: value.project_id.0,
            name: value.project_data.name,
            owner_account_id: value.project_data.owner_account_id.value,
            description: value.project_data.description,
            is_default: value.project_data.project_type == ProjectType::Default,
        }
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct OwnerAccountIdRow {
    pub owner_account_id: String,
}

#[async_trait]
pub trait ProjectRepo: Send + Sync {
    async fn create(&self, project: &ProjectRecord) -> Result<(), RepoError>;

    async fn get(&self, project_id: &Uuid) -> Result<Option<ProjectRecord>, RepoError>;

    async fn get_all(&self) -> Result<Vec<ProjectRecord>, RepoError>;

    async fn get_owned(
        &self,
        account_id: &str,
        additional_projects: &[Uuid],
    ) -> Result<Vec<ProjectRecord>, RepoError>;

    /// get owners of the accounts. Will not error if one of the projects was not found.
    async fn get_owners(&self, project_ids: &[Uuid]) -> Result<Vec<String>, RepoError>;

    async fn get_owned_count(&self, account_id: &str) -> Result<u64, RepoError>;

    async fn get_default(&self, account_id: &str) -> Result<Option<ProjectRecord>, RepoError>;

    async fn delete(&self, project_id: &Uuid) -> Result<(), RepoError>;

    async fn get_installed_plugins(
        &self,
        owner: &PluginOwnerRow,
        project_id: &Uuid,
    ) -> Result<Vec<PluginInstallationRecord<ProjectPluginInstallationTarget>>, RepoError>;

    async fn install_plugin(
        &self,
        record: &PluginInstallationRecord<ProjectPluginInstallationTarget>,
    ) -> Result<(), RepoError>;

    async fn uninstall_plugin(
        &self,
        owner: &PluginOwnerRow,
        project_id: &Uuid,
        plugin_installation_id: &Uuid,
    ) -> Result<(), RepoError>;

    async fn update_plugin_installation(
        &self,
        owner: &PluginOwnerRow,
        project_id: &Uuid,
        plugin_installation_id: &Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> Result<(), RepoError>;
}

pub struct DbProjectRepo<DB: Pool> {
    db_pool: DB,
    plugin_installation_queries: Arc<
        dyn PluginInstallationRepoQueries<DB::Db, ProjectPluginInstallationTarget> + Send + Sync,
    >,
}

impl<DB: Pool + Sync> DbProjectRepo<DB>
where
    DbPluginInstallationRepoQueries<DB::Db>:
        PluginInstallationRepoQueries<DB::Db, ProjectPluginInstallationTarget>,
{
    pub fn new(db_pool: DB) -> Self {
        let plugin_installation_queries = Arc::new(DbPluginInstallationRepoQueries::new());
        Self {
            db_pool,
            plugin_installation_queries,
        }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ProjectRepo for DbProjectRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, project: &ProjectRecord) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.with_rw("project", "create").begin().await?;

        let query = sqlx::query(
            r#"
            INSERT INTO projects
                (project_id, name, description)
            VALUES
                ($1, $2, $3)
            "#,
        )
        .bind(project.project_id)
        .bind(project.name.clone())
        .bind(project.description.clone());

        transaction.execute(query).await?;

        let query = sqlx::query(
            r#"
            INSERT INTO project_account
                (project_id, owner_account_id, is_default)
            VALUES
                ($1, $2, $3)
            "#,
        )
        .bind(project.project_id)
        .bind(project.owner_account_id.clone())
        .bind(project.is_default);

        transaction.execute(query).await?;

        self.db_pool
            .with_rw("project", "create")
            .commit(transaction)
            .await?;

        Ok(())
    }

    async fn get(&self, project_id: &Uuid) -> Result<Option<ProjectRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectRecord>(
            r#"
            SELECT * FROM project_account pa
            JOIN projects p ON pa.project_id = p.project_id
            WHERE
            p.project_id = $1
            "#,
        )
        .bind(project_id);

        self.db_pool
            .with_ro("project", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn get_all(&self) -> Result<Vec<ProjectRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectRecord>(
            r#"
            SELECT * FROM project_account pa
            JOIN projects p ON pa.project_id = p.project_id
            "#,
        );

        self.db_pool
            .with_ro("project", "get_all")
            .fetch_all(query)
            .await
    }

    async fn get_owned(
        &self,
        account_id: &str,
        additional_project_ids: &[Uuid],
    ) -> Result<Vec<ProjectRecord>, RepoError> {
        let mut query = QueryBuilder::new(
            r#"
            SELECT * FROM project_account pa
            JOIN projects p ON pa.project_id = p.project_id
            WHERE
                pa.owner_account_id =
            "#,
        );
        query.push_bind(account_id);

        if !additional_project_ids.is_empty() {
            query.push("OR p.project_id IN (");

            {
                let mut in_list = query.separated(", ");
                for project_id in additional_project_ids {
                    in_list.push_bind(project_id);
                }
                in_list.push_unseparated(")");
            }
        }

        self.db_pool
            .with_ro("project", "get_owned")
            .fetch_all(query.build_query_as::<ProjectRecord>())
            .await
    }

    async fn get_owned_count(&self, account_id: &str) -> Result<u64, RepoError> {
        let query = sqlx::query(
            r#"
            SELECT count(distinct p.project_id) AS project_count
            FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
            WHERE pa.owner_account_id = $1
            "#,
        )
        .bind(account_id);

        let result = self
            .db_pool
            .with_ro("project", "get_owned_count")
            .fetch_optional(query)
            .and_then(|row| match row {
                Some(row) => future::ok(row),
                None => future::err(sqlx::Error::RowNotFound.into()),
            })
            .await?;

        let count: i64 = result.get("project_count");
        Ok(count as u64)
    }

    async fn get_owners(&self, project_ids: &[Uuid]) -> Result<Vec<String>, RepoError> {
        if project_ids.is_empty() {
            return Ok(Vec::new());
        };

        let mut query =
            QueryBuilder::new("SELECT owner_account_id FROM project_account WHERE project_id IN (");

        {
            let mut in_list = query.separated(", ");
            for project_id in project_ids {
                in_list.push_bind(project_id);
            }
            in_list.push_unseparated(") ");
        }

        self.db_pool
            .with_ro("project", "get_by_ids")
            .fetch_all(query.build_query_as::<OwnerAccountIdRow>())
            .await
            .map(|vs| vs.into_iter().map(|v| v.owner_account_id).collect())
    }

    async fn get_default(&self, account_id: &str) -> Result<Option<ProjectRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectRecord>(
            r#"
            SELECT * FROM project_account pa JOIN projects p ON pa.project_id = p.project_id
            WHERE pa.owner_account_id = $1 AND pa.is_default = true
            "#,
        )
        .bind(account_id);

        self.db_pool
            .with_ro("project", "get_own_default")
            .fetch_optional_as(query)
            .await
    }

    async fn delete(&self, project_id: &Uuid) -> Result<(), RepoError> {
        let mut transaction = self.db_pool.with_rw("project", "delete").begin().await?;

        transaction
            .execute(
                sqlx::query("DELETE FROM project_account WHERE project_id = $1").bind(project_id),
            )
            .await?;

        transaction
            .execute(
                sqlx::query("DELETE FROM project_grants WHERE grantor_project_id = $1")
                    .bind(project_id),
            )
            .await?;

        transaction
            .execute(sqlx::query("DELETE FROM projects WHERE project_id = $1").bind(project_id))
            .await?;

        self.db_pool
            .with_rw("project", "delete")
            .commit(transaction)
            .await?;

        Ok(())
    }

    async fn get_installed_plugins(
        &self,
        owner: &PluginOwnerRow,
        project_id: &Uuid,
    ) -> Result<Vec<PluginInstallationRecord<ProjectPluginInstallationTarget>>, RepoError> {
        let target = ProjectPluginInstallationTargetRow {
            project_id: *project_id,
        };
        let mut query = self.plugin_installation_queries.get_all(owner, &target);

        let query =
            query.build_query_as::<PluginInstallationRecord<ProjectPluginInstallationTarget>>();

        Ok(self
            .db_pool
            .with_ro("project", "get_installed_plugins")
            .fetch_all(query)
            .await?)
    }

    async fn install_plugin(
        &self,
        record: &PluginInstallationRecord<ProjectPluginInstallationTarget>,
    ) -> Result<(), RepoError> {
        let mut query = self.plugin_installation_queries.create(record);

        self.db_pool
            .with_rw("project", "install_plugin")
            .execute(query.build())
            .await?;

        Ok(())
    }

    async fn uninstall_plugin(
        &self,
        owner: &PluginOwnerRow,
        project_id: &Uuid,
        plugin_installation_id: &Uuid,
    ) -> Result<(), RepoError> {
        let target_row = ProjectPluginInstallationTargetRow {
            project_id: *project_id,
        };
        let mut query =
            self.plugin_installation_queries
                .delete(owner, &target_row, plugin_installation_id);

        self.db_pool
            .with_rw("project", "uninstall_plugin")
            .execute(query.build())
            .await?;

        Ok(())
    }

    async fn update_plugin_installation(
        &self,
        owner: &PluginOwnerRow,
        project_id: &Uuid,
        plugin_installation_id: &Uuid,
        new_priority: i32,
        new_parameters: Vec<u8>,
    ) -> Result<(), RepoError> {
        let target_row = ProjectPluginInstallationTargetRow {
            project_id: *project_id,
        };
        let mut query = self.plugin_installation_queries.update(
            owner,
            &target_row,
            plugin_installation_id,
            new_priority,
            new_parameters,
        );

        self.db_pool
            .with_rw("project", "update_plugin_installation")
            .execute(query.build())
            .await?;

        Ok(())
    }
}
