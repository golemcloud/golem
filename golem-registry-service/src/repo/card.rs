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

use crate::repo::model::card::{CardRecord, CardRepoError};
use crate::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RequiresNotificationSignal, RequiresSignalExt,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::card::CardId;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{PoolLabelledTransaction, RepoError, RepoResult, ResultExt};
use indoc::indoc;
use sqlx::Row;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait CardRepo: Send + Sync {
    async fn create(&self, card: CardRecord) -> Result<CardRecord, CardRepoError>;

    async fn insert_token_root_card(
        &self,
        account_id: Uuid,
        expected_epoch: i64,
        card: CardRecord,
    ) -> Result<Option<CardRecord>, CardRepoError>;

    // Delete a card including all descendants. Returns ids of all deleted cards.
    async fn delete(
        &self,
        card_id: CardId,
    ) -> Result<RequiresNotificationSignal<Vec<CardId>>, CardRepoError>;

    async fn existing(&self, card_ids: Vec<CardId>) -> RepoResult<Vec<CardId>>;
}

pub struct LoggedCardRepo<Repo: CardRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "card repository";

impl<Repo: CardRepo> LoggedCardRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_card_id(card_id: CardId) -> Span {
        info_span!(SPAN_NAME, card_id = %card_id)
    }
}

#[async_trait]
impl<Repo: CardRepo> CardRepo for LoggedCardRepo<Repo> {
    async fn create(&self, card: CardRecord) -> Result<CardRecord, CardRepoError> {
        let span = Self::span_card_id(CardId(card.card_id));

        self.repo.create(card).instrument(span).await
    }

    async fn insert_token_root_card(
        &self,
        account_id: Uuid,
        expected_epoch: i64,
        card: CardRecord,
    ) -> Result<Option<CardRecord>, CardRepoError> {
        let span = Self::span_card_id(CardId(card.card_id));

        self.repo
            .insert_token_root_card(account_id, expected_epoch, card)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        card_id: CardId,
    ) -> Result<RequiresNotificationSignal<Vec<CardId>>, CardRepoError> {
        self.repo
            .delete(card_id)
            .instrument(Self::span_card_id(card_id))
            .await
    }

    async fn existing(&self, card_ids: Vec<CardId>) -> RepoResult<Vec<CardId>> {
        self.repo
            .existing(card_ids)
            .instrument(info_span!(SPAN_NAME))
            .await
    }
}

pub struct DbCardRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "card";

impl<DBP: Pool> DbCardRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedCardRepo<Self>
    where
        Self: CardRepo,
    {
        LoggedCardRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbCardRepo<PostgresPool> {
    async fn insert_parent_links(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        card_id: Uuid,
        parent_ids: &[Uuid],
    ) -> Result<(), CardRepoError> {
        for parent_id in parent_ids {
            tx.execute(
                sqlx::query("INSERT INTO card_parents (card_id, parent_id) VALUES ($1, $2)")
                    .bind(card_id)
                    .bind(parent_id),
            )
            .await
            .to_error_on_foreign_key_violation(CardRepoError::ParentNotFound(*parent_id))?;
        }

        Ok(())
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbCardRepo<PostgresPool> {
    pub async fn delete_tree_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        card_id: CardId,
    ) -> Result<Vec<CardId>, CardRepoError> {
        let rows = tx
            .fetch_all(
                sqlx::query(indoc! { r#"
                    WITH RECURSIVE to_delete(card_id) AS (
                        SELECT $1
                        UNION
                        SELECT cp.card_id
                        FROM card_parents cp
                        JOIN to_delete td ON cp.parent_id = td.card_id
                    )
                    DELETE FROM cards
                    WHERE card_id IN (SELECT card_id FROM to_delete)
                    RETURNING card_id
                "#})
                .bind(card_id.0),
            )
            .await
            .to_error_on_foreign_key_violation(CardRepoError::CardTreeChangedDuringDelete)?;

        let mut deleted = Vec::with_capacity(rows.len());
        for row in rows {
            let deleted_card_id = row.try_get::<Uuid, _>("card_id").map_err(RepoError::from)?;
            deleted.push(CardId(deleted_card_id));
        }

        if !deleted.is_empty() {
            DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                tx,
                &NewRegistryChangeEvent::cards_revoked(deleted.iter().map(|id| id.0).collect()),
            )
            .await?;
        }

        Ok(deleted)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbCardRepo<PostgresPool> {
    async fn create_in_tx(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        record: CardRecord,
    ) -> Result<CardRecord, CardRepoError> {
        Self::lock_parent_cards_for_create(tx, record.data.value().parent_ids.as_slice()).await?;

        let inserted: CardRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO cards
                        (card_id, data, created_at, expires_at, system_card, managed_by)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING card_id, data, created_at, expires_at, system_card, managed_by
                "#})
                .bind(record.card_id)
                .bind(record.data)
                .bind(record.created_at)
                .bind(record.expires_at)
                .bind(record.system_card)
                .bind(record.managed_by),
            )
            .await?;

        Self::insert_parent_links(
            tx,
            inserted.card_id,
            inserted.data.value().parent_ids.as_slice(),
        )
        .await?;

        Ok(inserted)
    }
}

impl DbCardRepo<PostgresPool> {
    async fn lock_parent_cards_for_create(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        parent_ids: &[Uuid],
    ) -> Result<(), CardRepoError> {
        for parent_id in parent_ids {
            let row = tx
                .fetch_optional(
                    sqlx::query("SELECT card_id FROM cards WHERE card_id = $1 FOR UPDATE")
                        .bind(*parent_id),
                )
                .await?;

            if row.is_none() {
                return Err(CardRepoError::ParentNotFound(*parent_id));
            }
        }

        Ok(())
    }
}

impl DbCardRepo<SqlitePool> {
    async fn lock_parent_cards_for_create(
        _tx: &mut PoolLabelledTransaction<SqlitePool>,
        _parent_ids: &[Uuid],
    ) -> RepoResult<()> {
        // SQLite serializes write transactions, so there is no separate row-locking
        // primitive to use here. Missing parents are rejected by the card_parents FK.
        Ok(())
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl CardRepo for DbCardRepo<PostgresPool> {
    async fn create(&self, record: CardRecord) -> Result<CardRecord, CardRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                Box::pin(async move { Self::create_in_tx(tx, record).await })
            })
            .await
    }

    async fn insert_token_root_card(
        &self,
        account_id: Uuid,
        expected_epoch: i64,
        record: CardRecord,
    ) -> Result<Option<CardRecord>, CardRepoError> {
        let result = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "insert_token_root_card", |tx| {
                Box::pin(async move {
                    let inserted = Self::create_in_tx(tx, record).await?;

                    let rows_affected = tx
                        .execute(
                            sqlx::query(indoc! { r#"
                                UPDATE accounts
                                SET token_root_card_id = $1
                                WHERE account_id = $2
                                  AND token_root_card_epoch = $3
                                  AND token_root_card_id IS NULL
                                  AND deleted_at IS NULL
                            "#})
                            .bind(inserted.card_id)
                            .bind(account_id)
                            .bind(expected_epoch),
                        )
                        .await?
                        .rows_affected();

                    if rows_affected == 1 {
                        Ok::<_, CardRepoError>(inserted)
                    } else {
                        Err(CardRepoError::ConcurrentModification)
                    }
                })
            })
            .await;

        match result {
            Ok(card) => Ok(Some(card)),
            Err(CardRepoError::ConcurrentModification) => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn delete(
        &self,
        card_id: CardId,
    ) -> Result<RequiresNotificationSignal<Vec<CardId>>, CardRepoError> {
        let deleted = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "hard_delete", |tx| {
                Box::pin(async move { Self::delete_tree_in_tx(tx, card_id).await })
            })
            .await?;

        Ok(deleted.requires_notification_signal())
    }

    async fn existing(&self, card_ids: Vec<CardId>) -> RepoResult<Vec<CardId>> {
        let mut result = Vec::new();
        let mut api = self.with_ro("existing");
        for card_id in card_ids {
            if let Some(row) = api
                .fetch_optional(
                    sqlx::query("SELECT card_id FROM cards WHERE card_id = $1").bind(card_id.0),
                )
                .await?
            {
                result.push(CardId(row.try_get("card_id").map_err(RepoError::from)?));
            }
        }
        Ok(result)
    }
}
