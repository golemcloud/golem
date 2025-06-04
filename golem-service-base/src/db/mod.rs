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
use futures::{future, TryFutureExt};
use sqlx::query::{Query, QueryAs};
use sqlx::{Database, Error, FromRow, IntoArguments, Row};
use std::fmt::Debug;

pub mod postgres;
pub mod sqlite;

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
pub trait Pool: Debug {
    type LabelledApi: LabelledPoolApi;
    type LabelledTransaction;
    type QueryResult;
    type Db: Database + Sync;
    type Args<'a>;

    /// Gets a pooled database interface for READ ONLY operations
    fn with_ro(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi;

    /// Gets a pooled database interface for READ/WRITE operations
    fn with_rw(&self, svc_name: &'static str, api_name: &'static str) -> Self::LabelledApi;
}

#[async_trait]
pub trait PoolApi {
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

    async fn fetch_all<'a, O, A>(
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

    async fn commit(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError>;
    async fn rollback(&self, tx: Self::LabelledTransaction) -> Result<(), RepoError>;
}

#[async_trait]
pub trait LabelledPoolTransaction: PoolApi {}
