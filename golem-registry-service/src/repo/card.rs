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

use crate::repo::model::card::CardDataRecord;
use crate::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RequiresNotificationSignal, RequiresSignalExt,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::model::card::Card;
use golem_common::serialization;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{PoolLabelledTransaction, RepoError, RepoResult, SqlDateTime};
use indoc::indoc;
use sqlx::Row;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait CardRepo: Send + Sync {
    async fn create(&self, card: Card) -> RepoResult<()>;

    async fn hard_delete(&self, card_id: Uuid)
    -> RepoResult<RequiresNotificationSignal<Vec<Uuid>>>;

    async fn existing(&self, card_ids: Vec<Uuid>) -> RepoResult<Vec<Uuid>>;
}

pub struct LoggedCardRepo<Repo: CardRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "card repository";

impl<Repo: CardRepo> LoggedCardRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_card_id(card_id: Uuid) -> Span {
        info_span!(SPAN_NAME, card_id = %card_id)
    }
}

#[async_trait]
impl<Repo: CardRepo> CardRepo for LoggedCardRepo<Repo> {
    async fn create(&self, card: Card) -> RepoResult<()> {
        self.repo
            .create(card.clone())
            .instrument(Self::span_card_id(card.card_id))
            .await
    }

    async fn hard_delete(
        &self,
        card_id: Uuid,
    ) -> RepoResult<RequiresNotificationSignal<Vec<Uuid>>> {
        self.repo
            .hard_delete(card_id)
            .instrument(Self::span_card_id(card_id))
            .await
    }

    async fn existing(&self, card_ids: Vec<Uuid>) -> RepoResult<Vec<Uuid>> {
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
    async fn insert_parents(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        card_id: Uuid,
        parent_ids: &[Uuid],
    ) -> RepoResult<()> {
        for parent_id in parent_ids {
            tx.execute(
                sqlx::query("INSERT INTO card_parents (card_id, parent_id) VALUES ($1, $2)")
                    .bind(card_id)
                    .bind(*parent_id),
            )
            .await?;
        }

        Ok(())
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl CardRepo for DbCardRepo<PostgresPool> {
    async fn create(&self, card: Card) -> RepoResult<()> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                Box::pin(async move {
                    let data = serialize_card_data(&card)?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO cards
                                (card_id, data, created_at, expires_at, system_card, polymorphic)
                            VALUES ($1, $2, $3, $4, $5, $6)
                        "#})
                        .bind(card.card_id)
                        .bind(data)
                        .bind(SqlDateTime::from(card.created_at))
                        .bind(card.expires_at.map(SqlDateTime::from))
                        .bind(card.system_card)
                        .bind(card.polymorphic),
                    )
                    .await?;

                    Self::insert_parents(tx, card.card_id, &card.parent_ids).await
                })
            })
            .await
    }

    async fn hard_delete(
        &self,
        card_id: Uuid,
    ) -> RepoResult<RequiresNotificationSignal<Vec<Uuid>>> {
        let deleted = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "hard_delete", |tx| {
                Box::pin(async move {
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
                            .bind(card_id),
                        )
                        .await?;

                    let mut deleted = Vec::with_capacity(rows.len());
                    for row in rows {
                        let deleted_card_id =
                            row.try_get::<Uuid, _>("card_id").map_err(RepoError::from)?;
                        deleted.push(deleted_card_id);
                    }

                    if !deleted.is_empty() {
                        DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                            tx,
                            &NewRegistryChangeEvent::cards_revoked(deleted.clone()),
                        )
                        .await?;
                    }

                    Ok::<_, RepoError>(deleted)
                })
            })
            .await?;

        Ok(deleted.requires_notification_signal())
    }

    async fn existing(&self, card_ids: Vec<Uuid>) -> RepoResult<Vec<Uuid>> {
        let mut result = Vec::new();
        let mut api = self.with_ro("existing");
        for card_id in card_ids {
            if let Some(row) = api
                .fetch_optional(
                    sqlx::query("SELECT card_id FROM cards WHERE card_id = $1").bind(card_id),
                )
                .await?
            {
                result.push(row.try_get("card_id").map_err(RepoError::from)?);
            }
        }
        Ok(result)
    }
}

fn serialize<T: desert_rust::BinarySerializer>(value: &T) -> RepoResult<Vec<u8>> {
    serialization::serialize(value).map_err(to_repo_string)
}

fn serialize_card_data(card: &Card) -> RepoResult<Vec<u8>> {
    serialize(&CardDataRecord {
        parent_ids: card.parent_ids.clone(),
        lower_positive: card.lower_positive.clone(),
        lower_negative: card.lower_negative.clone(),
        upper_positive: card.upper_positive.clone(),
        upper_negative: card.upper_negative.clone(),
    })
}

fn to_repo_string(error: String) -> RepoError {
    RepoError::InternalError(anyhow::anyhow!(error))
}
