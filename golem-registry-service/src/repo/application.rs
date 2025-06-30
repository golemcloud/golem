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

use crate::repo::model::{AuditFields, BindFields, RevisionAuditFields, SqlDateTime};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::FromRow;
use std::fmt::Debug;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationRecord {
    pub application_id: Uuid,
    pub name: String,
    pub account_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationRevisionRecord {
    pub application_id: Uuid,
    pub revision_id: i64,
    pub name: String,
    pub account_id: Uuid,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
}

#[async_trait]
pub trait ApplicationRepo: Send + Sync {
    async fn get_by_name(
        &self,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ApplicationRecord>>;

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<ApplicationRecord>;
}

pub struct LoggedApplicationRepo<Repo: ApplicationRepo> {
    repo: Repo,
}

impl<Repo: ApplicationRepo> LoggedApplicationRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span(application_name: &str) -> Span {
        info_span!("application repository", application_name)
    }
}

#[async_trait]
impl<Repo: ApplicationRepo> ApplicationRepo for LoggedApplicationRepo<Repo> {
    async fn get_by_name(
        &self,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ApplicationRecord>> {
        self.repo
            .get_by_name(owner_account_id, name)
            .instrument(Self::span(name))
            .await
    }

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<ApplicationRecord> {
        self.repo
            .ensure(user_account_id, owner_account_id, name)
            .await
    }
}

pub struct DbApplicationRepo<DB: Pool> {
    db_pool: DB,
}

static METRICS_SVC_NAME: &str = "application";

impl<DB: Pool> DbApplicationRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DB::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DB::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(
    golem_service_base::db::postgres::PostgresPool ->
        golem_service_base::db::postgres::PostgresPool,
        golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ApplicationRepo for DbApplicationRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn get_by_name(
        &self,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ApplicationRecord>> {
        self.with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT application_id, name, account_id, created_at, updated_at, deleted_at, modified_by
                    FROM applications
                    WHERE account_id = $1 AND name = $2 AND deleted_at IS NULL
                "#})
                .bind(owner_account_id)
                .bind(name),
            )
            .await
    }

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<ApplicationRecord> {
        if let Some(app) = self.get_by_name(owner_account_id, name).await? {
            return Ok(app);
        }

        let result: repo::Result<ApplicationRecord> = {
            self.with_rw("ensure - insert")
                .fetch_one_as(
                    sqlx::query_as(indoc! {r#"
                        INSERT INTO applications (application_id, name, account_id, created_at, updated_at, deleted_at, modified_by)
                        VALUES ($1, $2, $3, $4, $5, $6, $7)
                        RETURNING application_id, name, account_id, created_at, updated_at, deleted_at, modified_by
                    "#})
                        .bind(Uuid::new_v4())
                        .bind(name)
                        .bind(owner_account_id)
                        .bind_audit_fields(AuditFields::new(*user_account_id))
            )
            .await
        };

        let result = match result {
            Err(err) if err.is_unique_violation() => None,
            result => Some(result),
        };
        if let Some(result) = result {
            return result;
        }

        match self.get_by_name(owner_account_id, name).await? {
            Some(app) => Ok(app),
            None => Err(RepoError::Internal(
                "illegal state: missing application".to_string(),
            )),
        }
    }
}
