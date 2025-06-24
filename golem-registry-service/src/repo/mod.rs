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

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::Database;

pub mod account;
pub mod application;
pub mod environment;
pub mod plan;
pub mod token;

/// SqlDateTime is a wrapper around DateTime<Utc> which allows storing it as NaiveDateTime
/// (e.g. TIMESTAMP WITHOUT TIME ZONE for Postgres). Doing so allows us to use the same queries
/// for Sqlite and Postgres without any custom casting for the specific database, while still
/// using UTC times in our Repo Record types.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SqlDateTime {
    utc: DateTime<Utc>,
}

impl SqlDateTime {
    pub fn new(utc: DateTime<Utc>) -> Self {
        Self { utc }
    }

    pub fn now() -> Self {
        Utc::now().into()
    }

    pub fn as_utc(&self) -> &DateTime<Utc> {
        &self.utc
    }

    pub fn into_utc(self) -> DateTime<Utc> {
        self.utc
    }
}

impl From<DateTime<Utc>> for SqlDateTime {
    fn from(utc: DateTime<Utc>) -> Self {
        Self::new(utc)
    }
}

impl From<SqlDateTime> for DateTime<Utc> {
    fn from(sql_dt: SqlDateTime) -> Self {
        sql_dt.utc
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <NaiveDateTime as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <NaiveDateTime as sqlx::Type<DB>>::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        <NaiveDateTime as sqlx::Encode<'q, DB>>::encode_by_ref(&self.utc.naive_utc(), buf)
    }

    fn size_hint(&self) -> usize {
        <NaiveDateTime as sqlx::Encode<'q, DB>>::size_hint(&self.utc.naive_utc())
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self {
            utc: <NaiveDateTime as sqlx::Decode<'r, DB>>::decode(value)?.and_utc(),
        })
    }
}
