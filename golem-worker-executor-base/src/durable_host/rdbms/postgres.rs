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
use crate::preview2::wasi::rdbms::postgres::{
    DbColumn, DbColumnType, DbColumnTypePrimitive, DbRow, DbValue, DbValuePrimitive, Error, Host,
    HostDbConnection, HostDbResultSet, IpAddress,
};
use crate::services::rdbms::postgres::PostgresType;
use crate::services::rdbms::RdbmsPoolKey;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use chrono::{Datelike, Offset, Timelike};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {}

pub struct PostgresDbConnection {
    pub pool_key: RdbmsPoolKey,
}

impl PostgresDbConnection {
    pub fn new(pool_key: RdbmsPoolKey) -> Self {
        Self { pool_key }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<PostgresDbConnection>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-connection", "open");

        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let result = self
            .state
            .rdbms_service
            .postgres()
            .create(&address, &worker_id)
            .await;

        match result {
            Ok(key) => {
                let entry = PostgresDbConnection::new(key);
                let resource = self.as_wasi_view().table().push(entry)?;
                Ok(Ok(resource))
            }
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn query(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSetEntry>, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-connection", "query");
        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<PostgresDbConnection>(&self_)?
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
                    .postgres()
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
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-connection", "execute");
        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<PostgresDbConnection>(&self_)?
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
                    .postgres()
                    .execute(&pool_key, &worker_id, &statement, params)
                    .await
                    .map_err(|e| e.into());

                Ok(result)
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    fn drop(&mut self, rep: Resource<PostgresDbConnection>) -> anyhow::Result<()> {
        record_host_function_call("rdbms::postgres::db-connection", "drop");

        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<PostgresDbConnection>(&rep)?
            .pool_key
            .clone();

        let _ = self
            .state
            .rdbms_service
            .postgres()
            .remove(&pool_key, &worker_id);

        self.as_wasi_view()
            .table()
            .delete::<PostgresDbConnection>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbConnection for &mut DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<PostgresDbConnection>, Error>> {
        (*self).open(address).await
    }

    async fn query(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultSetEntry>, Error>> {
        (*self).query(self_, statement, params).await
    }

    async fn execute(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        (*self).execute(self_, statement, params).await
    }

    fn drop(&mut self, rep: Resource<PostgresDbConnection>) -> anyhow::Result<()> {
        // (*self).drop(rep)
        HostDbConnection::drop(*self, rep)
    }
}

pub struct DbResultSetEntry {
    pub internal: Arc<dyn crate::services::rdbms::DbResultSet<PostgresType> + Send + Sync>,
}

impl DbResultSetEntry {
    pub fn new(
        internal: Arc<dyn crate::services::rdbms::DbResultSet<PostgresType> + Send + Sync>,
    ) -> Self {
        Self { internal }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::postgres::DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-result-set", "get-columns");

        let internal = self
            .as_wasi_view()
            .table()
            .get::<crate::durable_host::rdbms::postgres::DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let columns = internal.deref().get_columns().await.map_err(Error::from)?;

        let columns = columns.into_iter().map(|c| c.into()).collect();
        Ok(columns)
    }

    async fn get_next(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::postgres::DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::postgres::db-result-set", "get-next");
        let internal = self
            .as_wasi_view()
            .table()
            .get::<crate::durable_host::rdbms::postgres::DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let rows = internal.deref().get_next().await.map_err(Error::from)?;

        let rows = rows.map(|r| r.into_iter().map(|r| r.into()).collect());
        Ok(rows)
    }

    fn drop(
        &mut self,
        rep: Resource<crate::durable_host::rdbms::postgres::DbResultSetEntry>,
    ) -> anyhow::Result<()> {
        record_host_function_call("rdbms::postgres::db-result-set", "drop");
        self.as_wasi_view()
            .table()
            .delete::<crate::durable_host::rdbms::postgres::DbResultSetEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for &mut DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::postgres::DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        (*self).get_columns(self_).await
    }

    async fn get_next(
        &mut self,
        self_: Resource<crate::durable_host::rdbms::postgres::DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        (*self).get_next(self_).await
    }

    fn drop(
        &mut self,
        rep: Resource<crate::durable_host::rdbms::postgres::DbResultSetEntry>,
    ) -> anyhow::Result<()> {
        // (*self).drop(rep)
        HostDbResultSet::drop(*self, rep)
    }
}

impl TryFrom<DbValuePrimitive> for crate::services::rdbms::postgres::types::DbValuePrimitive {
    type Error = String;
    fn try_from(value: DbValuePrimitive) -> Result<Self, Self::Error> {
        match value {
            DbValuePrimitive::Character(v) => Ok(Self::Character(v)),
            DbValuePrimitive::Int2(i) => Ok(Self::Int2(i)),
            DbValuePrimitive::Int4(i) => Ok(Self::Int4(i)),
            DbValuePrimitive::Int8(i) => Ok(Self::Int8(i)),
            DbValuePrimitive::Numeric(s) => {
                let v = bigdecimal::BigDecimal::from_str(&s).map_err(|e| e.to_string())?;
                Ok(Self::Numeric(v))
            }
            DbValuePrimitive::Float4(f) => Ok(Self::Float4(f)),
            DbValuePrimitive::Float8(f) => Ok(Self::Float8(f)),
            DbValuePrimitive::Boolean(b) => Ok(Self::Boolean(b)),
            DbValuePrimitive::Timestamp((year, month, day, hour, minute, second, nanosecond)) => {
                let date = chrono::naive::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
                    .ok_or("Date value is not valid")?;
                let time = chrono::NaiveTime::from_hms_nano_opt(
                    hour as u32,
                    minute as u32,
                    second as u32,
                    nanosecond,
                )
                .ok_or("Time value is not valid")?;

                Ok(Self::Timestamp(
                    chrono::naive::NaiveDateTime::new(date, time).and_utc(),
                ))
            }
            DbValuePrimitive::Timestamptz((
                year,
                month,
                day,
                hour,
                minute,
                second,
                nanosecond,
                offset,
            )) => {
                let date = chrono::naive::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
                    .ok_or("Date value is not valid")?;
                let time = chrono::NaiveTime::from_hms_nano_opt(
                    hour as u32,
                    minute as u32,
                    second as u32,
                    nanosecond,
                )
                .ok_or("Time value is not valid")?;
                let offset = chrono::offset::FixedOffset::west_opt(offset)
                    .ok_or("Offset value is not valid")?;
                let datetime = chrono::naive::NaiveDateTime::new(date, time)
                    .checked_add_offset(offset)
                    .ok_or("Offset value is not valid")?;
                Ok(Self::Timestamptz(datetime.and_utc()))
            }
            DbValuePrimitive::Time((hour, minute, second, nanosecond)) => {
                let time = chrono::NaiveTime::from_hms_nano_opt(
                    hour as u32,
                    minute as u32,
                    second as u32,
                    nanosecond,
                )
                .ok_or("Time value is not valid")?;
                Ok(Self::Time(time))
            }
            DbValuePrimitive::Timetz((hour, minute, second, nanosecond, offset)) => {
                let time = chrono::NaiveTime::from_hms_nano_opt(
                    hour as u32,
                    minute as u32,
                    second as u32,
                    nanosecond,
                )
                .ok_or("Time value is not valid")?;
                let offset = chrono::offset::FixedOffset::west_opt(offset)
                    .ok_or("Offset value is not valid")?;
                Ok(Self::Timetz((time, offset)))
            }
            DbValuePrimitive::Date((year, month, day)) => {
                let date = chrono::naive::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
                    .ok_or("Date value is not valid")?;
                Ok(Self::Date(date))
            }
            DbValuePrimitive::Interval(v) => Ok(Self::Interval(chrono::Duration::microseconds(v))),
            DbValuePrimitive::Text(s) => Ok(Self::Text(s)),
            DbValuePrimitive::Varchar(s) => Ok(Self::Varchar(s)),
            DbValuePrimitive::Bpchar(s) => Ok(Self::Bpchar(s)),
            DbValuePrimitive::Bytea(u) => Ok(Self::Bytea(u)),
            DbValuePrimitive::Json(v) => {
                let v: serde_json::Value = serde_json::from_str(&v).map_err(|e| e.to_string())?;
                Ok(Self::Json(v))
            }
            DbValuePrimitive::Jsonb(v) => {
                let v: serde_json::Value = serde_json::from_str(&v).map_err(|e| e.to_string())?;
                Ok(Self::Jsonb(v))
            }
            DbValuePrimitive::Xml(s) => Ok(Self::Xml(s)),
            DbValuePrimitive::Uuid((h, l)) => Ok(Self::Uuid(Uuid::from_u64_pair(h, l))),
            DbValuePrimitive::Bit(v) => Ok(Self::Bit(v)),
            DbValuePrimitive::Varbit(v) => Ok(Self::Varbit(v)),
            DbValuePrimitive::Oid(v) => Ok(Self::Oid(v)),
            DbValuePrimitive::Inet(v) => match v {
                IpAddress::Ipv4((a, b, c, d)) => {
                    let v = Ipv4Addr::new(a, b, c, d);
                    Ok(Self::Inet(IpAddr::V4(v)))
                }
                IpAddress::Ipv6((a, b, c, d, e, f, g, h)) => {
                    let v = Ipv6Addr::new(a, b, c, d, e, f, g, h);
                    Ok(Self::Inet(IpAddr::V6(v)))
                }
            },
            DbValuePrimitive::Null => Ok(Self::Null),
        }
    }
}

impl From<crate::services::rdbms::postgres::types::DbValuePrimitive> for DbValuePrimitive {
    fn from(value: crate::services::rdbms::postgres::types::DbValuePrimitive) -> Self {
        match value {
            crate::services::rdbms::postgres::types::DbValuePrimitive::Character(s) => {
                Self::Character(s)
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Int2(i) => Self::Int2(i),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Int4(i) => Self::Int4(i),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Int8(i) => Self::Int8(i),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Numeric(s) => {
                Self::Numeric(s.to_string())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Float4(f) => Self::Float4(f),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Float8(f) => Self::Float8(f),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Boolean(b) => {
                Self::Boolean(b)
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Timestamp(v) => {
                let year = v.date_naive().year();
                let month = v.date_naive().month() as u8;
                let day = v.date_naive().day() as u8;
                let hour = v.time().hour() as u8;
                let minute = v.time().minute() as u8;
                let second = v.time().second() as u8;
                let nanosecond = v.time().nanosecond();
                Self::Timestamp((year, month, day, hour, minute, second, nanosecond))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Timestamptz(v) => {
                let year = v.date_naive().year();
                let month = v.date_naive().month() as u8;
                let day = v.date_naive().day() as u8;
                let hour = v.time().hour() as u8;
                let minute = v.time().minute() as u8;
                let second = v.time().second() as u8;
                let nanosecond = v.time().nanosecond();
                let offset = v.offset().fix().local_minus_utc();
                Self::Timestamptz((year, month, day, hour, minute, second, nanosecond, offset))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Time(v) => {
                let hour = v.hour() as u8;
                let minute = v.minute() as u8;
                let second = v.second() as u8;
                let nanosecond = v.nanosecond();
                Self::Time((hour, minute, second, nanosecond))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Timetz((v, o)) => {
                let hour = v.hour() as u8;
                let minute = v.minute() as u8;
                let second = v.second() as u8;
                let nanosecond = v.nanosecond();
                let offset = o.local_minus_utc();
                Self::Timetz((hour, minute, second, nanosecond, offset))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Date(v) => {
                let year = v.year();
                let month = v.month() as u8;
                let day = v.day() as u8;
                Self::Date((year, month, day))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Interval(v) => {
                Self::Interval(v.num_milliseconds())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Text(s) => Self::Text(s),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Varchar(s) => {
                Self::Varchar(s)
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Bpchar(s) => Self::Bpchar(s),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Bytea(u) => Self::Bytea(u),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Json(s) => {
                Self::Json(s.to_string())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Jsonb(s) => {
                Self::Jsonb(s.to_string())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Xml(s) => Self::Xml(s),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Uuid(uuid) => {
                Self::Uuid(uuid.as_u64_pair())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Bit(v) => Self::Bit(v),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Varbit(v) => Self::Varbit(v),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Oid(v) => Self::Oid(v),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Inet(v) => match v {
                IpAddr::V4(v) => {
                    let octets = v.octets();
                    Self::Inet(IpAddress::Ipv4((
                        octets[0], octets[1], octets[2], octets[3],
                    )))
                }
                IpAddr::V6(v) => {
                    let segments = v.segments();
                    Self::Inet(IpAddress::Ipv6((
                        segments[0],
                        segments[1],
                        segments[2],
                        segments[3],
                        segments[4],
                        segments[5],
                        segments[6],
                        segments[7],
                    )))
                }
            },
            crate::services::rdbms::postgres::types::DbValuePrimitive::Null => Self::Null,
        }
    }
}

impl TryFrom<DbValue> for crate::services::rdbms::postgres::types::DbValue {
    type Error = String;
    fn try_from(value: DbValue) -> Result<Self, Self::Error> {
        match value {
            DbValue::Primitive(p) => {
                let v = p.try_into()?;
                Ok(Self::Primitive(v))
            }
            DbValue::Array(vs) => {
                let vs = vs
                    .into_iter()
                    .map(|v| v.try_into())
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(Self::Array(vs))
            }
        }
    }
}

impl From<crate::services::rdbms::postgres::types::DbValue> for DbValue {
    fn from(value: crate::services::rdbms::postgres::types::DbValue) -> Self {
        match value {
            crate::services::rdbms::postgres::types::DbValue::Primitive(p) => {
                Self::Primitive(p.into())
            }
            crate::services::rdbms::postgres::types::DbValue::Array(vs) => {
                Self::Array(vs.into_iter().map(|v| v.into()).collect())
            }
        }
    }
}

impl From<crate::services::rdbms::DbRow<crate::services::rdbms::postgres::types::DbValue>>
    for DbRow
{
    fn from(
        value: crate::services::rdbms::DbRow<crate::services::rdbms::postgres::types::DbValue>,
    ) -> Self {
        Self {
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl From<crate::services::rdbms::postgres::types::DbColumnTypePrimitive>
    for DbColumnTypePrimitive
{
    fn from(value: crate::services::rdbms::postgres::types::DbColumnTypePrimitive) -> Self {
        match value {
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Character => {
                Self::Character
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Int2 => Self::Int2,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Int4 => Self::Int4,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Int8 => Self::Int8,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Numeric => {
                Self::Numeric
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Float4 => Self::Float4,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Float8 => Self::Float8,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Boolean => {
                Self::Boolean
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Timestamp => {
                Self::Timestamp
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Timestamptz => {
                Self::Timestamptz
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Interval => {
                Self::Interval
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Time => Self::Time,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Timetz => Self::Timetz,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Date => Self::Date,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Text => Self::Text,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Varchar => {
                Self::Varchar
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Bpchar => Self::Bpchar,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Bytea => Self::Bytea,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Jsonb => Self::Jsonb,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Json => Self::Json,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Xml => Self::Xml,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Bit => Self::Bit,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Varbit => Self::Varbit,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Oid => Self::Oid,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Inet => Self::Inet,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Uuid => Self::Uuid,
        }
    }
}

impl From<crate::services::rdbms::postgres::types::DbColumnType> for DbColumnType {
    fn from(value: crate::services::rdbms::postgres::types::DbColumnType) -> Self {
        match value {
            crate::services::rdbms::postgres::types::DbColumnType::Primitive(p) => {
                Self::Primitive(p.into())
            }
            crate::services::rdbms::postgres::types::DbColumnType::Array(p) => {
                Self::Array(p.into())
            }
        }
    }
}

impl From<crate::services::rdbms::postgres::types::DbColumn> for DbColumn {
    fn from(value: crate::services::rdbms::postgres::types::DbColumn) -> Self {
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

// impl From<rdbms_types::DbColumnTypeMeta> for DbColumnTypeMeta {
//     fn from(value: rdbms_types::DbColumnTypeMeta) -> Self {
//         Self {
//             name: value.name,
//             db_type: value.db_type.into(),
//             db_type_flags: value
//                 .db_type_flags
//                 .iter()
//                 .fold(DbColumnTypeFlags::empty(), |a, b| a | b.clone().into()),
//             foreign_key: value.foreign_key,
//         }
//     }
// }
//
// impl From<rdbms_types::DbColumnTypeFlag> for DbColumnTypeFlags {
//     fn from(value: rdbms_types::DbColumnTypeFlag) -> Self {
//         match value {
//             rdbms_types::DbColumnTypeFlag::PrimaryKey => DbColumnTypeFlags::PRIMARY_KEY,
//             rdbms_types::DbColumnTypeFlag::ForeignKey => DbColumnTypeFlags::FOREIGN_KEY,
//             rdbms_types::DbColumnTypeFlag::Unique => DbColumnTypeFlags::UNIQUE,
//             rdbms_types::DbColumnTypeFlag::Nullable => DbColumnTypeFlags::NULLABLE,
//             rdbms_types::DbColumnTypeFlag::Generated => DbColumnTypeFlags::GENERATED,
//             rdbms_types::DbColumnTypeFlag::AutoIncrement => DbColumnTypeFlags::AUTO_INCREMENT,
//             rdbms_types::DbColumnTypeFlag::DefaultValue => DbColumnTypeFlags::DEFAULT_VALUE,
//             rdbms_types::DbColumnTypeFlag::Indexed => DbColumnTypeFlags::INDEXED,
//         }
//     }
// }
