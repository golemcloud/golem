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

use crate::repo::RepoError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::{TryFutureExt, future};
use sqlx::query::{Query, QueryAs};
use sqlx::{Database, Error, FromRow, IntoArguments, Row};
use std::fmt::Debug;
use tracing::{error, warn};

pub mod postgres;
pub mod sqlite;

// TODO: better name, should not be in common?
#[derive(sqlx::FromRow, Debug)]
pub struct DBValue {
    value: Vec<u8>,
}

impl DBValue {
    pub fn into_bytes(self) -> Bytes {
        Bytes::from(self.value)
    }
}

#[async_trait]
pub trait Pool: Debug + Sync + Clone {
    type LabelledApi: LabelledPoolApi;
    type QueryResult;
    type Db: Database + Sync;
    type Args<'a>;

    /// Gets a pooled database interface for READ ONLY operations
    fn with_ro(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi;

    /// Gets a pooled database interface for READ/WRITE operations
    fn with_rw(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi;

    /// With_tx gets a pooled database interface for READ/WRITE operations, starts a transaction,
    /// then executes f with the transaction as a parameter, which has to return a Result.
    /// If f returns a RepoError or business error, then the transaction will be rolled back,
    /// otherwise it will be committed. This means that f should not explicitly call commit or
    /// rollback, this is ensured by only sharing a mut ref (given rollback and commit consumes the transaction).
    ///
    /// One reason to prefer using this function compared to direct usage of transactions is that
    /// this style enforces calling labeled rollback on any error. In direct style, rollback is usually
    /// only called on the sqlx::Transaction drop, unless explicitly handled by the code, which means
    /// that those rollbacks are not visible for metrics.
    async fn with_tx_err<R, E, F>(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        f: F,
    ) -> Result<R, E>
    where
        R: Send,
        E: Debug + Send + From<RepoError>,
        F: for<'f> FnOnce(
                &'f mut <Self::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, E>>
            + Send,
    {
        let mut tx = self.with_rw(svc_name, api_name).begin().await?;
        match f(&mut tx).await {
            Ok(result) => {
                tx.commit().await?;
                Ok(result)
            }
            Err(err) => {
                warn!(
                    svc_name, api_name, error = ?err,
                    "Rolling back, transaction failed with repo error",
                );

                // If rollback fails, we still return the original error, but log the rollback error
                if let Err(rollback_error) = tx.rollback().await {
                    error!(svc_name, api_name, rollback_error = %rollback_error, "Rollback failed");
                }

                Err(err)
            }
        }
    }

    /// A simplified version of with_tx_err for cases when there is no need for rollbacks based on
    /// business logic and errors. See with_tx for more info.
    async fn with_tx<R, F>(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        f: F,
    ) -> Result<R, RepoError>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <Self::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, RepoError>>
            + Send,
    {
        let mut tx = self.with_rw(svc_name, api_name).begin().await?;
        match f(&mut tx).await {
            Ok(result) => {
                tx.commit().await?;
                Ok(result)
            }
            Err(err) => {
                warn!(
                    svc_name, api_name, error = %err,
                    "Rolling back, transaction failed with repo error",
                );

                // If rollback fails, we still return the original error, but log the rollback error
                if let Err(err) = tx.rollback().await {
                    error!(svc_name, api_name, error = %err, "Rollback failed");
                }

                Err(err)
            }
        }
    }
}

#[async_trait]
pub trait PoolApi: Send {
    type QueryResult;
    type Row: Row;
    type Db: Database;
    type Args<'a>;

    async fn execute<'a>(
        &mut self,
        query: Query<'a, Self::Db, Self::Args<'a>>,
    ) -> Result<Self::QueryResult, RepoError>;

    async fn fetch_optional<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Option<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>;

    async fn fetch_one<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Self::Row, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
    {
        self.fetch_optional(query)
            .and_then(|row| match row {
                Some(row) => future::ok(row),
                None => future::err(Error::RowNotFound.into()),
            })
            .await
    }

    async fn fetch_optional_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Option<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>;

    async fn fetch_one_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<O, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>,
    {
        self.fetch_optional_as(query_as)
            .and_then(|row| match row {
                Some(row) => future::ok(row),
                None => future::err(Error::RowNotFound.into()),
            })
            .await
    }

    async fn fetch_all<'a, A>(
        &mut self,
        query: Query<'a, Self::Db, A>,
    ) -> Result<Vec<Self::Row>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>;

    async fn fetch_all_as<'a, O, A>(
        &mut self,
        query_as: QueryAs<'a, Self::Db, O, A>,
    ) -> Result<Vec<O>, RepoError>
    where
        A: 'a + IntoArguments<'a, Self::Db>,
        O: 'a + Send + Unpin + for<'r> FromRow<'r, Self::Row>;
}

#[async_trait]
pub trait LabelledPoolApi: PoolApi {
    type LabelledTransaction: LabelledPoolTransaction;

    async fn begin(&self) -> Result<Self::LabelledTransaction, RepoError>;
}

#[async_trait]
pub trait LabelledPoolTransaction: PoolApi {
    async fn commit(self) -> Result<(), RepoError>;
    async fn rollback(self) -> Result<(), RepoError>;
}
