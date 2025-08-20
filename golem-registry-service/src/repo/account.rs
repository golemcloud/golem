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

use super::model::account::{AccountRevisionRecord, AccountRoleRecord, JoinedAccountRecord};
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::account::AccountRepoError;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoResult, ResultExt};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn create(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<JoinedAccountRecord, AccountRepoError>;

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<JoinedAccountRecord, AccountRepoError>;

    async fn get_by_id(
        &self,
        account_id: &Uuid,
    ) -> Result<Option<JoinedAccountRecord>, AccountRepoError>;

    async fn get_by_email(
        &self,
        email: &str,
    ) -> Result<Option<JoinedAccountRecord>, AccountRepoError>;
}

pub struct LoggedAccountRepo<Repo: AccountRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "account repository";

impl<Repo: AccountRepo> LoggedAccountRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_account_id(account_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, account_id=%account_id)
    }

    fn span_email(email: &str) -> Span {
        info_span!(SPAN_NAME, email)
    }
}

#[async_trait]
impl<Repo: AccountRepo> AccountRepo for LoggedAccountRepo<Repo> {
    async fn create(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<JoinedAccountRecord, AccountRepoError> {
        let span = Self::span_account_id(&revision.account_id);
        self.repo.create(revision).instrument(span).await
    }

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<JoinedAccountRecord, AccountRepoError> {
        let span = Self::span_account_id(&revision.account_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        account_id: &Uuid,
    ) -> Result<Option<JoinedAccountRecord>, AccountRepoError> {
        self.repo
            .get_by_id(account_id)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_by_email(
        &self,
        email: &str,
    ) -> Result<Option<JoinedAccountRecord>, AccountRepoError> {
        let span = Self::span_email(email);
        self.repo.get_by_email(email).instrument(span).await
    }
}

pub struct DbAccountRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "account";

impl<DBP: Pool> DbAccountRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedAccountRepo<Self>
    where
        Self: AccountRepo,
    {
        LoggedAccountRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbAccountRepo<PostgresPool> {
    async fn insert_one_role(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: AccountRoleRecord,
    ) -> RepoResult<AccountRoleRecord> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! {r#"
                    INSERT INTO account_revision_roles (account_id, revision_id, role)
                    VALUES ($1, $2, $3)
                    RETURNING account_id, revision_id, role
                "#})
            .bind(record.account_id)
            .bind(record.revision_id)
            .bind(record.role),
        )
        .await
    }

    async fn get_roles(
        &self,
        account_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<AccountRoleRecord>> {
        self.with_ro("get_roles")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT account_id, revision_id, role
                    FROM account_revision_roles
                    WHERE account_id = $1 AND revision_id = $2
                "#})
                .bind(account_id)
                .bind(revision_id),
            )
            .await
    }

    async fn insert_revision(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: AccountRevisionRecord,
    ) -> Result<AccountRevisionRecord, AccountRepoError> {
        let original_roles = revision.roles;

        let mut revision: AccountRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO account_revisions
                    (account_id, revision_id, name, email, plan_id,
                        created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    RETURNING account_id, revision_id, name, email,
                        plan_id, created_at, created_by,
                        deleted
                "# })
                .bind(revision.account_id)
                .bind(revision.revision_id)
                .bind(revision.name)
                .bind(revision.email)
                .bind(revision.plan_id)
                .bind_deletable_revision_audit(revision.audit),
            )
            .await
            .to_custom_result_on_unique_violation(AccountRepoError::VersionAlreadyExists {
                version: revision.revision_id,
            })?;

        revision.roles = {
            let mut inserted_roles = Vec::with_capacity(revision.roles.len());
            for role in original_roles {
                inserted_roles.push(Self::insert_one_role(tx, role).await?);
            }
            inserted_roles
        };

        Ok(revision)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountRepo for DbAccountRepo<PostgresPool> {
    async fn create(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<JoinedAccountRecord, AccountRepoError> {
        self.db_pool.with_tx_custom_error(METRICS_SVC_NAME, "create", |tx| {
            async move {
                let account_record: AccountRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! {r#"
                            INSERT INTO accounts (account_id, email, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                            VALUES ($1, $2, $3, $4, NULL, $5, $6)
                            RETURNING account_id, email, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(revision.account_id)
                            .bind(&revision.email)
                            .bind(&revision.audit.created_at)
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                    )
                    .await
                    .to_custom_result_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?;

                let revision_record = Self::insert_revision(tx, revision).await?;

                Ok(JoinedAccountRecord {
                    account_id: account_record.account_id,
                    current_revision_id: account_record.current_revision_id,
                    email: account_record.email,
                    name: revision_record.name,
                    plan_id: revision_record.plan_id,
                    audit: account_record.audit,
                    roles: revision_record.roles
                })
            }.boxed()
        }).await
    }

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<JoinedAccountRecord, AccountRepoError> {
        self.db_pool.with_tx_custom_error(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision.clone()).await?;

                let account_record: AccountRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE accounts
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3, email = $4
                            WHERE account_id = $5
                            RETURNING account_id, email, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(&revision.email)
                            .bind(revision.account_id)
                    ).await
                    .to_custom_result_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?;

                Ok(JoinedAccountRecord {
                    account_id: account_record.account_id,
                    current_revision_id: account_record.current_revision_id,
                    email: account_record.email,
                    name: revision_record.name,
                    plan_id: revision_record.plan_id,
                    audit: account_record.audit,
                    roles: revision_record.roles
                })
            }.boxed()
        }).await
    }

    async fn get_by_id(
        &self,
        account_id: &Uuid,
    ) -> Result<Option<JoinedAccountRecord>, AccountRepoError> {
        let mut result: Option<JoinedAccountRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT a.account_id, a.current_revision_id, a.email, a.created_at, a.updated_at, a.deleted_at, a.modified_by, ar.name, ar.plan_id
                    FROM accounts a
                    JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                    WHERE a.account_id = $1 AND a.deleted_at IS NULL
                "#})
                    .bind(account_id),
            )
            .await?;

        if let Some(result) = &mut result {
            result.roles = self
                .get_roles(&result.account_id, result.current_revision_id)
                .await?;
        };

        Ok(result)
    }

    async fn get_by_email(
        &self,
        email: &str,
    ) -> Result<Option<JoinedAccountRecord>, AccountRepoError> {
        let mut result: Option<JoinedAccountRecord> = self.with_ro("get_by_email")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT a.account_id, a.current_revision_id, a.email, a.created_at, a.updated_at, a.deleted_at, a.modified_by, ar.name, ar.plan_id
                    FROM accounts a
                    JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                    WHERE a.email = $1 AND a.deleted_at IS NULL
                "#})
                    .bind(email),
            )
            .await?;

        if let Some(result) = &mut result {
            result.roles = self
                .get_roles(&result.account_id, result.current_revision_id)
                .await?;
        };

        Ok(result)
    }
}
