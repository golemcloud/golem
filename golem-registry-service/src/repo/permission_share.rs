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

use crate::repo::card::DbCardRepo;
use crate::repo::model::BindFields;
use crate::repo::model::card::CardRecord;
use crate::repo::model::permission_share::{
    PermissionShareAuthExtRevisionRecord, PermissionShareExtRevisionRecord, PermissionShareRecord,
    PermissionShareRepoError, PermissionShareRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::card::CardId;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait PermissionShareRepo: Send + Sync {
    async fn create(
        &self,
        owner_account_id: Uuid,
        target_account_id: Uuid,
        revision: PermissionShareRevisionRecord,
        card: CardRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError>;

    async fn update(
        &self,
        revision: PermissionShareRevisionRecord,
        replacement_card: CardRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError>;

    async fn delete(
        &self,
        revision: PermissionShareRevisionRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError>;

    async fn get_by_id(
        &self,
        permission_share_id: Uuid,
    ) -> Result<Option<PermissionShareAuthExtRevisionRecord>, PermissionShareRepoError>;

    async fn get_by_owner_and_name(
        &self,
        owner_account_id: Uuid,
        name: &str,
    ) -> Result<Option<PermissionShareExtRevisionRecord>, PermissionShareRepoError>;

    async fn get_for_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<PermissionShareExtRevisionRecord>, PermissionShareRepoError>;

    async fn get_for_target(
        &self,
        target_account_id: Uuid,
    ) -> Result<Vec<PermissionShareExtRevisionRecord>, PermissionShareRepoError>;

    async fn active_cards_for_target(
        &self,
        target_account_id: Uuid,
    ) -> Result<Vec<CardRecord>, PermissionShareRepoError>;
}

pub struct LoggedPermissionShareRepo<Repo: PermissionShareRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "permission share repository";

impl<Repo: PermissionShareRepo> LoggedPermissionShareRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_permission_share_id(permission_share_id: Uuid) -> Span {
        info_span!(SPAN_NAME, permission_share_id = %permission_share_id)
    }

    fn span_account_id(account_id: Uuid) -> Span {
        info_span!(SPAN_NAME, account_id = %account_id)
    }
}

#[async_trait]
impl<Repo: PermissionShareRepo> PermissionShareRepo for LoggedPermissionShareRepo<Repo> {
    async fn create(
        &self,
        owner_account_id: Uuid,
        target_account_id: Uuid,
        revision: PermissionShareRevisionRecord,
        card: CardRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError> {
        self.repo
            .create(owner_account_id, target_account_id, revision, card)
            .instrument(Self::span_account_id(owner_account_id))
            .await
    }

    async fn update(
        &self,
        revision: PermissionShareRevisionRecord,
        replacement_card: CardRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError> {
        let span = Self::span_permission_share_id(revision.permission_share_id);
        self.repo
            .update(revision, replacement_card)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        revision: PermissionShareRevisionRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError> {
        let span = Self::span_permission_share_id(revision.permission_share_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        permission_share_id: Uuid,
    ) -> Result<Option<PermissionShareAuthExtRevisionRecord>, PermissionShareRepoError> {
        self.repo
            .get_by_id(permission_share_id)
            .instrument(Self::span_permission_share_id(permission_share_id))
            .await
    }

    async fn get_by_owner_and_name(
        &self,
        owner_account_id: Uuid,
        name: &str,
    ) -> Result<Option<PermissionShareExtRevisionRecord>, PermissionShareRepoError> {
        self.repo
            .get_by_owner_and_name(owner_account_id, name)
            .instrument(Self::span_account_id(owner_account_id))
            .await
    }

    async fn get_for_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<PermissionShareExtRevisionRecord>, PermissionShareRepoError> {
        self.repo
            .get_for_owner(owner_account_id)
            .instrument(Self::span_account_id(owner_account_id))
            .await
    }

    async fn get_for_target(
        &self,
        target_account_id: Uuid,
    ) -> Result<Vec<PermissionShareExtRevisionRecord>, PermissionShareRepoError> {
        self.repo
            .get_for_target(target_account_id)
            .instrument(Self::span_account_id(target_account_id))
            .await
    }

    async fn active_cards_for_target(
        &self,
        target_account_id: Uuid,
    ) -> Result<Vec<CardRecord>, PermissionShareRepoError> {
        self.repo
            .active_cards_for_target(target_account_id)
            .instrument(Self::span_account_id(target_account_id))
            .await
    }
}

pub struct DbPermissionShareRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "permission-share";

impl<DBP: Pool> DbPermissionShareRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedPermissionShareRepo<Self>
    where
        Self: PermissionShareRepo,
    {
        LoggedPermissionShareRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbPermissionShareRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: PermissionShareRevisionRecord,
    ) -> Result<PermissionShareRevisionRecord, PermissionShareRepoError> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO permission_share_revisions
                    (permission_share_id, revision_id, name, card_id, data, created_at, created_by, deleted)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING permission_share_id, revision_id, name, card_id, data, created_at, created_by, deleted
            "#})
            .bind(revision.permission_share_id)
            .bind(revision.revision_id)
            .bind(revision.name)
            .bind(revision.card_id)
            .bind(revision.data)
            .bind_deletable_revision_audit(revision.audit),
        )
        .await
        .to_error_on_unique_violation(PermissionShareRepoError::ConcurrentModification)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PermissionShareRepo for DbPermissionShareRepo<PostgresPool> {
    async fn create(
        &self,
        owner_account_id: Uuid,
        target_account_id: Uuid,
        revision: PermissionShareRevisionRecord,
        card: CardRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError> {
        let result = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                async move {
                    let card = DbCardRepo::<PostgresPool>::create_in_tx(tx, card).await?;

                    let share: PermissionShareRecord = tx
                        .fetch_one_as(
                            sqlx::query_as(indoc! { r#"
                                INSERT INTO permission_shares
                                    (permission_share_id, owner_account_id, target_account_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                                VALUES ($1, $2, $3, $4, $5, $5, NULL, $6, $7)
                                RETURNING permission_share_id, owner_account_id, target_account_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                            "#})
                            .bind(revision.permission_share_id)
                            .bind(owner_account_id)
                            .bind(target_account_id)
                            .bind(&revision.name)
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id),
                        )
                        .await
                        .to_error_on_unique_violation(
                            PermissionShareRepoError::ShareViolatesUniqueness,
                        )?;

                    let mut revision = revision;
                    revision.card_id = Some(card.card_id);
                    let revision = Self::insert_revision(tx, revision).await?;

                    Ok::<_, PermissionShareRepoError>(PermissionShareExtRevisionRecord {
                        owner_account_id: share.owner_account_id,
                        target_account_id: share.target_account_id,
                        revision,
                    })
                }
                .boxed()
            })
            .await?;

        Ok(result)
    }

    async fn update(
        &self,
        revision: PermissionShareRevisionRecord,
        replacement_card: CardRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError> {
        let result = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "update", |tx| {
                async move {
                    let old_card_id = revision.card_id;
                    let replacement_card =
                        DbCardRepo::<PostgresPool>::create_in_tx(tx, replacement_card).await?;
                    let mut revision = revision;
                    revision.card_id = Some(replacement_card.card_id);
                    let revision = Self::insert_revision(tx, revision).await?;

                    let share: PermissionShareRecord = tx
                        .fetch_optional_as(
                            sqlx::query_as(indoc! { r#"
                                UPDATE permission_shares
                                SET name = $1, updated_at = $2, modified_by = $3, current_revision_id = $4
                                WHERE permission_share_id = $5
                                RETURNING permission_share_id, owner_account_id, target_account_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                            "#})
                            .bind(&revision.name)
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.permission_share_id),
                        )
                        .await
                        .to_error_on_unique_violation(
                            PermissionShareRepoError::ShareViolatesUniqueness,
                        )?
                        .ok_or(PermissionShareRepoError::ConcurrentModification)?;

                    if let Some(old_card_id) =
                        old_card_id.filter(|card_id| *card_id != replacement_card.card_id)
                    {
                        DbCardRepo::<PostgresPool>::delete_tree_in_tx(tx, CardId(old_card_id))
                            .await?;
                    }

                    Ok::<_, PermissionShareRepoError>(PermissionShareExtRevisionRecord {
                        owner_account_id: share.owner_account_id,
                        target_account_id: share.target_account_id,
                        revision,
                    })
                }
                .boxed()
            })
            .await?;

        Ok(result)
    }

    async fn delete(
        &self,
        revision: PermissionShareRevisionRecord,
    ) -> Result<PermissionShareExtRevisionRecord, PermissionShareRepoError> {
        let result = self
            .db_pool
                .with_tx_err(METRICS_SVC_NAME, "delete", |tx| {
                async move {
                    let mut revision = Self::insert_revision(tx, revision).await?;
                    let old_card_id = revision.card_id;

                    let share: PermissionShareRecord = tx
                        .fetch_optional_as(
                            sqlx::query_as(indoc! { r#"
                                UPDATE permission_shares
                                SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3
                                WHERE permission_share_id = $4
                                RETURNING permission_share_id, owner_account_id, target_account_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                            "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.permission_share_id),
                        )
                        .await?
                        .ok_or(PermissionShareRepoError::ConcurrentModification)?;

                    if let Some(old_card_id) = old_card_id {
                        DbCardRepo::<PostgresPool>::delete_tree_in_tx(tx, CardId(old_card_id))
                            .await?;
                        revision.card_id = None;
                    }

                    Ok::<_, PermissionShareRepoError>(PermissionShareExtRevisionRecord {
                        owner_account_id: share.owner_account_id,
                        target_account_id: share.target_account_id,
                        revision,
                    })
                }
                .boxed()
            })
            .await?;

        Ok(result)
    }

    async fn get_by_id(
        &self,
        permission_share_id: Uuid,
    ) -> Result<Option<PermissionShareAuthExtRevisionRecord>, PermissionShareRepoError> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT ps.owner_account_id, ps.target_account_id,
                           psr.permission_share_id, psr.revision_id, psr.name, psr.card_id, psr.data, psr.created_at, psr.created_by, psr.deleted,
                           owner.email AS owner_account_email,
                           target.email AS target_account_email
                    FROM permission_shares ps
                    JOIN accounts owner
                        ON owner.account_id = ps.owner_account_id
                       AND owner.deleted_at IS NULL
                    JOIN accounts target
                        ON target.account_id = ps.target_account_id
                       AND target.deleted_at IS NULL
                    JOIN permission_share_revisions psr
                        ON psr.permission_share_id = ps.permission_share_id
                       AND psr.revision_id = ps.current_revision_id
                    WHERE ps.permission_share_id = $1
                      AND ps.deleted_at IS NULL
                "#})
                .bind(permission_share_id),
            )
            .await
            .map_err(Into::into)
    }

    async fn get_by_owner_and_name(
        &self,
        owner_account_id: Uuid,
        name: &str,
    ) -> Result<Option<PermissionShareExtRevisionRecord>, PermissionShareRepoError> {
        self.with_ro("get_by_owner_and_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT ps.owner_account_id, ps.target_account_id,
                           psr.permission_share_id, psr.revision_id, psr.name, psr.card_id, psr.data, psr.created_at, psr.created_by, psr.deleted
                    FROM permission_shares ps
                    JOIN permission_share_revisions psr
                        ON psr.permission_share_id = ps.permission_share_id
                       AND psr.revision_id = ps.current_revision_id
                    WHERE ps.owner_account_id = $1
                      AND ps.name = $2
                      AND ps.deleted_at IS NULL
                "#})
                .bind(owner_account_id)
                .bind(name),
            )
            .await
            .map_err(Into::into)
    }

    async fn get_for_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<PermissionShareExtRevisionRecord>, PermissionShareRepoError> {
        self.with_ro("get_for_owner")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT ps.owner_account_id, ps.target_account_id,
                           psr.permission_share_id, psr.revision_id, psr.name, psr.card_id, psr.data, psr.created_at, psr.created_by, psr.deleted
                    FROM permission_shares ps
                    JOIN permission_share_revisions psr
                        ON psr.permission_share_id = ps.permission_share_id
                       AND psr.revision_id = ps.current_revision_id
                    WHERE ps.owner_account_id = $1
                      AND ps.deleted_at IS NULL
                    ORDER BY ps.name
                "#})
                .bind(owner_account_id),
            )
            .await
            .map_err(Into::into)
    }

    async fn get_for_target(
        &self,
        target_account_id: Uuid,
    ) -> Result<Vec<PermissionShareExtRevisionRecord>, PermissionShareRepoError> {
        self.with_ro("get_for_target")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT ps.owner_account_id, ps.target_account_id,
                           psr.permission_share_id, psr.revision_id, psr.name, psr.card_id, psr.data, psr.created_at, psr.created_by, psr.deleted
                    FROM permission_shares ps
                    JOIN permission_share_revisions psr
                        ON psr.permission_share_id = ps.permission_share_id
                       AND psr.revision_id = ps.current_revision_id
                    WHERE ps.target_account_id = $1
                      AND ps.deleted_at IS NULL
                    ORDER BY ps.name
                "#})
                .bind(target_account_id),
            )
            .await
            .map_err(Into::into)
    }

    async fn active_cards_for_target(
        &self,
        target_account_id: Uuid,
    ) -> Result<Vec<CardRecord>, PermissionShareRepoError> {
        self.with_ro("active_cards_for_target")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.card_id, c.data, c.created_at, c.expires_at, c.system_card, c.managed_by
                    FROM permission_shares ps
                    JOIN permission_share_revisions psr
                        ON psr.permission_share_id = ps.permission_share_id
                       AND psr.revision_id = ps.current_revision_id
                    JOIN cards c ON c.card_id = psr.card_id
                    WHERE ps.target_account_id = $1
                      AND ps.deleted_at IS NULL
                    ORDER BY ps.permission_share_id
                "#})
                .bind(target_account_id),
            )
            .await
            .map_err(Into::into)
    }
}
