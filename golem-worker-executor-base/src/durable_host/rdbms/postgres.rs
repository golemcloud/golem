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

use crate::durable_host::rdbms::rdbms_utils;
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
use sqlx::types::mac_address::MacAddress;
use sqlx::types::BitVec;
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
            DbValuePrimitive::Timestamp(v) => {
                let value = rdbms_utils::timestamp_to_naivedatetime(v)?;
                Ok(Self::Timestamp(value))
            }
            DbValuePrimitive::Timestamptz(v) => {
                let value = rdbms_utils::timestamptz_to_datetime(v)?;
                Ok(Self::Timestamptz(value))
            }
            DbValuePrimitive::Time(v) => {
                let value = rdbms_utils::time_to_nativetime(v)?;
                Ok(Self::Time(value))
            }
            DbValuePrimitive::Timetz(v) => {
                let value = rdbms_utils::timetz_to_nativetime_and_offset(v)?;
                Ok(Self::Timetz(value))
            }
            DbValuePrimitive::Date(v) => {
                let value = rdbms_utils::date_to_nativedate(v)?;
                Ok(Self::Date(value))
            }
            DbValuePrimitive::Interval(v) => Ok(Self::Interval(v)),
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
            DbValuePrimitive::Jsonpath(s) => Ok(Self::Jsonpath(s)),
            DbValuePrimitive::Xml(s) => Ok(Self::Xml(s)),
            DbValuePrimitive::Uuid((h, l)) => Ok(Self::Uuid(Uuid::from_u64_pair(h, l))),
            DbValuePrimitive::Bit(v) => Ok(Self::Bit(BitVec::from_iter(v))),
            DbValuePrimitive::Varbit(v) => Ok(Self::Varbit(BitVec::from_iter(v))),
            DbValuePrimitive::Oid(v) => Ok(Self::Oid(v)),
            DbValuePrimitive::Inet(v) => Ok(Self::Inet(v.into())),
            DbValuePrimitive::Cidr(v) => Ok(Self::Cidr(v.into())),
            DbValuePrimitive::Macaddr(v) => Ok(Self::Macaddr(MacAddress::new(v.into()))),
            DbValuePrimitive::Int4range(v) => {
                let v = postgres_utils::int4range_to_bounds(v);
                Ok(Self::Int4range(v))
            }
            DbValuePrimitive::Int8range(v) => {
                let v = postgres_utils::int8range_to_bounds(v);
                Ok(Self::Int8range(v))
            }
            DbValuePrimitive::Numrange(v) => {
                let v = postgres_utils::numrange_to_bounds(v)?;
                Ok(Self::Numrange(v))
            }
            DbValuePrimitive::Tsrange(v) => {
                let v = postgres_utils::tsrange_to_bounds(v)?;
                Ok(Self::Tsrange(v))
            }
            DbValuePrimitive::Tstzrange(v) => {
                let v = postgres_utils::tstzrange_to_bounds(v)?;
                Ok(Self::Tstzrange(v))
            }
            DbValuePrimitive::Daterange(v) => {
                let v = postgres_utils::daterange_to_bounds(v)?;
                Ok(Self::Daterange(v))
            }
            DbValuePrimitive::Money(v) => Ok(Self::Money(v)),
            DbValuePrimitive::CustomEnum(v) => Ok(Self::CustomEnum(v)),
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
                Self::Timestamp(rdbms_utils::naivedatetime_to_timestamp(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Timestamptz(v) => {
                Self::Timestamptz(rdbms_utils::datetime_to_timestamptz(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Time(v) => {
                Self::Time(rdbms_utils::naivetime_to_time(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Timetz((v, o)) => {
                Self::Timetz(rdbms_utils::naivetime_and_offset_to_time(v, o))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Date(v) => {
                Self::Date(rdbms_utils::naivedate_to_date(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Interval(v) => {
                Self::Interval(v)
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
            crate::services::rdbms::postgres::types::DbValuePrimitive::Jsonpath(s) => {
                Self::Jsonpath(s)
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Xml(s) => Self::Xml(s),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Uuid(uuid) => {
                Self::Uuid(uuid.as_u64_pair())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Bit(v) => {
                Self::Bit(v.iter().collect())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Varbit(v) => {
                Self::Varbit(v.iter().collect())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Inet(v) => {
                Self::Inet(v.into())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Cidr(v) => {
                Self::Cidr(v.into())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Macaddr(v) => {
                let v = v.bytes();
                DbValuePrimitive::Macaddr(v.into())
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Tsrange(v) => {
                Self::Tsrange(postgres_utils::bounds_to_tsrange(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Tstzrange(v) => {
                Self::Tstzrange(postgres_utils::bounds_to_tstzrange(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Daterange(v) => {
                Self::Daterange(postgres_utils::bounds_to_daterange(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Int4range(v) => {
                Self::Int4range(postgres_utils::bounds_to_int4range(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Int8range(v) => {
                Self::Int8range(postgres_utils::bounds_to_int8range(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Numrange(v) => {
                Self::Numrange(postgres_utils::bounds_to_numrange(v))
            }
            crate::services::rdbms::postgres::types::DbValuePrimitive::Oid(v) => Self::Oid(v),
            crate::services::rdbms::postgres::types::DbValuePrimitive::Money(v) => Self::Money(v),
            crate::services::rdbms::postgres::types::DbValuePrimitive::CustomEnum(v) => {
                Self::CustomEnum(v)
            }
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
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Jsonpath => {
                Self::Jsonpath
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Xml => Self::Xml,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Bit => Self::Bit,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Varbit => Self::Varbit,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Oid => Self::Oid,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Inet => Self::Inet,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Cidr => Self::Cidr,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Macaddr => {
                Self::Macaddr
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Int4range => {
                Self::Int4range
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Int8range => {
                Self::Int8range
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Numrange => {
                Self::Numrange
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Tsrange => {
                Self::Tsrange
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Tstzrange => {
                Self::Tstzrange
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Daterange => {
                Self::Daterange
            }
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Uuid => Self::Uuid,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::Money => Self::Money,
            crate::services::rdbms::postgres::types::DbColumnTypePrimitive::CustomEnum(v) => {
                Self::CustomEnum(v)
            }
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

impl From<IpAddr> for IpAddress {
    fn from(value: IpAddr) -> Self {
        match value {
            IpAddr::V4(v) => Self::Ipv4(v.octets().into()),
            IpAddr::V6(v) => Self::Ipv6(v.segments().into()),
        }
    }
}

impl From<IpAddress> for IpAddr {
    fn from(value: IpAddress) -> Self {
        match value {
            IpAddress::Ipv4((a, b, c, d)) => {
                let v = Ipv4Addr::new(a, b, c, d);
                IpAddr::V4(v)
            }
            IpAddress::Ipv6((a, b, c, d, e, f, g, h)) => {
                let v = Ipv6Addr::new(a, b, c, d, e, f, g, h);
                IpAddr::V6(v)
            }
        }
    }
}

pub(crate) mod postgres_utils {
    use crate::durable_host::rdbms::rdbms_utils::{
        date_to_nativedate, datetime_to_timestamptz, naivedate_to_date, naivedatetime_to_timestamp,
        timestamp_to_naivedatetime, timestamptz_to_datetime,
    };
    use crate::preview2::wasi::rdbms::postgres::{
        Daterange, Int4range, Int8range, Numrange, Tsrange, Tstzrange,
    };
    use bigdecimal::BigDecimal;
    use std::ops::Bound;
    use std::str::FromStr;

    type NaiveDateTimeBounds = (Bound<chrono::NaiveDateTime>, Bound<chrono::NaiveDateTime>);
    type DateTimeBounds = (
        Bound<chrono::DateTime<chrono::Utc>>,
        Bound<chrono::DateTime<chrono::Utc>>,
    );
    type NaiveDateBounds = (Bound<chrono::NaiveDate>, Bound<chrono::NaiveDate>);
    type I32Bounds = (Bound<i32>, Bound<i32>);
    type I64Bounds = (Bound<i64>, Bound<i64>);
    type BigDecimalBounds = (Bound<BigDecimal>, Bound<BigDecimal>);

    pub(crate) fn int4range_to_bounds(value: Int4range) -> I32Bounds {
        let (lower, upper) = value;
        let lower = to_bounds(lower);
        let upper = to_bounds(upper);
        (lower, upper)
    }

    pub(crate) fn int8range_to_bounds(value: Int8range) -> I64Bounds {
        let (lower, upper) = value;
        let lower = to_bounds(lower);
        let upper = to_bounds(upper);
        (lower, upper)
    }

    pub(crate) fn numrange_to_bounds(value: Numrange) -> Result<BigDecimalBounds, String> {
        let (lower, upper) = value;
        let lower = to_converted_bounds(lower, |v| {
            BigDecimal::from_str(&v).map_err(|e| e.to_string())
        })?;
        let upper = to_converted_bounds(upper, |v| {
            BigDecimal::from_str(&v).map_err(|e| e.to_string())
        })?;
        Ok((lower, upper))
    }

    pub(crate) fn tsrange_to_bounds(value: Tsrange) -> Result<NaiveDateTimeBounds, String> {
        let (lower, upper) = value;
        let lower = to_converted_bounds(lower, timestamp_to_naivedatetime)?;
        let upper = to_converted_bounds(upper, timestamp_to_naivedatetime)?;
        Ok((lower, upper))
    }

    pub(crate) fn tstzrange_to_bounds(value: Tstzrange) -> Result<DateTimeBounds, String> {
        let (lower, upper) = value;
        let lower = to_converted_bounds(lower, timestamptz_to_datetime)?;
        let upper = to_converted_bounds(upper, timestamptz_to_datetime)?;
        Ok((lower, upper))
    }

    pub(crate) fn daterange_to_bounds(value: Daterange) -> Result<NaiveDateBounds, String> {
        let (lower, upper) = value;
        let lower = to_converted_bounds(lower, date_to_nativedate)?;
        let upper = to_converted_bounds(upper, date_to_nativedate)?;
        Ok((lower, upper))
    }

    fn to_bounds<T>(value: Option<(T, bool)>) -> Bound<T> {
        match value {
            Some((v, true)) => Bound::Included(v),
            Some((v, false)) => Bound::Excluded(v),
            None => Bound::Unbounded,
        }
    }

    fn to_converted_bounds<I, O>(
        value: Option<(I, bool)>,
        f: impl Fn(I) -> Result<O, String>,
    ) -> Result<Bound<O>, String> {
        match value {
            Some((v, true)) => {
                let v = f(v)?;
                Ok(Bound::Included(v))
            }
            Some((v, false)) => {
                let v = f(v)?;
                Ok(Bound::Excluded(v))
            }
            None => Ok(Bound::Unbounded),
        }
    }

    pub(crate) fn bounds_to_int4range(value: I32Bounds) -> Int4range {
        let (lower, upper) = value;
        let lower = from_bounds(lower);
        let upper = from_bounds(upper);
        (lower, upper)
    }

    pub(crate) fn bounds_to_int8range(value: I64Bounds) -> Int8range {
        let (lower, upper) = value;
        let lower = from_bounds(lower);
        let upper = from_bounds(upper);
        (lower, upper)
    }

    pub(crate) fn bounds_to_numrange(value: BigDecimalBounds) -> Numrange {
        let (lower, upper) = value;
        let lower = from_bounds(lower.map(|v| v.to_string()));
        let upper = from_bounds(upper.map(|v| v.to_string()));
        (lower, upper)
    }

    pub(crate) fn bounds_to_tsrange(value: NaiveDateTimeBounds) -> Tsrange {
        let (lower, upper) = value;
        let lower = from_bounds(lower.map(naivedatetime_to_timestamp));
        let upper = from_bounds(upper.map(naivedatetime_to_timestamp));
        (lower, upper)
    }

    pub(crate) fn bounds_to_tstzrange(value: DateTimeBounds) -> Tstzrange {
        let (lower, upper) = value;
        let lower = from_bounds(lower.map(datetime_to_timestamptz));
        let upper = from_bounds(upper.map(datetime_to_timestamptz));
        (lower, upper)
    }

    pub(crate) fn bounds_to_daterange(value: NaiveDateBounds) -> Daterange {
        let (lower, upper) = value;
        let lower = from_bounds(lower.map(naivedate_to_date));
        let upper = from_bounds(upper.map(naivedate_to_date));
        (lower, upper)
    }

    fn from_bounds<T>(value: Bound<T>) -> Option<(T, bool)> {
        match value {
            Bound::Included(v) => Some((v, true)),
            Bound::Excluded(v) => Some((v, false)),
            Bound::Unbounded => None,
        }
    }
}
