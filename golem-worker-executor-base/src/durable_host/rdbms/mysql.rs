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
    Date, DbColumn, DbColumnType, DbRow, DbValue, Error, Host, HostDbConnection, HostDbResultSet,
    Time, Timestamp,
};
use crate::services::rdbms::mysql::types as mysql_types;
use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::RdbmsPoolKey;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use chrono::{Datelike, Timelike};
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

impl TryFrom<DbValue> for mysql_types::DbValue {
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
                let value = v.try_into()?;
                Ok(Self::Timestamp(value))
            }
            DbValue::Date(v) => {
                let value = v.try_into()?;
                Ok(Self::Date(value))
            }
            DbValue::Time(v) => {
                let value = v.try_into()?;
                Ok(Self::Time(value))
            }
            DbValue::Datetime(v) => {
                let value = v.try_into()?;
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

impl From<mysql_types::DbValue> for DbValue {
    fn from(value: mysql_types::DbValue) -> Self {
        match value {
            mysql_types::DbValue::Boolean(v) => Self::Boolean(v),
            mysql_types::DbValue::Tinyint(v) => Self::Tinyint(v),
            mysql_types::DbValue::Smallint(v) => Self::Smallint(v),
            mysql_types::DbValue::Mediumint(v) => Self::Mediumint(v),
            mysql_types::DbValue::Int(v) => Self::Int(v),
            mysql_types::DbValue::Bigint(v) => Self::Bigint(v),
            mysql_types::DbValue::TinyintUnsigned(v) => Self::TinyintUnsigned(v),
            mysql_types::DbValue::SmallintUnsigned(v) => Self::SmallintUnsigned(v),
            mysql_types::DbValue::MediumintUnsigned(v) => Self::MediumintUnsigned(v),
            mysql_types::DbValue::IntUnsigned(v) => Self::IntUnsigned(v),
            mysql_types::DbValue::BigintUnsigned(v) => Self::BigintUnsigned(v),
            mysql_types::DbValue::Decimal(v) => Self::Decimal(v.to_string()),
            mysql_types::DbValue::Float(v) => Self::Float(v),
            mysql_types::DbValue::Double(v) => Self::Double(v),
            mysql_types::DbValue::Text(v) => Self::Text(v),
            mysql_types::DbValue::Varchar(v) => Self::Varchar(v),
            mysql_types::DbValue::Fixchar(v) => Self::Fixchar(v),
            mysql_types::DbValue::Blob(v) => Self::Blob(v),
            mysql_types::DbValue::Tinyblob(v) => Self::Tinyblob(v),
            mysql_types::DbValue::Mediumblob(v) => Self::Mediumblob(v),
            mysql_types::DbValue::Longblob(v) => Self::Longblob(v),
            mysql_types::DbValue::Binary(v) => Self::Binary(v),
            mysql_types::DbValue::Varbinary(v) => Self::Varbinary(v),
            mysql_types::DbValue::Tinytext(v) => Self::Tinytext(v),
            mysql_types::DbValue::Mediumtext(v) => Self::Mediumtext(v),
            mysql_types::DbValue::Longtext(v) => Self::Longtext(v),
            mysql_types::DbValue::Json(v) => Self::Json(v.to_string()),
            mysql_types::DbValue::Timestamp(v) => Self::Timestamp(v.into()),
            mysql_types::DbValue::Date(v) => Self::Date(v.into()),
            mysql_types::DbValue::Time(v) => Self::Time(v.into()),
            mysql_types::DbValue::Datetime(v) => Self::Datetime(v.into()),
            mysql_types::DbValue::Year(v) => Self::Year(v),
            mysql_types::DbValue::Set(v) => Self::Set(v),
            mysql_types::DbValue::Enumeration(v) => Self::Enumeration(v),
            mysql_types::DbValue::Bit(v) => Self::Bit(v.iter().collect()),
            mysql_types::DbValue::Null => Self::Null,
        }
    }
}

impl From<crate::services::rdbms::DbRow<mysql_types::DbValue>> for DbRow {
    fn from(value: crate::services::rdbms::DbRow<mysql_types::DbValue>) -> Self {
        Self {
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl From<mysql_types::DbColumnType> for DbColumnType {
    fn from(value: mysql_types::DbColumnType) -> Self {
        match value {
            mysql_types::DbColumnType::Boolean => Self::Boolean,
            mysql_types::DbColumnType::Tinyint => Self::Tinyint,
            mysql_types::DbColumnType::Smallint => Self::Smallint,
            mysql_types::DbColumnType::Mediumint => Self::Mediumint,
            mysql_types::DbColumnType::Int => Self::Int,
            mysql_types::DbColumnType::Bigint => Self::Bigint,
            mysql_types::DbColumnType::IntUnsigned => Self::IntUnsigned,
            mysql_types::DbColumnType::TinyintUnsigned => Self::TinyintUnsigned,
            mysql_types::DbColumnType::SmallintUnsigned => Self::SmallintUnsigned,
            mysql_types::DbColumnType::MediumintUnsigned => Self::MediumintUnsigned,
            mysql_types::DbColumnType::BigintUnsigned => Self::BigintUnsigned,
            mysql_types::DbColumnType::Float => Self::Float,
            mysql_types::DbColumnType::Double => Self::Double,
            mysql_types::DbColumnType::Decimal => Self::Decimal,
            mysql_types::DbColumnType::Text => Self::Text,
            mysql_types::DbColumnType::Varchar => Self::Varchar,
            mysql_types::DbColumnType::Fixchar => Self::Fixchar,
            mysql_types::DbColumnType::Blob => Self::Blob,
            mysql_types::DbColumnType::Json => Self::Json,
            mysql_types::DbColumnType::Timestamp => Self::Timestamp,
            mysql_types::DbColumnType::Date => Self::Date,
            mysql_types::DbColumnType::Time => todo!(), //Self::Time,
            mysql_types::DbColumnType::Datetime => Self::Datetime,
            mysql_types::DbColumnType::Year => Self::Year,
            mysql_types::DbColumnType::Bit => Self::Bit,
            mysql_types::DbColumnType::Binary => Self::Binary,
            mysql_types::DbColumnType::Varbinary => Self::Varbinary,
            mysql_types::DbColumnType::Tinyblob => Self::Tinyblob,
            mysql_types::DbColumnType::Mediumblob => Self::Mediumblob,
            mysql_types::DbColumnType::Longblob => Self::Longblob,
            mysql_types::DbColumnType::Tinytext => Self::Tinytext,
            mysql_types::DbColumnType::Mediumtext => Self::Mediumtext,
            mysql_types::DbColumnType::Longtext => Self::Longtext,
            mysql_types::DbColumnType::Enumeration => Self::Enumeration,
            mysql_types::DbColumnType::Set => Self::Set,
        }
    }
}

impl From<mysql_types::DbColumn> for DbColumn {
    fn from(value: mysql_types::DbColumn) -> Self {
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

impl TryFrom<Time> for chrono::NaiveTime {
    type Error = String;

    fn try_from(value: Time) -> Result<Self, Self::Error> {
        let time = chrono::NaiveTime::from_hms_nano_opt(
            value.hour as u32,
            value.minute as u32,
            value.second as u32,
            value.nanosecond,
        )
        .ok_or("Time value is not valid")?;
        Ok(time)
    }
}

impl TryFrom<Date> for chrono::NaiveDate {
    type Error = String;

    fn try_from(value: Date) -> Result<Self, Self::Error> {
        let date = chrono::naive::NaiveDate::from_ymd_opt(
            value.year,
            value.month as u32,
            value.day as u32,
        )
        .ok_or("Date value is not valid")?;
        Ok(date)
    }
}

impl TryFrom<Timestamp> for chrono::DateTime<chrono::Utc> {
    type Error = String;

    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let date = value.date.try_into()?;
        let time = value.time.try_into()?;
        Ok(chrono::naive::NaiveDateTime::new(date, time).and_utc())
    }
}

impl From<chrono::NaiveTime> for Time {
    fn from(v: chrono::NaiveTime) -> Self {
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
}

impl From<chrono::NaiveDate> for Date {
    fn from(v: chrono::NaiveDate) -> Self {
        let year = v.year();
        let month = v.month() as u8;
        let day = v.day() as u8;
        Date { year, month, day }
    }
}

impl From<chrono::DateTime<chrono::Utc>> for Timestamp {
    fn from(v: chrono::DateTime<chrono::Utc>) -> Self {
        let v = v.naive_utc();
        let date = v.date().into();
        let time = v.time().into();
        Timestamp { date, time }
    }
}
