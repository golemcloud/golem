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

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use sqlx::Database;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use std::fmt::Display;
use std::ops::{Deref, Sub};

/// SqlDateTime is a wrapper around DateTime<Utc> which allows storing it as NaiveDateTime
/// (e.g., TIMESTAMP WITHOUT TIME ZONE for Postgres). Doing so allows us to use the same queries
/// for Sqlite and Postgres without any custom casting for the specific database, while still
/// using UTC times in our Repo Record types.
///
/// Another feature provided by SqlDateTime is that it defines PartialEq with some small difference
/// allowed. This is useful for writing repo tests in cases where the resolutions
/// of timestamps are different between the OS (used by Rust) and the DB timestamp type.
///
/// Note that above means that SqlDateTime MUST NOT implement Eq.

#[derive(Debug, Clone, PartialOrd)]
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

impl PartialEq<SqlDateTime> for SqlDateTime {
    fn eq(&self, other: &SqlDateTime) -> bool {
        self.utc.sub(other.utc).abs() < TimeDelta::milliseconds(1)
    }
}

impl Display for SqlDateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.utc, f)
    }
}

impl Deref for SqlDateTime {
    type Target = DateTime<Utc>;

    fn deref(&self) -> &Self::Target {
        self.as_utc()
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
        NaiveDateTime::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        NaiveDateTime::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Encode<'q, DB>,
{
    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer<'q>) -> Result<IsNull, BoxDynError>
    where
        Self: Sized,
    {
        self.utc.naive_utc().encode(buf)
    }

    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        self.utc.naive_utc().encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        self.utc.naive_utc().size_hint()
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self {
            utc: NaiveDateTime::decode(value)?.and_utc(),
        })
    }
}
