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

use super::model::account::{
    AccountBySecretRecord, AccountExtRevisionRecord, AccountRevisionRecord,
};
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::account::AccountRepoError;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn create(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError>;

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError>;

    async fn delete(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError>;

    async fn get_by_id(
        &self,
        account_id: Uuid,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError>;

    async fn get_by_email(
        &self,
        email: &str,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError>;

    async fn get_by_secret(
        &self,
        secret: &str,
    ) -> Result<Option<AccountBySecretRecord>, AccountRepoError>;
}

pub struct LoggedAccountRepo<Repo: AccountRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "account repository";

impl<Repo: AccountRepo> LoggedAccountRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_account_id(account_id: Uuid) -> Span {
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
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let span = Self::span_account_id(revision.account_id);
        self.repo.create(revision).instrument(span).await
    }

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let span = Self::span_account_id(revision.account_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let span = Self::span_account_id(revision.account_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        account_id: Uuid,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError> {
        self.repo
            .get_by_id(account_id)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn get_by_email(
        &self,
        email: &str,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError> {
        let span = Self::span_email(email);
        self.repo.get_by_email(email).instrument(span).await
    }

    async fn get_by_secret(
        &self,
        secret: &str,
    ) -> Result<Option<AccountBySecretRecord>, AccountRepoError> {
        self.repo.get_by_secret(secret).await
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
    async fn insert_revision(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: AccountRevisionRecord,
    ) -> Result<AccountRevisionRecord, AccountRepoError> {
        let revision: AccountRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO account_revisions
                    (account_id, revision_id, name, email, plan_id, roles,
                        created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    RETURNING account_id, revision_id, name, email,
                        plan_id, roles, created_at, created_by, deleted
                "# })
                .bind(revision.account_id)
                .bind(revision.revision_id)
                .bind(revision.name)
                .bind(revision.email)
                .bind(revision.plan_id)
                .bind(revision.roles)
                .bind_deletable_revision_audit(revision.audit),
            )
            .await
            .to_error_on_unique_violation(AccountRepoError::ConcurrentModification)?;

        Ok(revision)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountRepo for DbAccountRepo<PostgresPool> {
    async fn create(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "create", |tx| {
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
                    .to_error_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?;

                let revision_record = Self::insert_revision(tx, revision).await?;

                Ok(AccountExtRevisionRecord {
                    entity_created_at: account_record.audit.created_at,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision.clone()).await?;

                let account_record: AccountRecord = tx
                    .fetch_optional_as(
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
                    .to_error_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?
                    .ok_or(AccountRepoError::ConcurrentModification)?;

                Ok(AccountExtRevisionRecord {
                    entity_created_at: account_record.audit.created_at,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn delete(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "delete", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision.clone()).await?;

                let account_record: AccountRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE accounts
                            SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3, email = $4
                            WHERE account_id = $5
                            RETURNING account_id, email, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(&revision.email)
                            .bind(revision.account_id)
                    ).await
                    .to_error_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?
                    .ok_or(AccountRepoError::ConcurrentModification)?;

                Ok(AccountExtRevisionRecord {
                    entity_created_at: account_record.audit.created_at,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn get_by_id(
        &self,
        account_id: Uuid,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError> {
        let result: Option<AccountExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT a.created_at AS entity_created_at, ar.account_id, ar.revision_id, ar.name, ar.email, ar.plan_id, ar.roles, ar.created_at, ar.created_by, ar.deleted
                    FROM accounts a
                    JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                    WHERE
                        a.account_id = $1
                        AND a.deleted_at IS NULL
                "#})
                    .bind(account_id)
            )
            .await?;

        Ok(result)
    }

    async fn get_by_email(
        &self,
        email: &str,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError> {
        let result: Option<AccountExtRevisionRecord> = self.with_ro("get_by_email")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT a.created_at AS entity_created_at, ar.account_id, ar.revision_id, ar.name, ar.email, ar.plan_id, ar.roles, ar.created_at, ar.created_by, ar.deleted
                    FROM accounts a
                    JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                    WHERE
                        a.email = $1
                        AND a.deleted_at IS NULL
                "#})
                    .bind(email)
            )
            .await?;

        Ok(result)
    }

    async fn get_by_secret(
        &self,
        secret: &str,
    ) -> Result<Option<AccountBySecretRecord>, AccountRepoError> {
        let result: Option<AccountBySecretRecord> = self.with_ro("get_account_by_secret")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        t.token_id,
                        t.expires_at as token_expires_at,
                        a.created_at AS entity_created_at,
                        ar.account_id, ar.revision_id, ar.name, ar.email,
                        ar.plan_id, ar.roles, ar.created_at, ar.created_by, ar.deleted
                    FROM accounts a
                    JOIN account_revisions ar ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
                    JOIN tokens t
                        ON t.account_id = a.account_id
                    WHERE
                        t.secret = $1
                        AND a.deleted_at IS NULL
                "#})
                .bind(secret),
            )
            .await?;

        Ok(result)
    }
}
