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

use super::model::oauth2_webflow_state::OAuth2WebFlowStateRecord;
use crate::model::login::OAuth2WebflowStateMetadata;
use crate::repo::model::datetime::SqlDateTime;
use crate::repo::model::new_repo_uuid;
use crate::repo::model::token::TokenRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::RepoResult;
use indoc::indoc;
use sqlx::Database;
use sqlx::types::Json;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait OAuth2WebflowStateRepo: Send + Sync {
    async fn create(
        &self,
        metadata: OAuth2WebflowStateMetadata,
    ) -> RepoResult<OAuth2WebFlowStateRecord>;

    async fn set_token_id(
        &self,
        state_id: Uuid,
        token_id: Uuid,
    ) -> RepoResult<OAuth2WebFlowStateRecord>;

    async fn get_by_id(&self, state_id: Uuid) -> RepoResult<Option<OAuth2WebFlowStateRecord>>;

    async fn delete_by_id(&self, state_id: Uuid) -> RepoResult<u64>;

    async fn delete_expired(&self, delete_before: SqlDateTime) -> RepoResult<u64>;
}

pub struct LoggedOAuth2WebflowStateRepo<Repo: OAuth2WebflowStateRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "oauth2_webflow_state repository";

impl<Repo: OAuth2WebflowStateRepo> LoggedOAuth2WebflowStateRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_id(state_id: Uuid) -> Span {
        info_span!(SPAN_NAME, state_id = %state_id)
    }

    fn span_id_and_token(state_id: Uuid, token_id: Uuid) -> Span {
        info_span!(SPAN_NAME, state_id = %state_id, token_id = %token_id)
    }
}

#[async_trait]
impl<Repo: OAuth2WebflowStateRepo> OAuth2WebflowStateRepo for LoggedOAuth2WebflowStateRepo<Repo> {
    async fn create(
        &self,
        metadata: OAuth2WebflowStateMetadata,
    ) -> RepoResult<OAuth2WebFlowStateRecord> {
        self.repo.create(metadata).await
    }

    async fn set_token_id(
        &self,
        state_id: Uuid,
        token_id: Uuid,
    ) -> RepoResult<OAuth2WebFlowStateRecord> {
        self.repo
            .set_token_id(state_id, token_id)
            .instrument(Self::span_id_and_token(state_id, token_id))
            .await
    }

    async fn get_by_id(&self, state_id: Uuid) -> RepoResult<Option<OAuth2WebFlowStateRecord>> {
        self.repo
            .get_by_id(state_id)
            .instrument(Self::span_id(state_id))
            .await
    }

    async fn delete_by_id(&self, state_id: Uuid) -> RepoResult<u64> {
        self.repo
            .delete_by_id(state_id)
            .instrument(Self::span_id(state_id))
            .await
    }

    async fn delete_expired(&self, delete_before: SqlDateTime) -> RepoResult<u64> {
        self.repo.delete_expired(delete_before).await
    }
}

pub struct DbOAuth2WebflowStateRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "oauth2_webflow_state";

impl<DBP: Pool> DbOAuth2WebflowStateRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedOAuth2WebflowStateRepo<Self>
    where
        Self: OAuth2WebflowStateRepo,
    {
        LoggedOAuth2WebflowStateRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl OAuth2WebflowStateRepo for DbOAuth2WebflowStateRepo<PostgresPool> {
    async fn create(
        &self,
        metadata: OAuth2WebflowStateMetadata,
    ) -> RepoResult<OAuth2WebFlowStateRecord> {
        self.with_rw("create")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO oauth2_web_flow_states (state_id, metadata, token_id, created_at)
                    VALUES ($1, $2, NULL, $3)
                    RETURNING state_id, metadata, token_id, created_at
                "#})
                .bind(new_repo_uuid())
                .bind(Json::from(metadata))
                .bind(SqlDateTime::now()),
            )
            .await
    }

    async fn set_token_id(
        &self,
        state_id: Uuid,
        token_id: Uuid,
    ) -> RepoResult<OAuth2WebFlowStateRecord> {
        let state: OAuth2WebFlowStateRecord = self
            .with_rw("set_token_id")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    UPDATE oauth2_web_flow_states
                    SET token_id = $1
                    WHERE state_id = $2
                    RETURNING state_id, metadata, token_id, created_at
                "#})
                .bind(token_id)
                .bind(state_id),
            )
            .await?;

        self.with_token(state).await
    }

    async fn get_by_id(&self, state_id: Uuid) -> RepoResult<Option<OAuth2WebFlowStateRecord>> {
        let state: Option<OAuth2WebFlowStateRecord> = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT state_id, metadata, token_id, created_at
                    FROM oauth2_web_flow_states
                    WHERE state_id = $1
                "#})
                .bind(state_id),
            )
            .await?;

        match state {
            Some(state) => Ok(Some(self.with_token(state).await?)),
            None => Ok(None),
        }
    }

    async fn delete_by_id(&self, state_id: Uuid) -> RepoResult<u64> {
        let result = self
            .with_rw("delete_by_id")
            .execute(
                sqlx::query(indoc! { r#"
                    DELETE FROM oauth2_web_flow_states WHERE state_id = $1
                "#})
                .bind(state_id),
            )
            .await?;

        Ok(result.rows_affected())
    }

    async fn delete_expired(&self, delete_before: SqlDateTime) -> RepoResult<u64> {
        let result = self
            .with_rw("delete_expired")
            .execute(
                sqlx::query(indoc! { r#"
                    DELETE FROM oauth2_web_flow_states WHERE created_at < $1
                "#})
                .bind(delete_before),
            )
            .await?;

        Ok(result.rows_affected())
    }
}

#[async_trait]
trait OAuth2WebflowStateRepoInternal: OAuth2WebflowStateRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn get_token_by_id(&self, token_id: Uuid) -> RepoResult<Option<TokenRecord>>;

    async fn with_token(
        &self,
        mut state: OAuth2WebFlowStateRecord,
    ) -> RepoResult<OAuth2WebFlowStateRecord> {
        if let Some(token_id) = state.token_id {
            state.token = self.get_token_by_id(token_id).await?
            // TODO: fail on None?
        }
        Ok(state)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl OAuth2WebflowStateRepoInternal for DbOAuth2WebflowStateRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn get_token_by_id(&self, token_id: Uuid) -> RepoResult<Option<TokenRecord>> {
        self.with_ro("get_token_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT token_id, secret, account_id, created_at, expires_at
                    FROM tokens
                    WHERE token_id = $1
                "#})
                .bind(token_id),
            )
            .await
    }
}
