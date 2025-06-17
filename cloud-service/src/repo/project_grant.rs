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

use crate::model::{ProjectGrant, ProjectGrantData};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::ProjectId;
use golem_common::model::{ProjectGrantId, ProjectPolicyId};
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use std::result::Result;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ProjectGrantRecord {
    pub project_grant_id: Uuid,
    pub grantor_project_id: Uuid,
    pub grantee_account_id: String,
    pub project_policy_id: Uuid,
}

impl From<ProjectGrantRecord> for ProjectGrant {
    fn from(value: ProjectGrantRecord) -> Self {
        ProjectGrant {
            id: ProjectGrantId(value.project_grant_id),
            data: ProjectGrantData {
                grantor_project_id: ProjectId(value.grantor_project_id),
                grantee_account_id: value.grantee_account_id.as_str().into(),
                project_policy_id: ProjectPolicyId(value.project_policy_id),
            },
        }
    }
}

impl From<ProjectGrant> for ProjectGrantRecord {
    fn from(value: ProjectGrant) -> Self {
        Self {
            project_grant_id: value.id.0,
            grantor_project_id: value.data.grantor_project_id.0,
            grantee_account_id: value.data.grantee_account_id.value,
            project_policy_id: value.data.project_policy_id.0,
        }
    }
}

#[async_trait]
pub trait ProjectGrantRepo: Send + Sync {
    async fn create(&self, project_grant: &ProjectGrantRecord) -> Result<(), RepoError>;

    async fn get(&self, project_grant_id: &Uuid) -> Result<Option<ProjectGrantRecord>, RepoError>;

    async fn get_by_grantee_account(
        &self,
        grantee_account_id: &str,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError>;

    async fn get_by_project(&self, project_id: &Uuid)
        -> Result<Vec<ProjectGrantRecord>, RepoError>;

    async fn delete(&self, project_grant_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbProjectGrantRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbProjectGrantRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ProjectGrantRepo for DbProjectGrantRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, project_grant: &ProjectGrantRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
            r#"
              INSERT INTO project_grants
                (project_grant_id, grantor_project_id, project_policy_id, grantee_account_id)
              VALUES
                ($1, $2, $3, $4)
            "#,
        )
        .bind(project_grant.project_grant_id)
        .bind(project_grant.grantor_project_id)
        .bind(project_grant.project_policy_id)
        .bind(project_grant.grantee_account_id.clone());

        self.db_pool
            .with_rw("project_grant", "create")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn get(&self, project_grant_id: &Uuid) -> Result<Option<ProjectGrantRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE project_grant_id = $1",
        )
        .bind(project_grant_id);

        self.db_pool
            .with_ro("project_grant", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn get_by_project(
        &self,
        project_id: &Uuid,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE grantor_project_id = $1",
        )
        .bind(project_id);

        self.db_pool
            .with_ro("project_grant", "get_by_project")
            .fetch_all(query)
            .await
    }

    async fn get_by_grantee_account(
        &self,
        grantee_account_id: &str,
    ) -> Result<Vec<ProjectGrantRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectGrantRecord>(
            "SELECT * FROM project_grants WHERE grantee_account_id = $1",
        )
        .bind(grantee_account_id);

        self.db_pool
            .with_ro("project_grant", "get_by_account")
            .fetch_all(query)
            .await
    }

    async fn delete(&self, project_grant_id: &Uuid) -> Result<(), RepoError> {
        let query = sqlx::query("DELETE FROM project_grants WHERE project_grant_id = $1")
            .bind(project_grant_id);

        self.db_pool
            .with_rw("project_grant", "delete")
            .execute(query)
            .await?;

        Ok(())
    }
}
