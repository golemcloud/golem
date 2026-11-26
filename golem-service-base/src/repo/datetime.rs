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

use jiff::{SignedDuration, Timestamp};
use sqlx::Database;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use std::fmt::Display;
use std::ops::{Deref, Sub};
use time::PrimitiveDateTime;

/// SqlDateTime wraps a UTC Timestamp and stores its civil UTC DateTime
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
    utc: Timestamp,
}

impl SqlDateTime {
    pub fn new(utc: Timestamp) -> Self {
        Self { utc }
    }

    pub fn now() -> Self {
        Timestamp::now().into()
    }

    pub fn as_utc(&self) -> &Timestamp {
        &self.utc
    }

    pub fn into_utc(self) -> Timestamp {
        self.utc
    }

    fn sql_datetime(&self) -> Result<PrimitiveDateTime, BoxDynError> {
        let utc = time::OffsetDateTime::from_unix_timestamp_nanos(self.utc.as_nanosecond())?;
        Ok(PrimitiveDateTime::new(utc.date(), utc.time()))
    }

    fn from_sql_datetime(value: PrimitiveDateTime) -> Result<Self, BoxDynError> {
        Ok(Self {
            utc: Timestamp::from_nanosecond(value.assume_utc().unix_timestamp_nanos())?,
        })
    }
}

impl PartialEq<SqlDateTime> for SqlDateTime {
    fn eq(&self, other: &SqlDateTime) -> bool {
        self.utc.sub(other.utc).abs() < SignedDuration::from_millis(1)
    }
}

impl Display for SqlDateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.utc, f)
    }
}

impl Deref for SqlDateTime {
    type Target = Timestamp;

    fn deref(&self) -> &Self::Target {
        self.as_utc()
    }
}

impl From<Timestamp> for SqlDateTime {
    fn from(utc: Timestamp) -> Self {
        Self::new(utc)
    }
}

impl From<SqlDateTime> for Timestamp {
    fn from(sql_dt: SqlDateTime) -> Self {
        sql_dt.utc
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlDateTime
where
    PrimitiveDateTime: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        PrimitiveDateTime::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        PrimitiveDateTime::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlDateTime
where
    PrimitiveDateTime: sqlx::Encode<'q, DB>,
{
    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer<'q>) -> Result<IsNull, BoxDynError>
    where
        Self: Sized,
    {
        self.sql_datetime()?.encode(buf)
    }

    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        self.sql_datetime()?.encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        self.sql_datetime()
            .map(|value| <PrimitiveDateTime as sqlx::Encode<'q, DB>>::size_hint(&value))
            .unwrap_or(0)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for SqlDateTime {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, BoxDynError> {
        super::postgres_temporal::validate_timestamp(value.clone())?;
        Self::from_sql_datetime(<PrimitiveDateTime as sqlx::Decode<sqlx::Postgres>>::decode(
            value,
        )?)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for SqlDateTime {
    fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Self::from_sql_datetime(<PrimitiveDateTime as sqlx::Decode<sqlx::Sqlite>>::decode(
            value,
        )?)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::MySql> for SqlDateTime {
    fn decode(value: sqlx::mysql::MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        Self::from_sql_datetime(<PrimitiveDateTime as sqlx::Decode<sqlx::MySql>>::decode(
            value,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    async fn sqlite_datetime_roundtrip_preserves_utc() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        for value in [
            "2026-01-02T03:04:05.123456789+05:30",
            "1969-12-31T23:59:59.123456789Z",
            "2000-02-29T00:00:00Z",
        ] {
            let timestamp: Timestamp = value.parse().unwrap();
            let stored = SqlDateTime::new(timestamp);
            let (actual,): (SqlDateTime,) = sqlx::query_as("SELECT ?")
                .bind(stored)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(actual.into_utc(), timestamp);
        }
        let (actual,): (SqlDateTime,) = sqlx::query_as("SELECT '2026-01-02 03:04:05'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            actual.into_utc(),
            "2026-01-02T03:04:05Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn equality_retains_submillisecond_tolerance() {
        let timestamp = Timestamp::UNIX_EPOCH;
        assert_eq!(
            SqlDateTime::new(timestamp),
            SqlDateTime::new(timestamp + SignedDuration::from_micros(999))
        );
        assert_ne!(
            SqlDateTime::new(timestamp),
            SqlDateTime::new(timestamp + SignedDuration::from_millis(1))
        );
    }
}
