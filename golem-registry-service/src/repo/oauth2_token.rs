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

use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::{RepoError, ResultExt};
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait OAuth2TokenRepo: Send + Sync {
    // Repository methods will be added here
    // Placeholder for future methods
}

pub struct LoggedOAuth2TokenRepo<Repo: OAuth2TokenRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "oauth2_token repository";

impl<Repo: OAuth2TokenRepo> LoggedOAuth2TokenRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span() -> Span {
        info_span!(SPAN_NAME)
    }
}

#[async_trait]
impl<Repo: OAuth2TokenRepo> OAuth2TokenRepo for LoggedOAuth2TokenRepo<Repo> {
    // Implementation of OAuth2TokenRepo methods will be added here
    // Each method will call the underlying repo method with tracing instrumentation
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

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> repo::Result<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, repo::Result<R>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl OAuth2TokenRepo for DbOAuth2TokenRepo<PostgresPool> {

}
