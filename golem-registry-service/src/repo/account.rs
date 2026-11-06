// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
    AccountBySecretRecord, AccountDeletionHoldRecord, AccountExtRevisionRecord,
    AccountRevisionRecord,
};
use crate::repo::account_usage::DbAccountUsageRepo;
use crate::repo::card::DbCardRepo;
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::card::CardRecord;
use crate::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RequiresNotificationSignal, RequiresSignalExt,
};
use crate::repo::{
    account_resource_override::{DbAccountResourceOverrideRepo, OverrideReconciliationScope},
    model::account::AccountRepoError,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::account_usage::MonthlyUsageModeTransitionSource;
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
        account_root_card: CardRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError>;

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError>;

    async fn update_plan_and_reconcile_overrides(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError>;

    async fn delete(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AccountExtRevisionRecord>, AccountRepoError>;

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

    /// Places the deletion hold of the holder, or replaces the reason of its existing hold.
    /// Returns `None` when the account does not exist or is deleted.
    async fn place_deletion_hold(
        &self,
        hold: AccountDeletionHoldRecord,
    ) -> Result<Option<AccountDeletionHoldRecord>, AccountRepoError>;

    /// Removes the deletion hold of the holder, if the account has one.
    async fn remove_deletion_hold(
        &self,
        account_id: Uuid,
        holder_account_id: Uuid,
    ) -> Result<(), AccountRepoError>;

    async fn list_deletion_holds(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<AccountDeletionHoldRecord>, AccountRepoError>;
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
        account_root_card: CardRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let span = Self::span_account_id(revision.account_id);
        self.repo
            .create(revision, account_root_card)
            .instrument(span)
            .await
    }

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let span = Self::span_account_id(revision.account_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn update_plan_and_reconcile_overrides(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let span = Self::span_account_id(revision.account_id);
        self.repo
            .update_plan_and_reconcile_overrides(revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AccountExtRevisionRecord>, AccountRepoError> {
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

    async fn place_deletion_hold(
        &self,
        hold: AccountDeletionHoldRecord,
    ) -> Result<Option<AccountDeletionHoldRecord>, AccountRepoError> {
        let span = Self::span_account_id(hold.account_id);
        self.repo.place_deletion_hold(hold).instrument(span).await
    }

    async fn remove_deletion_hold(
        &self,
        account_id: Uuid,
        holder_account_id: Uuid,
    ) -> Result<(), AccountRepoError> {
        self.repo
            .remove_deletion_hold(account_id, holder_account_id)
            .instrument(Self::span_account_id(account_id))
            .await
    }

    async fn list_deletion_holds(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<AccountDeletionHoldRecord>, AccountRepoError> {
        self.repo
            .list_deletion_holds(account_id)
            .instrument(Self::span_account_id(account_id))
            .await
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

impl DbAccountRepo<PostgresPool> {
    /// Locks the live account until the transaction ends, so that a concurrent delete waits for
    /// the transaction and then sees its deletion hold. Returns false when the account does not
    /// exist or is deleted.
    async fn lock_live_account_for_hold_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        account_id: Uuid,
    ) -> Result<bool, AccountRepoError> {
        let account: Option<(Uuid,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id
                    FROM accounts
                    WHERE account_id = $1
                      AND deleted_at IS NULL
                    FOR SHARE
                "# })
                .bind(account_id),
            )
            .await?;
        Ok(account.is_some())
    }
}

impl DbAccountRepo<SqlitePool> {
    /// SQLite has no row locks and allows one write transaction at a time. Returns false when
    /// the account does not exist or is deleted.
    async fn lock_live_account_for_hold_in_tx(
        tx: &mut <<SqlitePool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        account_id: Uuid,
    ) -> Result<bool, AccountRepoError> {
        let account: Option<(Uuid,)> = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT account_id
                    FROM accounts
                    WHERE account_id = $1
                      AND deleted_at IS NULL
                "# })
                .bind(account_id),
            )
            .await?;
        Ok(account.is_some())
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

    async fn update_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        let revision_record = Self::insert_revision(tx, revision.clone()).await?;

        let account_record: AccountRecord = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    UPDATE accounts
                    SET updated_at = $1, modified_by = $2, current_revision_id = $3, email = $4
                    WHERE account_id = $5
                    RETURNING account_id, email, account_root_card_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                "#})
                .bind(&revision.audit.created_at)
                .bind(revision.audit.created_by)
                .bind(revision.revision_id)
                .bind(&revision.email)
                .bind(revision.account_id),
            )
            .await
            .to_error_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?
            .ok_or(AccountRepoError::ConcurrentModification)?;

        Ok(AccountExtRevisionRecord {
            entity_created_at: account_record.audit.created_at,
            account_root_card_id: account_record.account_root_card_id,
            revision: revision_record,
        })
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl AccountRepo for DbAccountRepo<PostgresPool> {
    async fn create(
        &self,
        revision: AccountRevisionRecord,
        account_root_card: CardRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "create", |tx| {
            async move {
                let account_root_card = DbCardRepo::<PostgresPool>::create_in_tx(tx, account_root_card).await?;

                let account_record: AccountRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! {r#"
                            INSERT INTO accounts (account_id, email, account_root_card_id, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                            VALUES ($1, $2, $3, $4, $4, NULL, $5, $6)
                            RETURNING account_id, email, account_root_card_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(revision.account_id)
                            .bind(&revision.email)
                            .bind(account_root_card.card_id)
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                    )
                    .await
                    .to_error_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?;

                let revision_record = Self::insert_revision(tx, revision).await?;

                Ok(AccountExtRevisionRecord {
                    entity_created_at: account_record.audit.created_at,
                    account_root_card_id: account_record.account_root_card_id,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn update(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "update", |tx| {
                async move { Self::update_in_tx(tx, revision).await }.boxed()
            })
            .await
    }

    async fn update_plan_and_reconcile_overrides(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<AccountExtRevisionRecord, AccountRepoError> {
        self.db_pool
            .with_tx_err(
                METRICS_SVC_NAME,
                "update_plan_and_reconcile_overrides",
                |tx| {
                    async move {
                        DbAccountResourceOverrideRepo::<PostgresPool>::lock_account_in_tx(
                            tx,
                            revision.account_id,
                        )
                        .await?
                        .ok_or(AccountRepoError::ConcurrentModification)?;
                        let override_policies =
                            DbAccountResourceOverrideRepo::<PostgresPool>::lock_plan_policies_in_tx(
                                tx,
                                revision.plan_id,
                            )
                            .await?
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Account {} references missing destination plan {}",
                                    revision.account_id,
                                    revision.plan_id
                                )
                            })?;
                        let (overage_eligible,): (bool,) = tx
                            .fetch_one_as(
                                sqlx::query_as(
                                    "SELECT overage_eligible FROM plans WHERE plan_id = $1",
                                )
                                .bind(revision.plan_id),
                            )
                            .await?;
                        let account = Self::update_in_tx(tx, revision.clone()).await?;
                        DbAccountResourceOverrideRepo::<PostgresPool>::reconcile_in_tx(
                            tx,
                            OverrideReconciliationScope::Account(revision.account_id),
                            override_policies,
                        )
                        .await?;
                        if !overage_eligible {
                            DbAccountUsageRepo::<PostgresPool>::force_hard_limit_in_tx(
                                tx,
                                revision.account_id,
                                revision.audit.created_by,
                                MonthlyUsageModeTransitionSource::IneligiblePlanAssigned,
                            )
                            .await?;
                        }

                        Ok(account)
                    }
                    .boxed()
                },
            )
            .await
    }

    async fn delete(
        &self,
        revision: AccountRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AccountExtRevisionRecord>, AccountRepoError> {
        let result = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "delete", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision.clone()).await?;

                let account_record: AccountRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE accounts
                            SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3, email = $4
                            WHERE account_id = $5
                            RETURNING account_id, email, account_root_card_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(&revision.email)
                            .bind(revision.account_id)
                    ).await
                    .to_error_on_unique_violation(AccountRepoError::AccountViolatesUniqueness)?
                    .ok_or(AccountRepoError::ConcurrentModification)?;

                // The update above locks the account row. A transaction that places a hold
                // locks the row too, so its hold is visible here, or it waits for this
                // transaction and then finds the account deleted.
                let holds: Vec<AccountDeletionHoldRecord> = tx
                    .fetch_all_as(
                        sqlx::query_as(indoc! {r#"
                            SELECT account_id, holder_account_id, reason, created_at, updated_at
                            FROM account_deletion_holds
                            WHERE account_id = $1
                            ORDER BY created_at, holder_account_id
                        "#})
                            .bind(revision.account_id)
                    ).await?;
                if !holds.is_empty() {
                    return Err(AccountRepoError::DeletionHeld(holds));
                }

                let change_event =
                    NewRegistryChangeEvent::account_tokens_invalidated(revision.account_id);
                DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                    .await?;

                if let Some(account_root_card_id) = account_record.account_root_card_id {
                    DbCardRepo::<PostgresPool>::delete_tree_in_tx(tx, golem_common::model::card::CardId(account_root_card_id)).await?;
                }

                Ok::<_, AccountRepoError>(AccountExtRevisionRecord {
                    entity_created_at: account_record.audit.created_at,
                    account_root_card_id: account_record.account_root_card_id,
                    revision: revision_record,
                })
            }.boxed()
        })
        .await?;

        Ok(result.requires_notification_signal())
    }

    async fn get_by_id(
        &self,
        account_id: Uuid,
    ) -> Result<Option<AccountExtRevisionRecord>, AccountRepoError> {
        let result: Option<AccountExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT a.created_at AS entity_created_at, a.account_root_card_id, ar.account_id, ar.revision_id, ar.name, ar.email, ar.plan_id, ar.roles, ar.created_at, ar.created_by, ar.deleted
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
                    SELECT a.created_at AS entity_created_at, a.account_root_card_id, ar.account_id, ar.revision_id, ar.name, ar.email, ar.plan_id, ar.roles, ar.created_at, ar.created_by, ar.deleted
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
                        t.impersonated_by,
                        a.created_at AS entity_created_at,
                        a.account_root_card_id,
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

    async fn place_deletion_hold(
        &self,
        hold: AccountDeletionHoldRecord,
    ) -> Result<Option<AccountDeletionHoldRecord>, AccountRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "place_deletion_hold", |tx| {
                async move {
                    if !Self::lock_live_account_for_hold_in_tx(tx, hold.account_id).await? {
                        return Ok(None);
                    }
                    let record: AccountDeletionHoldRecord = tx
                        .fetch_one_as(
                            sqlx::query_as(indoc! {r#"
                                INSERT INTO account_deletion_holds
                                    (account_id, holder_account_id, reason, created_at, updated_at)
                                VALUES ($1, $2, $3, $4, $5)
                                ON CONFLICT (account_id, holder_account_id)
                                DO UPDATE SET reason = $3, updated_at = $5
                                RETURNING account_id, holder_account_id, reason, created_at, updated_at
                            "#})
                            .bind(hold.account_id)
                            .bind(hold.holder_account_id)
                            .bind(hold.reason)
                            .bind(hold.created_at)
                            .bind(hold.updated_at),
                        )
                        .await?;
                    Ok::<_, AccountRepoError>(Some(record))
                }
                .boxed()
            })
            .await
    }

    async fn remove_deletion_hold(
        &self,
        account_id: Uuid,
        holder_account_id: Uuid,
    ) -> Result<(), AccountRepoError> {
        self.db_pool
            .with_rw(METRICS_SVC_NAME, "remove_deletion_hold")
            .execute(
                sqlx::query(indoc! {r#"
                    DELETE FROM account_deletion_holds
                    WHERE account_id = $1 AND holder_account_id = $2
                "#})
                .bind(account_id)
                .bind(holder_account_id),
            )
            .await?;
        Ok(())
    }

    async fn list_deletion_holds(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<AccountDeletionHoldRecord>, AccountRepoError> {
        let result = self
            .with_ro("list_deletion_holds")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT account_id, holder_account_id, reason, created_at, updated_at
                    FROM account_deletion_holds
                    WHERE account_id = $1
                    ORDER BY created_at, holder_account_id
                "#})
                .bind(account_id),
            )
            .await?;
        Ok(result)
    }
}
