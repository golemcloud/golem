// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::rdbms::mysql::{
    DbColumn, DbColumnType, DbRow, DbValue, Error, Host, HostDbConnection, HostDbResultSet,
};
use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::RdbmsPoolKey;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use sqlx::types::BitVec;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {}

pub struct MysqlDbConnection {
    pub pool_key: RdbmsPoolKey,
}

impl MysqlDbConnection {
    pub fn new(pool_key: RdbmsPoolKey) -> Self {
        Self { pool_key }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<MysqlDbConnection>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-connection", "open");

        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let result = self
            .state
            .rdbms_service
            .mysql()
            .create(&address, &worker_id)
            .await;

        match result {
            Ok(key) => {
                let entry = MysqlDbConnection::new(key);
                let resource = self.as_wasi_view().table().push(entry)?;
                Ok(Ok(resource))
            }
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn query(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSetEntry>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-connection", "query");
        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&self_)?
            .pool_key
            .clone();
        match params
            .into_iter()
            .map(|v| v.try_into())
            .collect::<Result<Vec<_>, String>>()
        {
            Ok(params) => {
                let result = self
                    .state
                    .rdbms_service
                    .mysql()
                    .query(&pool_key, &worker_id, &statement, params)
                    .await;

                match result {
                    Ok(result) => {
                        let entry = DbResultSetEntry::new(result);
                        let db_result_set = self.as_wasi_view().table().push(entry)?;
                        Ok(Ok(db_result_set))
                    }
                    Err(e) => Ok(Err(e.into())),
                }
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    async fn execute(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-connection", "execute");
        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&self_)?
            .pool_key
            .clone();

        match params
            .into_iter()
            .map(|v| v.try_into())
            .collect::<Result<Vec<_>, String>>()
        {
            Ok(params) => {
                let result = self
                    .state
                    .rdbms_service
                    .mysql()
                    .execute(&pool_key, &worker_id, &statement, params)
                    .await
                    .map_err(|e| e.into());

                Ok(result)
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    fn drop(&mut self, rep: Resource<MysqlDbConnection>) -> anyhow::Result<()> {
        record_host_function_call("rdbms::mysql::db-connection", "drop");

        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&rep)?
            .pool_key
            .clone();

        let _ = self
            .state
            .rdbms_service
            .mysql()
            .remove(&pool_key, &worker_id);

        self.as_wasi_view()
            .table()
            .delete::<MysqlDbConnection>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for &mut DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<MysqlDbConnection>, Error>> {
        (*self).open(address).await
    }

    async fn query(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSetEntry>, Error>> {
        (*self).query(self_, statement, params).await
    }

    async fn execute(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        (*self).execute(self_, statement, params).await
    }

    fn drop(&mut self, rep: Resource<MysqlDbConnection>) -> anyhow::Result<()> {
        // (*self).drop(rep)
        HostDbConnection::drop(*self, rep)
    }
}

pub struct DbResultSetEntry {
    pub internal: Arc<dyn crate::services::rdbms::DbResultSet<MysqlType> + Send + Sync>,
}

impl crate::durable_host::rdbms::mysql::DbResultSetEntry {
    pub fn new(
        internal: Arc<dyn crate::services::rdbms::DbResultSet<MysqlType> + Send + Sync>,
    ) -> Self {
        Self { internal }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::mysql::DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-result-set", "get-columns");

        let internal = self
            .as_wasi_view()
            .table()
            .get::<crate::durable_host::rdbms::mysql::DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let columns = internal.deref().get_columns().await.map_err(Error::from)?;

        let columns = columns.into_iter().map(|c| c.into()).collect();
        Ok(columns)
    }

    async fn get_next(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::mysql::DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::mysql::db-result-set", "get-next");
        let internal = self
            .as_wasi_view()
            .table()
            .get::<crate::durable_host::rdbms::mysql::DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let rows = internal.deref().get_next().await.map_err(Error::from)?;

        let rows = rows.map(|r| r.into_iter().map(|r| r.into()).collect());
        Ok(rows)
    }

    fn drop(
        &mut self,
        rep: Resource<crate::durable_host::rdbms::mysql::DbResultSetEntry>,
    ) -> anyhow::Result<()> {
        record_host_function_call("rdbms::mysql::db-result-set", "drop");
        self.as_wasi_view()
            .table()
            .delete::<crate::durable_host::rdbms::mysql::DbResultSetEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for &mut DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::mysql::DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        (*self).get_columns(self_).await
    }

    async fn get_next(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::mysql::DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        (*self).get_next(self_).await
    }

    fn drop(
        &mut self,
        rep: Resource<crate::durable_host::rdbms::mysql::DbResultSetEntry>,
    ) -> anyhow::Result<()> {
        // (*self).drop(rep)
        HostDbResultSet::drop(*self, rep)
    }
}

impl TryFrom<DbValue> for crate::services::rdbms::mysql::types::DbValue {
    type Error = String;
    fn try_from(value: DbValue) -> Result<Self, Self::Error> {
        match value {
            DbValue::Boolean(v) => Ok(Self::Boolean(v)),
            DbValue::Tinyint(v) => Ok(Self::Tinyint(v)),
            DbValue::Smallint(v) => Ok(Self::Smallint(v)),
            DbValue::Mediumint(v) => Ok(Self::Mediumint(v)),
            DbValue::Int(v) => Ok(Self::Int(v)),
            DbValue::Bigint(v) => Ok(Self::Bigint(v)),
            DbValue::TinyintUnsigned(v) => Ok(Self::TinyintUnsigned(v)),
            DbValue::SmallintUnsigned(v) => Ok(Self::SmallintUnsigned(v)),
            DbValue::MediumintUnsigned(v) => Ok(Self::MediumintUnsigned(v)),
            DbValue::IntUnsigned(v) => Ok(Self::IntUnsigned(v)),
            DbValue::BigintUnsigned(v) => Ok(Self::BigintUnsigned(v)),
            DbValue::Decimal(s) => {
                let v = bigdecimal::BigDecimal::from_str(&s).map_err(|e| e.to_string())?;
                Ok(Self::Decimal(v))
            }
            DbValue::Float(v) => Ok(Self::Float(v)),
            DbValue::Double(v) => Ok(Self::Double(v)),
            DbValue::Text(v) => Ok(Self::Text(v)),
            DbValue::Varchar(v) => Ok(Self::Varchar(v)),
            DbValue::Fixchar(v) => Ok(Self::Fixchar(v)),
            DbValue::Blob(v) => Ok(Self::Blob(v)),
            DbValue::Tinyblob(v) => Ok(Self::Tinyblob(v)),
            DbValue::Mediumblob(v) => Ok(Self::Mediumblob(v)),
            DbValue::Longblob(v) => Ok(Self::Longblob(v)),
            DbValue::Binary(v) => Ok(Self::Binary(v)),
            DbValue::Varbinary(v) => Ok(Self::Varbinary(v)),
            DbValue::Tinytext(v) => Ok(Self::Tinytext(v)),
            DbValue::Mediumtext(v) => Ok(Self::Mediumtext(v)),
            DbValue::Longtext(v) => Ok(Self::Longtext(v)),
            DbValue::Json(v) => {
                let v: serde_json::Value = serde_json::from_str(&v).map_err(|e| e.to_string())?;
                Ok(Self::Json(v))
            }
            DbValue::Timestamp(v) => {
                let value = mysql_utils::timestamp_to_datetime(v)?;
                Ok(Self::Timestamp(value))
            }
            DbValue::Date(v) => {
                let value = mysql_utils::date_to_nativedate(v)?;
                Ok(Self::Date(value))
            }
            DbValue::Time(v) => {
                let value = mysql_utils::time_to_nativetime(v)?;
                Ok(Self::Time(value))
            }
            DbValue::Datetime(v) => {
                let value = mysql_utils::timestamp_to_datetime(v)?;
                Ok(Self::Datetime(value))
            }
            DbValue::Year(v) => Ok(Self::Year(v)),
            DbValue::Set(v) => Ok(Self::Set(v)),
            DbValue::Enumeration(v) => Ok(Self::Enumeration(v)),
            DbValue::Bit(v) => Ok(Self::Bit(BitVec::from_iter(v))),
            DbValue::Null => Ok(Self::Null),
        }
    }
}

impl From<crate::services::rdbms::mysql::types::DbValue> for DbValue {
    fn from(value: crate::services::rdbms::mysql::types::DbValue) -> Self {
        match value {
            crate::services::rdbms::mysql::types::DbValue::Boolean(v) => Self::Boolean(v),
            crate::services::rdbms::mysql::types::DbValue::Tinyint(v) => Self::Tinyint(v),
            crate::services::rdbms::mysql::types::DbValue::Smallint(v) => Self::Smallint(v),
            crate::services::rdbms::mysql::types::DbValue::Mediumint(v) => Self::Mediumint(v),
            crate::services::rdbms::mysql::types::DbValue::Int(v) => Self::Int(v),
            crate::services::rdbms::mysql::types::DbValue::Bigint(v) => Self::Bigint(v),
            crate::services::rdbms::mysql::types::DbValue::TinyintUnsigned(v) => {
                Self::TinyintUnsigned(v)
            }
            crate::services::rdbms::mysql::types::DbValue::SmallintUnsigned(v) => {
                Self::SmallintUnsigned(v)
            }
            crate::services::rdbms::mysql::types::DbValue::MediumintUnsigned(v) => {
                Self::MediumintUnsigned(v)
            }
            crate::services::rdbms::mysql::types::DbValue::IntUnsigned(v) => Self::IntUnsigned(v),
            crate::services::rdbms::mysql::types::DbValue::BigintUnsigned(v) => {
                Self::BigintUnsigned(v)
            }
            crate::services::rdbms::mysql::types::DbValue::Decimal(v) => {
                Self::Decimal(v.to_string())
            }
            crate::services::rdbms::mysql::types::DbValue::Float(v) => Self::Float(v),
            crate::services::rdbms::mysql::types::DbValue::Double(v) => Self::Double(v),
            crate::services::rdbms::mysql::types::DbValue::Text(v) => Self::Text(v),
            crate::services::rdbms::mysql::types::DbValue::Varchar(v) => Self::Varchar(v),
            crate::services::rdbms::mysql::types::DbValue::Fixchar(v) => Self::Fixchar(v),
            crate::services::rdbms::mysql::types::DbValue::Blob(v) => Self::Blob(v),
            crate::services::rdbms::mysql::types::DbValue::Tinyblob(v) => Self::Tinyblob(v),
            crate::services::rdbms::mysql::types::DbValue::Mediumblob(v) => Self::Mediumblob(v),
            crate::services::rdbms::mysql::types::DbValue::Longblob(v) => Self::Longblob(v),
            crate::services::rdbms::mysql::types::DbValue::Binary(v) => Self::Binary(v),
            crate::services::rdbms::mysql::types::DbValue::Varbinary(v) => Self::Varbinary(v),
            crate::services::rdbms::mysql::types::DbValue::Tinytext(v) => Self::Tinytext(v),
            crate::services::rdbms::mysql::types::DbValue::Mediumtext(v) => Self::Mediumtext(v),
            crate::services::rdbms::mysql::types::DbValue::Longtext(v) => Self::Longtext(v),
            crate::services::rdbms::mysql::types::DbValue::Json(v) => Self::Json(v.to_string()),
            crate::services::rdbms::mysql::types::DbValue::Timestamp(v) => {
                Self::Timestamp(mysql_utils::datetime_to_timestamp(v))
            }
            crate::services::rdbms::mysql::types::DbValue::Date(v) => {
                Self::Date(mysql_utils::naivedate_to_date(v))
            }
            crate::services::rdbms::mysql::types::DbValue::Time(v) => {
                Self::Time(mysql_utils::naivetime_to_time(v))
            }
            crate::services::rdbms::mysql::types::DbValue::Datetime(v) => {
                Self::Datetime(mysql_utils::datetime_to_timestamp(v))
            }
            crate::services::rdbms::mysql::types::DbValue::Year(v) => Self::Year(v),
            crate::services::rdbms::mysql::types::DbValue::Set(v) => Self::Set(v),
            crate::services::rdbms::mysql::types::DbValue::Enumeration(v) => Self::Enumeration(v),
            crate::services::rdbms::mysql::types::DbValue::Bit(v) => Self::Bit(v.iter().collect()),
            crate::services::rdbms::mysql::types::DbValue::Null => Self::Null,
        }
    }
}

impl From<crate::services::rdbms::DbRow<crate::services::rdbms::mysql::types::DbValue>> for DbRow {
    fn from(
        value: crate::services::rdbms::DbRow<crate::services::rdbms::mysql::types::DbValue>,
    ) -> Self {
        Self {
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl From<crate::services::rdbms::mysql::types::DbColumnType> for DbColumnType {
    fn from(value: crate::services::rdbms::mysql::types::DbColumnType) -> Self {
        match value {
            crate::services::rdbms::mysql::types::DbColumnType::Boolean => Self::Boolean,
            crate::services::rdbms::mysql::types::DbColumnType::Tinyint => Self::Tinyint,
            crate::services::rdbms::mysql::types::DbColumnType::Smallint => Self::Smallint,
            crate::services::rdbms::mysql::types::DbColumnType::Mediumint => Self::Mediumint,
            crate::services::rdbms::mysql::types::DbColumnType::Int => Self::Int,
            crate::services::rdbms::mysql::types::DbColumnType::Bigint => Self::Bigint,
            crate::services::rdbms::mysql::types::DbColumnType::IntUnsigned => Self::IntUnsigned,
            crate::services::rdbms::mysql::types::DbColumnType::TinyintUnsigned => {
                Self::TinyintUnsigned
            }
            crate::services::rdbms::mysql::types::DbColumnType::SmallintUnsigned => {
                Self::SmallintUnsigned
            }
            crate::services::rdbms::mysql::types::DbColumnType::MediumintUnsigned => {
                Self::MediumintUnsigned
            }
            crate::services::rdbms::mysql::types::DbColumnType::BigintUnsigned => {
                Self::BigintUnsigned
            }
            crate::services::rdbms::mysql::types::DbColumnType::Float => Self::Float,
            crate::services::rdbms::mysql::types::DbColumnType::Double => Self::Double,
            crate::services::rdbms::mysql::types::DbColumnType::Decimal => Self::Decimal,
            crate::services::rdbms::mysql::types::DbColumnType::Text => Self::Text,
            crate::services::rdbms::mysql::types::DbColumnType::Varchar => Self::Varchar,
            crate::services::rdbms::mysql::types::DbColumnType::Fixchar => Self::Fixchar,
            crate::services::rdbms::mysql::types::DbColumnType::Blob => Self::Blob,
            crate::services::rdbms::mysql::types::DbColumnType::Json => Self::Json,
            crate::services::rdbms::mysql::types::DbColumnType::Timestamp => Self::Timestamp,
            crate::services::rdbms::mysql::types::DbColumnType::Date => Self::Date,
            crate::services::rdbms::mysql::types::DbColumnType::Time => todo!(), //Self::Time,
            crate::services::rdbms::mysql::types::DbColumnType::Datetime => Self::Datetime,
            crate::services::rdbms::mysql::types::DbColumnType::Year => Self::Year,
            crate::services::rdbms::mysql::types::DbColumnType::Bit => Self::Bit,
            crate::services::rdbms::mysql::types::DbColumnType::Binary => Self::Binary,
            crate::services::rdbms::mysql::types::DbColumnType::Varbinary => Self::Varbinary,
            crate::services::rdbms::mysql::types::DbColumnType::Tinyblob => Self::Tinyblob,
            crate::services::rdbms::mysql::types::DbColumnType::Mediumblob => Self::Mediumblob,
            crate::services::rdbms::mysql::types::DbColumnType::Longblob => Self::Longblob,
            crate::services::rdbms::mysql::types::DbColumnType::Tinytext => Self::Tinytext,
            crate::services::rdbms::mysql::types::DbColumnType::Mediumtext => Self::Mediumtext,
            crate::services::rdbms::mysql::types::DbColumnType::Longtext => Self::Longtext,
            crate::services::rdbms::mysql::types::DbColumnType::Enumeration => Self::Enumeration,
            crate::services::rdbms::mysql::types::DbColumnType::Set => Self::Set,
        }
    }
}

impl From<crate::services::rdbms::mysql::types::DbColumn> for DbColumn {
    fn from(value: crate::services::rdbms::mysql::types::DbColumn) -> Self {
        Self {
            ordinal: value.ordinal,
            name: value.name,
            db_type: value.db_type.into(),
            db_type_name: value.db_type_name,
        }
    }
}

impl From<crate::services::rdbms::Error> for Error {
    fn from(value: crate::services::rdbms::Error) -> Self {
        match value {
            crate::services::rdbms::Error::ConnectionFailure(v) => Self::ConnectionFailure(v),
            crate::services::rdbms::Error::QueryParameterFailure(v) => {
                Self::QueryParameterFailure(v)
            }
            crate::services::rdbms::Error::QueryExecutionFailure(v) => {
                Self::QueryExecutionFailure(v)
            }
            crate::services::rdbms::Error::QueryResponseFailure(v) => Self::QueryResponseFailure(v),
            crate::services::rdbms::Error::Other(v) => Self::Other(v),
        }
    }
}

pub(crate) mod mysql_utils {
    use crate::preview2::wasi::rdbms::mysql::{Date, Time, Timestamp};
    use chrono::{Datelike, Timelike};

    pub(crate) fn time_to_nativetime(value: Time) -> Result<chrono::NaiveTime, String> {
        let time = chrono::NaiveTime::from_hms_nano_opt(
            value.hour as u32,
            value.minute as u32,
            value.second as u32,
            value.nanosecond,
        )
        .ok_or("Time value is not valid")?;
        Ok(time)
    }

    pub(crate) fn date_to_nativedate(value: Date) -> Result<chrono::NaiveDate, String> {
        let date = chrono::naive::NaiveDate::from_ymd_opt(
            value.year,
            value.month as u32,
            value.day as u32,
        )
        .ok_or("Date value is not valid")?;
        Ok(date)
    }

    pub(crate) fn timestamp_to_datetime(
        value: Timestamp,
    ) -> Result<chrono::DateTime<chrono::Utc>, String> {
        timestamp_to_naivedatetime(value).map(|v| v.and_utc())
    }

    pub(crate) fn timestamp_to_naivedatetime(
        value: Timestamp,
    ) -> Result<chrono::NaiveDateTime, String> {
        let date = date_to_nativedate(value.date)?;
        let time = time_to_nativetime(value.time)?;
        Ok(chrono::naive::NaiveDateTime::new(date, time))
    }

    pub(crate) fn naivetime_to_time(v: chrono::NaiveTime) -> Time {
        let hour = v.hour() as u8;
        let minute = v.minute() as u8;
        let second = v.second() as u8;
        let nanosecond = v.nanosecond();
        Time {
            hour,
            minute,
            second,
            nanosecond,
        }
    }

    pub(crate) fn naivedate_to_date(v: chrono::NaiveDate) -> Date {
        let year = v.year();
        let month = v.month() as u8;
        let day = v.day() as u8;
        Date { year, month, day }
    }

    pub(crate) fn datetime_to_timestamp(v: chrono::DateTime<chrono::Utc>) -> Timestamp {
        naivedatetime_to_timestamp(v.naive_utc())
    }

    pub(crate) fn naivedatetime_to_timestamp(v: chrono::NaiveDateTime) -> Timestamp {
        let date = naivedate_to_date(v.date());
        let time = naivetime_to_time(v.time());
        Timestamp { date, time }
    }
}
