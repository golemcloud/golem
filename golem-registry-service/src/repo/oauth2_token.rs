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

use crate::repo::model::oauth2_token::OAuth2TokenRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoResult;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait OAuth2TokenRepo: Send + Sync {
    async fn create_or_update(&self, token: OAuth2TokenRecord) -> RepoResult<OAuth2TokenRecord>;

    async fn get_by_external_provider(
        &self,
        provider: &str,
        external_id: &str,
    ) -> RepoResult<Option<OAuth2TokenRecord>>;

    async fn unset_token_id_by_external_provider(
        &self,
        provider: &str,
        external_id: &str,
    ) -> RepoResult<Option<OAuth2TokenRecord>>;

    async fn get_by_token_id(&self, token_id: Uuid) -> RepoResult<Option<OAuth2TokenRecord>>;
}

pub struct LoggedOAuth2TokenRepo<Repo: OAuth2TokenRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "oauth2_token repository";

impl<Repo: OAuth2TokenRepo> LoggedOAuth2TokenRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_oath2_token(token: &OAuth2TokenRecord) -> Span {
        info_span!(SPAN_NAME, provider = token.provider, external_id = token.external_id, token_id = ?token.token_id)
    }

    fn span_provider(provider: &str, external_id: &str) -> Span {
        info_span!(SPAN_NAME, provider, external_id)
    }

    fn span_token_id(token_id: Uuid) -> Span {
        info_span!(SPAN_NAME, token_id=%token_id)
    }
}

#[async_trait]
impl<Repo: OAuth2TokenRepo> OAuth2TokenRepo for LoggedOAuth2TokenRepo<Repo> {
    async fn create_or_update(&self, token: OAuth2TokenRecord) -> RepoResult<OAuth2TokenRecord> {
        let span = Self::span_oath2_token(&token);
        self.repo.create_or_update(token).instrument(span).await
    }

    async fn get_by_external_provider(
        &self,
        provider: &str,
        external_id: &str,
    ) -> RepoResult<Option<OAuth2TokenRecord>> {
        self.repo
            .get_by_external_provider(provider, external_id)
            .instrument(Self::span_provider(provider, external_id))
            .await
    }

    async fn unset_token_id_by_external_provider(
        &self,
        provider: &str,
        external_id: &str,
    ) -> RepoResult<Option<OAuth2TokenRecord>> {
        self.repo
            .unset_token_id_by_external_provider(provider, external_id)
            .instrument(Self::span_provider(provider, external_id))
            .await
    }

    async fn get_by_token_id(&self, token_id: Uuid) -> RepoResult<Option<OAuth2TokenRecord>> {
        self.repo
            .get_by_token_id(token_id)
            .instrument(Self::span_token_id(token_id))
            .await
    }
}

pub struct DbOAuth2TokenRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "oauth2_token";

impl<DBP: Pool> DbOAuth2TokenRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedOAuth2TokenRepo<Self>
    where
        Self: OAuth2TokenRepo,
    {
        LoggedOAuth2TokenRepo::new(Self::new(db_pool))
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
impl OAuth2TokenRepo for DbOAuth2TokenRepo<PostgresPool> {
    async fn create_or_update(&self, token: OAuth2TokenRecord) -> RepoResult<OAuth2TokenRecord> {
        self.with_rw("create_or_update")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO oauth2_tokens (provider, external_id, token_id, account_id)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (provider, external_id) DO UPDATE SET token_id = $3, account_id = $4
                    RETURNING provider, external_id, token_id, account_id
                "#})
                .bind(token.provider)
                .bind(token.external_id)
                .bind(token.token_id)
                .bind(token.account_id),
            )
            .await
    }

    async fn get_by_external_provider(
        &self,
        provider: &str,
        external_id: &str,
    ) -> RepoResult<Option<OAuth2TokenRecord>> {
        self.with_ro("get_by_external_provider")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                SELECT provider, external_id, token_id, account_id
                FROM oauth2_tokens
                WHERE provider = $1 AND external_id = $2
            "#})
                .bind(provider)
                .bind(external_id),
            )
            .await
    }

    async fn unset_token_id_by_external_provider(
        &self,
        provider: &str,
        external_id: &str,
    ) -> RepoResult<Option<OAuth2TokenRecord>> {
        self.with_rw("unset_token_id_by_external_provider")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    UPDATE oauth2_tokens
                    SET token_id = NULL
                    WHERE provider = $1 AND external_id = $2
                    RETURNING provider, external_id, token_id, account_id
                "#})
                .bind(provider)
                .bind(external_id),
            )
            .await
    }

    async fn get_by_token_id(&self, token_id: Uuid) -> RepoResult<Option<OAuth2TokenRecord>> {
        self.with_ro("get_by_token_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT provider, external_id, token_id, account_id
                    FROM oauth2_tokens
                    WHERE token_id = $1
                "#})
                .bind(token_id),
            )
            .await
    }
}
