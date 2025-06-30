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

use crate::model::account::Account;
use crate::repo::model::{AuditFields, BindFields, RevisionAuditFields, SqlDateTime};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::{AccountId, PlanId};
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo;
use indoc::indoc;
use sqlx::FromRow;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub account_id: Uuid,
    pub email: String,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub name: String,
    pub plan_id: Uuid,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountRevisionRecord {
    pub account_id: Uuid,
    pub email: String,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub name: String,
    pub plan_id: Uuid,
}

#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn create(&self, account: AccountRecord) -> repo::Result<Option<AccountRecord>>;

    async fn get_by_id(&self, account_id: &Uuid) -> repo::Result<Option<AccountRecord>>;

    async fn get_by_email(&self, email: &str) -> repo::Result<Option<AccountRecord>>;
}

pub struct LoggedAccountRepo<Repo: AccountRepo> {
    repo: Repo,
}

impl<Repo: AccountRepo> LoggedAccountRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_account_id(account_id: &Uuid) -> Span {
        info_span!("account repository", account_id=%account_id)
    }

    fn span_email(email: &str) -> Span {
        info_span!("account repository", email)
    }
}

#[async_trait]
impl<Repo: AccountRepo> AccountRepo for LoggedAccountRepo<Repo> {
    async fn create(&self, account: AccountRecord) -> repo::Result<Option<AccountRecord>> {
        let span = Self::span_account_id(&account.account_id);
        self.repo.create(account).instrument(span).await
    }

    async fn get_by_id(&self, account_id: &Uuid) -> repo::Result<Option<AccountRecord>> {
        self.repo
            .get_by_id(account_id)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_by_email(&self, email: &str) -> repo::Result<Option<AccountRecord>> {
        let span = Self::span_email(email);
        self.repo.get_by_email(email).instrument(span).await
    }
}

pub struct DbAccountRepo<DB: Pool> {
    db_pool: DB,
}

static METRICS_SVC_NAME: &str = "account";

impl<DB: Pool> DbAccountRepo<DB> {
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
impl AccountRepo for DbAccountRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, account: AccountRecord) -> repo::Result<Option<AccountRecord>> {
        let result = self
            .with_rw("create")
            .fetch_one_as(
                sqlx::query_as(indoc! {r#"
                    INSERT INTO accounts (account_id, email, created_at, updated_at, deleted_at, modified_by, name, plan_id)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    RETURNING account_id, email, created_at, updated_at, deleted_at, modified_by, name, plan_id
                "#})
                    .bind(account.account_id)
                    .bind(account.email)
                    .bind_audit_fields(account.audit)
                    .bind(account.name)
                    .bind(account.plan_id),
            )
            .await;

        match result {
            Ok(account) => Ok(Some(account)),
            Err(err) if err.is_unique_violation() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn get_by_id(&self, account_id: &Uuid) -> repo::Result<Option<AccountRecord>> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT account_id, email, created_at, updated_at, deleted_at, modified_by, name, plan_id
                    FROM accounts
                    WHERE account_id = $1 AND deleted_at IS NULL
                "#})
                .bind(account_id),
            )
            .await
    }

    async fn get_by_email(&self, email: &str) -> repo::Result<Option<AccountRecord>> {
        self.with_ro("get_by_email")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT account_id, email, created_at, updated_at, deleted_at, modified_by, name, plan_id
                    FROM accounts
                    WHERE email = $1 AND deleted_at IS NULL
                "#})
                .bind(email),
            )
            .await
    }
}
