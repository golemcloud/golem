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
    Date, Datebound, Daterange, DbColumn, DbColumnType, DbRow, DbValue, Enumeration,
    EnumerationType, Error, Host, HostDbConnection, HostDbResultSet, Int4bound, Int4range,
    Int8bound, Int8range, Interval, IpAddress, Numbound, Numrange, Time, Timestamp, Timestamptz,
    Timetz, Tsbound, Tsrange, Tstzbound, Tstzrange,
};
use crate::services::rdbms::postgres::types as postgres_types;
use crate::services::rdbms::postgres::PostgresType;
use crate::services::rdbms::RdbmsPoolKey;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use chrono::{Datelike, Offset, Timelike};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::{Bound, Deref};
use std::str::FromStr;
use std::sync::Arc;
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
        HostDbResultSet::drop(*self, rep)
    }
}

impl TryFrom<DbValue> for postgres_types::DbValue {
    type Error = String;
    fn try_from(value: DbValue) -> Result<Self, Self::Error> {
        postgres_utils::to_db_value(value)
    }
}

impl From<postgres_types::DbValue> for DbValue {
    fn from(value: postgres_types::DbValue) -> Self {
        postgres_utils::from_db_value(value)
    }
}

impl From<crate::services::rdbms::DbRow<postgres_types::DbValue>> for DbRow {
    fn from(value: crate::services::rdbms::DbRow<postgres_types::DbValue>) -> Self {
        Self {
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl From<postgres_types::DbColumnType> for DbColumnType {
    fn from(value: postgres_types::DbColumnType) -> Self {
        postgres_utils::from_db_column_type(value)
    }
}

impl From<postgres_types::DbColumn> for DbColumn {
    fn from(value: postgres_types::DbColumn) -> Self {
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

impl TryFrom<Timetz> for postgres_types::TimeTz {
    type Error = String;

    fn try_from(value: Timetz) -> Result<Self, Self::Error> {
        let time = value.time.try_into()?;
        let offset = chrono::offset::FixedOffset::west_opt(value.offset)
            .ok_or("Offset value is not valid")?;
        Ok(Self { time, offset })
    }
}

impl From<Interval> for postgres_types::Interval {
    fn from(v: Interval) -> Self {
        Self {
            months: v.months,
            days: v.days,
            microseconds: v.microseconds,
        }
    }
}

impl From<Enumeration> for postgres_types::Enum {
    fn from(v: Enumeration) -> Self {
        Self {
            name: v.name,
            value: v.value,
        }
    }
}

impl From<EnumerationType> for postgres_types::EnumType {
    fn from(v: EnumerationType) -> Self {
        Self { name: v.name }
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

impl TryFrom<Timestamp> for chrono::NaiveDateTime {
    type Error = String;

    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let date = value.date.try_into()?;
        let time = value.time.try_into()?;
        Ok(chrono::naive::NaiveDateTime::new(date, time))
    }
}

impl TryFrom<Timestamptz> for chrono::DateTime<chrono::Utc> {
    type Error = String;

    fn try_from(value: Timestamptz) -> Result<Self, Self::Error> {
        let datetime: chrono::NaiveDateTime = value.timestamp.try_into()?;
        let offset = chrono::offset::FixedOffset::west_opt(value.offset)
            .ok_or("Offset value is not valid")?;
        let datetime = datetime
            .checked_add_offset(offset)
            .ok_or("Offset value is not valid")?;
        Ok(datetime.and_utc())
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

impl From<postgres_types::TimeTz> for Timetz {
    fn from(v: postgres_types::TimeTz) -> Self {
        let time = v.time.into();
        let offset = v.offset.local_minus_utc();
        Timetz { time, offset }
    }
}

impl From<postgres_types::Interval> for Interval {
    fn from(v: postgres_types::Interval) -> Self {
        Self {
            months: v.months,
            days: v.days,
            microseconds: v.microseconds,
        }
    }
}

impl From<postgres_types::Enum> for Enumeration {
    fn from(v: postgres_types::Enum) -> Self {
        Self {
            name: v.name,
            value: v.value,
        }
    }
}

impl From<postgres_types::EnumType> for EnumerationType {
    fn from(v: postgres_types::EnumType) -> Self {
        Self { name: v.name }
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

impl From<chrono::NaiveDateTime> for Timestamp {
    fn from(v: chrono::NaiveDateTime) -> Self {
        let date = v.date().into();
        let time = v.time().into();
        Timestamp { date, time }
    }
}

impl From<chrono::DateTime<chrono::Utc>> for Timestamptz {
    fn from(v: chrono::DateTime<chrono::Utc>) -> Self {
        let timestamp = v.naive_utc().into();
        let offset = v.offset().fix().local_minus_utc();
        Timestamptz { timestamp, offset }
    }
}

impl From<Int4range> for postgres_types::ValuesRange<i32> {
    fn from(value: Int4range) -> Self {
        fn to_bounds(v: Int4bound) -> Bound<i32> {
            match v {
                Int4bound::Included(v) => Bound::Included(v),
                Int4bound::Excluded(v) => Bound::Excluded(v),
                Int4bound::Unbounded => Bound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl From<Int8range> for postgres_types::ValuesRange<i64> {
    fn from(value: Int8range) -> Self {
        fn to_bounds(v: Int8bound) -> Bound<i64> {
            match v {
                Int8bound::Included(v) => Bound::Included(v),
                Int8bound::Excluded(v) => Bound::Excluded(v),
                Int8bound::Unbounded => Bound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl TryFrom<Numrange> for postgres_types::ValuesRange<BigDecimal> {
    type Error = String;

    fn try_from(value: Numrange) -> Result<Self, Self::Error> {
        fn to_bounds(v: Numbound) -> Result<Bound<BigDecimal>, String> {
            match v {
                Numbound::Included(v) => Ok(Bound::Included(
                    BigDecimal::from_str(&v).map_err(|e| e.to_string())?,
                )),
                Numbound::Excluded(v) => Ok(Bound::Excluded(
                    BigDecimal::from_str(&v).map_err(|e| e.to_string())?,
                )),
                Numbound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        Ok(Self {
            start: to_bounds(value.start)?,
            end: to_bounds(value.end)?,
        })
    }
}

impl TryFrom<Daterange> for postgres_types::ValuesRange<chrono::NaiveDate> {
    type Error = String;

    fn try_from(value: Daterange) -> Result<Self, Self::Error> {
        fn to_bounds(v: Datebound) -> Result<Bound<chrono::NaiveDate>, String> {
            match v {
                Datebound::Included(v) => Ok(Bound::Included(v.try_into()?)),
                Datebound::Excluded(v) => Ok(Bound::Excluded(v.try_into()?)),
                Datebound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        Ok(Self {
            start: to_bounds(value.start)?,
            end: to_bounds(value.end)?,
        })
    }
}

impl TryFrom<Tsrange> for postgres_types::ValuesRange<chrono::NaiveDateTime> {
    type Error = String;

    fn try_from(value: Tsrange) -> Result<Self, Self::Error> {
        fn to_bounds(v: Tsbound) -> Result<Bound<chrono::NaiveDateTime>, String> {
            match v {
                Tsbound::Included(v) => Ok(Bound::Included(v.try_into()?)),
                Tsbound::Excluded(v) => Ok(Bound::Excluded(v.try_into()?)),
                Tsbound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        Ok(Self {
            start: to_bounds(value.start)?,
            end: to_bounds(value.end)?,
        })
    }
}

impl TryFrom<Tstzrange> for postgres_types::ValuesRange<chrono::DateTime<chrono::Utc>> {
    type Error = String;

    fn try_from(value: Tstzrange) -> Result<Self, Self::Error> {
        fn to_bounds(v: Tstzbound) -> Result<Bound<chrono::DateTime<chrono::Utc>>, String> {
            match v {
                Tstzbound::Included(v) => Ok(Bound::Included(v.try_into()?)),
                Tstzbound::Excluded(v) => Ok(Bound::Excluded(v.try_into()?)),
                Tstzbound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        Ok(Self {
            start: to_bounds(value.start)?,
            end: to_bounds(value.end)?,
        })
    }
}

impl From<postgres_types::ValuesRange<i32>> for Int4range {
    fn from(value: postgres_types::ValuesRange<i32>) -> Self {
        fn to_bounds(v: Bound<i32>) -> Int4bound {
            match v {
                Bound::Included(v) => Int4bound::Included(v),
                Bound::Excluded(v) => Int4bound::Excluded(v),
                Bound::Unbounded => Int4bound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl From<postgres_types::ValuesRange<i64>> for Int8range {
    fn from(value: postgres_types::ValuesRange<i64>) -> Self {
        fn to_bounds(v: Bound<i64>) -> Int8bound {
            match v {
                Bound::Included(v) => Int8bound::Included(v),
                Bound::Excluded(v) => Int8bound::Excluded(v),
                Bound::Unbounded => Int8bound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl From<postgres_types::ValuesRange<BigDecimal>> for Numrange {
    fn from(value: postgres_types::ValuesRange<BigDecimal>) -> Self {
        fn to_bounds(v: Bound<BigDecimal>) -> Numbound {
            match v {
                Bound::Included(v) => Numbound::Included(v.to_string()),
                Bound::Excluded(v) => Numbound::Excluded(v.to_string()),
                Bound::Unbounded => Numbound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl From<postgres_types::ValuesRange<chrono::DateTime<chrono::Utc>>> for Tstzrange {
    fn from(value: postgres_types::ValuesRange<chrono::DateTime<chrono::Utc>>) -> Self {
        fn to_bounds(v: Bound<chrono::DateTime<chrono::Utc>>) -> Tstzbound {
            match v {
                Bound::Included(v) => Tstzbound::Included(v.into()),
                Bound::Excluded(v) => Tstzbound::Excluded(v.into()),
                Bound::Unbounded => Tstzbound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl From<postgres_types::ValuesRange<chrono::NaiveDateTime>> for Tsrange {
    fn from(value: postgres_types::ValuesRange<chrono::NaiveDateTime>) -> Self {
        fn to_bounds(v: Bound<chrono::NaiveDateTime>) -> Tsbound {
            match v {
                Bound::Included(v) => Tsbound::Included(v.into()),
                Bound::Excluded(v) => Tsbound::Excluded(v.into()),
                Bound::Unbounded => Tsbound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

impl From<postgres_types::ValuesRange<chrono::NaiveDate>> for Daterange {
    fn from(value: postgres_types::ValuesRange<chrono::NaiveDate>) -> Self {
        fn to_bounds(v: Bound<chrono::NaiveDate>) -> Datebound {
            match v {
                Bound::Included(v) => Datebound::Included(v.into()),
                Bound::Excluded(v) => Datebound::Excluded(v.into()),
                Bound::Unbounded => Datebound::Unbounded,
            }
        }
        Self {
            start: to_bounds(value.start),
            end: to_bounds(value.end),
        }
    }
}

pub(crate) mod postgres_utils {
    use crate::preview2::wasi::rdbms::postgres::{
        Composite, CompositeType, DbColumnType, DbColumnTypeNode, DbValue, DbValueNode, Domain,
        DomainType, MacAddress, NodeIndex, Uuid,
    };
    use crate::services::rdbms::postgres::types as postgres_types;
    use sqlx::types::BitVec;
    use std::str::FromStr;

    pub(crate) fn to_db_value(value: DbValue) -> Result<postgres_types::DbValue, String> {
        make_db_value(0, &value.nodes)
    }

    fn make_db_value(
        index: NodeIndex,
        nodes: &[DbValueNode],
    ) -> Result<postgres_types::DbValue, String> {
        if index as usize >= nodes.len() {
            Err(format!("Index ({}) out of range", index))
        } else {
            let node = &nodes[index as usize];
            match node {
                DbValueNode::Character(v) => Ok(postgres_types::DbValue::Character(*v)),
                DbValueNode::Int2(i) => Ok(postgres_types::DbValue::Int2(*i)),
                DbValueNode::Int4(i) => Ok(postgres_types::DbValue::Int4(*i)),
                DbValueNode::Int8(i) => Ok(postgres_types::DbValue::Int8(*i)),
                DbValueNode::Numeric(s) => {
                    let v = bigdecimal::BigDecimal::from_str(s).map_err(|e| e.to_string())?;
                    Ok(postgres_types::DbValue::Numeric(v))
                }
                DbValueNode::Float4(f) => Ok(postgres_types::DbValue::Float4(*f)),
                DbValueNode::Float8(f) => Ok(postgres_types::DbValue::Float8(*f)),
                DbValueNode::Boolean(b) => Ok(postgres_types::DbValue::Boolean(*b)),
                DbValueNode::Timestamp(v) => {
                    let value = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Timestamp(value))
                }
                DbValueNode::Timestamptz(v) => {
                    let value = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Timestamptz(value))
                }
                DbValueNode::Time(v) => {
                    let value = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Time(value))
                }
                DbValueNode::Timetz(v) => {
                    let value = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Timetz(value))
                }
                DbValueNode::Date(v) => {
                    let value = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Date(value))
                }
                DbValueNode::Interval(v) => Ok(postgres_types::DbValue::Interval((*v).into())),
                DbValueNode::Text(s) => Ok(postgres_types::DbValue::Text(s.clone())),
                DbValueNode::Varchar(s) => Ok(postgres_types::DbValue::Varchar(s.clone())),
                DbValueNode::Bpchar(s) => Ok(postgres_types::DbValue::Bpchar(s.clone())),
                DbValueNode::Bytea(u) => Ok(postgres_types::DbValue::Bytea(u.clone())),
                DbValueNode::Json(v) => {
                    let v: serde_json::Value =
                        serde_json::from_str(v).map_err(|e| e.to_string())?;
                    Ok(postgres_types::DbValue::Json(v))
                }
                DbValueNode::Jsonb(v) => {
                    let v: serde_json::Value =
                        serde_json::from_str(v).map_err(|e| e.to_string())?;
                    Ok(postgres_types::DbValue::Jsonb(v))
                }
                DbValueNode::Jsonpath(s) => Ok(postgres_types::DbValue::Jsonpath(s.clone())),
                DbValueNode::Xml(s) => Ok(postgres_types::DbValue::Xml(s.clone())),
                DbValueNode::Uuid(v) => Ok(postgres_types::DbValue::Uuid(
                    uuid::Uuid::from_u64_pair(v.high_bits, v.low_bits),
                )),
                DbValueNode::Bit(v) => {
                    Ok(postgres_types::DbValue::Bit(BitVec::from_iter(v.clone())))
                }
                DbValueNode::Varbit(v) => Ok(postgres_types::DbValue::Varbit(BitVec::from_iter(
                    v.clone(),
                ))),
                DbValueNode::Oid(v) => Ok(postgres_types::DbValue::Oid(*v)),
                DbValueNode::Inet(v) => Ok(postgres_types::DbValue::Inet((*v).into())),
                DbValueNode::Cidr(v) => Ok(postgres_types::DbValue::Cidr((*v).into())),
                DbValueNode::Macaddr(v) => Ok(postgres_types::DbValue::Macaddr(
                    sqlx::types::mac_address::MacAddress::new(v.octets.into()),
                )),
                DbValueNode::Int4range(v) => Ok(postgres_types::DbValue::Int4range((*v).into())),
                DbValueNode::Int8range(v) => Ok(postgres_types::DbValue::Int8range((*v).into())),
                DbValueNode::Numrange(v) => {
                    let v = v.clone().try_into()?;
                    Ok(postgres_types::DbValue::Numrange(v))
                }
                DbValueNode::Tsrange(v) => {
                    let v = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Tsrange(v))
                }
                DbValueNode::Tstzrange(v) => {
                    let v = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Tstzrange(v))
                }
                DbValueNode::Daterange(v) => {
                    let v = (*v).try_into()?;
                    Ok(postgres_types::DbValue::Daterange(v))
                }
                DbValueNode::Money(v) => Ok(postgres_types::DbValue::Money(*v)),
                DbValueNode::Enumeration(v) => Ok(postgres_types::DbValue::Enum(v.clone().into())),
                DbValueNode::Array(vs) => {
                    let mut values: Vec<postgres_types::DbValue> = Vec::with_capacity(vs.len());
                    for i in vs.iter() {
                        let v = make_db_value(*i, nodes)?;
                        values.push(v);
                    }
                    Ok(postgres_types::DbValue::Array(values))
                }
                DbValueNode::Composite(v) => {
                    let mut values: Vec<postgres_types::DbValue> =
                        Vec::with_capacity(v.values.len());
                    for i in v.values.iter() {
                        let v = make_db_value(*i, nodes)?;
                        values.push(v);
                    }
                    Ok(postgres_types::DbValue::Composite(
                        postgres_types::Composite::new(v.name.clone(), values),
                    ))
                }
                DbValueNode::Domain(v) => {
                    let value = make_db_value(v.value, nodes)?;
                    Ok(postgres_types::DbValue::Domain(
                        postgres_types::Domain::new(v.name.clone(), value),
                    ))
                }
                DbValueNode::Null => Ok(postgres_types::DbValue::Null),
            }
        }
    }

    pub(crate) fn from_db_value(value: postgres_types::DbValue) -> DbValue {
        let mut builder = DbValueBuilder::new();
        builder.add(value);
        builder.build()
    }

    struct DbValueBuilder {
        nodes: Vec<DbValueNode>,
        offset: NodeIndex,
    }

    impl DbValueBuilder {
        fn new() -> Self {
            Self::new_with_offset(0)
        }

        fn new_with_offset(offset: NodeIndex) -> Self {
            Self {
                nodes: Vec::new(),
                offset,
            }
        }

        fn add_nodes(&mut self, node: DbValueNode, child_nodes: Vec<DbValueNode>) -> NodeIndex {
            let index = self.add_node(node);
            self.nodes.extend(child_nodes);
            index
        }

        fn add_node(&mut self, node: DbValueNode) -> NodeIndex {
            self.nodes.push(node);
            self.nodes.len() as NodeIndex - 1 + self.offset
        }

        fn child_builder(&self) -> DbValueBuilder {
            let offset = self.nodes.len() as NodeIndex + 1 + self.offset;
            DbValueBuilder::new_with_offset(offset)
        }

        fn add(&mut self, value: postgres_types::DbValue) -> NodeIndex {
            match value {
                postgres_types::DbValue::Character(s) => self.add_node(DbValueNode::Character(s)),
                postgres_types::DbValue::Int2(i) => self.add_node(DbValueNode::Int2(i)),
                postgres_types::DbValue::Int4(i) => self.add_node(DbValueNode::Int4(i)),
                postgres_types::DbValue::Int8(i) => self.add_node(DbValueNode::Int8(i)),
                postgres_types::DbValue::Numeric(s) => {
                    self.add_node(DbValueNode::Numeric(s.to_string()))
                }
                postgres_types::DbValue::Float4(f) => self.add_node(DbValueNode::Float4(f)),
                postgres_types::DbValue::Float8(f) => self.add_node(DbValueNode::Float8(f)),
                postgres_types::DbValue::Boolean(b) => self.add_node(DbValueNode::Boolean(b)),
                postgres_types::DbValue::Timestamp(v) => {
                    self.add_node(DbValueNode::Timestamp(v.into()))
                }
                postgres_types::DbValue::Timestamptz(v) => {
                    self.add_node(DbValueNode::Timestamptz(v.into()))
                }
                postgres_types::DbValue::Time(v) => self.add_node(DbValueNode::Time(v.into())),
                postgres_types::DbValue::Timetz(v) => self.add_node(DbValueNode::Timetz(v.into())),
                postgres_types::DbValue::Date(v) => self.add_node(DbValueNode::Date(v.into())),
                postgres_types::DbValue::Interval(v) => {
                    self.add_node(DbValueNode::Interval(v.into()))
                }
                postgres_types::DbValue::Text(s) => self.add_node(DbValueNode::Text(s)),
                postgres_types::DbValue::Varchar(s) => self.add_node(DbValueNode::Varchar(s)),
                postgres_types::DbValue::Bpchar(s) => self.add_node(DbValueNode::Bpchar(s)),
                postgres_types::DbValue::Bytea(u) => self.add_node(DbValueNode::Bytea(u)),
                postgres_types::DbValue::Json(s) => self.add_node(DbValueNode::Json(s.to_string())),
                postgres_types::DbValue::Jsonb(s) => {
                    self.add_node(DbValueNode::Jsonb(s.to_string()))
                }
                postgres_types::DbValue::Jsonpath(s) => self.add_node(DbValueNode::Jsonpath(s)),
                postgres_types::DbValue::Xml(s) => self.add_node(DbValueNode::Xml(s)),
                postgres_types::DbValue::Uuid(uuid) => {
                    let (high_bits, low_bits) = uuid.as_u64_pair();
                    self.add_node(DbValueNode::Uuid(Uuid {
                        high_bits,
                        low_bits,
                    }))
                }
                postgres_types::DbValue::Bit(v) => {
                    self.add_node(DbValueNode::Bit(v.iter().collect()))
                }
                postgres_types::DbValue::Varbit(v) => {
                    self.add_node(DbValueNode::Varbit(v.iter().collect()))
                }
                postgres_types::DbValue::Inet(v) => self.add_node(DbValueNode::Inet(v.into())),
                postgres_types::DbValue::Cidr(v) => self.add_node(DbValueNode::Cidr(v.into())),
                postgres_types::DbValue::Macaddr(v) => {
                    let v = v.bytes();
                    self.add_node(DbValueNode::Macaddr(MacAddress { octets: v.into() }))
                }
                postgres_types::DbValue::Tsrange(v) => {
                    self.add_node(DbValueNode::Tsrange(v.into()))
                }
                postgres_types::DbValue::Tstzrange(v) => {
                    self.add_node(DbValueNode::Tstzrange(v.into()))
                }
                postgres_types::DbValue::Daterange(v) => {
                    self.add_node(DbValueNode::Daterange(v.into()))
                }
                postgres_types::DbValue::Int4range(v) => {
                    self.add_node(DbValueNode::Int4range(v.into()))
                }
                postgres_types::DbValue::Int8range(v) => {
                    self.add_node(DbValueNode::Int8range(v.into()))
                }
                postgres_types::DbValue::Numrange(v) => {
                    self.add_node(DbValueNode::Numrange(v.into()))
                }
                postgres_types::DbValue::Oid(v) => self.add_node(DbValueNode::Oid(v)),
                postgres_types::DbValue::Money(v) => self.add_node(DbValueNode::Money(v)),
                postgres_types::DbValue::Enum(v) => {
                    self.add_node(DbValueNode::Enumeration(v.into()))
                }
                postgres_types::DbValue::Composite(v) => {
                    let mut child_builder = self.child_builder();
                    let mut values: Vec<NodeIndex> = Vec::with_capacity(v.values.len());
                    for v in v.values {
                        let i = child_builder.add(v);
                        values.push(i);
                    }
                    self.add_nodes(
                        DbValueNode::Composite(Composite {
                            name: v.name,
                            values,
                        }),
                        child_builder.nodes,
                    )
                }
                postgres_types::DbValue::Domain(v) => {
                    let mut child_builder = self.child_builder();
                    let value_node_index = child_builder.add(*v.value);
                    self.add_nodes(
                        DbValueNode::Domain(Domain {
                            name: v.name,
                            value: value_node_index,
                        }),
                        child_builder.nodes,
                    )
                }
                postgres_types::DbValue::Array(vs) => {
                    let mut child_builder = self.child_builder();
                    let mut values: Vec<NodeIndex> = Vec::with_capacity(vs.len());
                    for v in vs {
                        let i = child_builder.add(v);
                        values.push(i);
                    }
                    self.add_nodes(DbValueNode::Array(values), child_builder.nodes)
                }
                postgres_types::DbValue::Null => self.add_node(DbValueNode::Null),
            }
        }

        fn build(self) -> DbValue {
            DbValue { nodes: self.nodes }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn to_db_column_type(
        value: DbColumnType,
    ) -> Result<postgres_types::DbColumnType, String> {
        make_db_column_type(0, &value.nodes)
    }

    fn make_db_column_type(
        index: NodeIndex,
        nodes: &[DbColumnTypeNode],
    ) -> Result<postgres_types::DbColumnType, String> {
        if index as usize >= nodes.len() {
            Err(format!("Index ({}) out of range", index))
        } else {
            let node = &nodes[index as usize];
            match node {
                DbColumnTypeNode::Character => Ok(postgres_types::DbColumnType::Character),
                DbColumnTypeNode::Int2 => Ok(postgres_types::DbColumnType::Int2),
                DbColumnTypeNode::Int4 => Ok(postgres_types::DbColumnType::Int4),
                DbColumnTypeNode::Int8 => Ok(postgres_types::DbColumnType::Int8),
                DbColumnTypeNode::Numeric => Ok(postgres_types::DbColumnType::Numeric),
                DbColumnTypeNode::Float4 => Ok(postgres_types::DbColumnType::Float4),
                DbColumnTypeNode::Float8 => Ok(postgres_types::DbColumnType::Float8),
                DbColumnTypeNode::Boolean => Ok(postgres_types::DbColumnType::Boolean),
                DbColumnTypeNode::Timestamp => Ok(postgres_types::DbColumnType::Timestamp),
                DbColumnTypeNode::Timestamptz => Ok(postgres_types::DbColumnType::Timestamptz),
                DbColumnTypeNode::Time => Ok(postgres_types::DbColumnType::Time),
                DbColumnTypeNode::Timetz => Ok(postgres_types::DbColumnType::Timetz),
                DbColumnTypeNode::Date => Ok(postgres_types::DbColumnType::Date),
                DbColumnTypeNode::Interval => Ok(postgres_types::DbColumnType::Interval),
                DbColumnTypeNode::Bytea => Ok(postgres_types::DbColumnType::Bytea),
                DbColumnTypeNode::Text => Ok(postgres_types::DbColumnType::Text),
                DbColumnTypeNode::Varchar => Ok(postgres_types::DbColumnType::Varchar),
                DbColumnTypeNode::Bpchar => Ok(postgres_types::DbColumnType::Bpchar),
                DbColumnTypeNode::Json => Ok(postgres_types::DbColumnType::Json),
                DbColumnTypeNode::Jsonb => Ok(postgres_types::DbColumnType::Jsonb),
                DbColumnTypeNode::Jsonpath => Ok(postgres_types::DbColumnType::Jsonpath),
                DbColumnTypeNode::Uuid => Ok(postgres_types::DbColumnType::Uuid),
                DbColumnTypeNode::Xml => Ok(postgres_types::DbColumnType::Xml),
                DbColumnTypeNode::Bit => Ok(postgres_types::DbColumnType::Bit),
                DbColumnTypeNode::Varbit => Ok(postgres_types::DbColumnType::Varbit),
                DbColumnTypeNode::Inet => Ok(postgres_types::DbColumnType::Inet),
                DbColumnTypeNode::Cidr => Ok(postgres_types::DbColumnType::Cidr),
                DbColumnTypeNode::Macaddr => Ok(postgres_types::DbColumnType::Macaddr),
                DbColumnTypeNode::Tsrange => Ok(postgres_types::DbColumnType::Tsrange),
                DbColumnTypeNode::Tstzrange => Ok(postgres_types::DbColumnType::Tstzrange),
                DbColumnTypeNode::Daterange => Ok(postgres_types::DbColumnType::Daterange),
                DbColumnTypeNode::Int4range => Ok(postgres_types::DbColumnType::Int4range),
                DbColumnTypeNode::Int8range => Ok(postgres_types::DbColumnType::Int8range),
                DbColumnTypeNode::Numrange => Ok(postgres_types::DbColumnType::Numrange),
                DbColumnTypeNode::Oid => Ok(postgres_types::DbColumnType::Oid),
                DbColumnTypeNode::Money => Ok(postgres_types::DbColumnType::Money),
                DbColumnTypeNode::Enumeration(v) => {
                    Ok(postgres_types::DbColumnType::Enum(v.clone().into()))
                }
                DbColumnTypeNode::Composite(v) => {
                    let mut attributes: Vec<(String, postgres_types::DbColumnType)> =
                        Vec::with_capacity(v.attributes.len());
                    for (n, i) in v.attributes.iter() {
                        let v = make_db_column_type(*i, nodes)?;
                        attributes.push((n.clone(), v));
                    }

                    Ok(postgres_types::DbColumnType::Composite(
                        postgres_types::CompositeType::new(v.name.clone(), attributes),
                    ))
                }
                DbColumnTypeNode::Domain(v) => {
                    let t = make_db_column_type(v.base_type, nodes)?;
                    Ok(postgres_types::DbColumnType::Domain(
                        postgres_types::DomainType::new(v.name.clone(), t),
                    ))
                }
                DbColumnTypeNode::Array(v) => {
                    let t = make_db_column_type(*v, nodes)?;
                    Ok(postgres_types::DbColumnType::Array(Box::new(t)))
                }
            }
        }
    }

    pub(crate) fn from_db_column_type(value: postgres_types::DbColumnType) -> DbColumnType {
        let mut builder = DbColumnTypeBuilder::new();
        builder.add(value);
        builder.build()
    }

    struct DbColumnTypeBuilder {
        nodes: Vec<DbColumnTypeNode>,
        offset: NodeIndex,
    }

    impl DbColumnTypeBuilder {
        fn new() -> Self {
            Self::new_with_offset(0)
        }

        fn new_with_offset(offset: NodeIndex) -> Self {
            Self {
                nodes: Vec::new(),
                offset,
            }
        }

        fn add_nodes(
            &mut self,
            node: DbColumnTypeNode,
            child_nodes: Vec<DbColumnTypeNode>,
        ) -> NodeIndex {
            let index = self.add_node(node);
            self.nodes.extend(child_nodes);
            index
        }

        fn add_node(&mut self, node: DbColumnTypeNode) -> NodeIndex {
            self.nodes.push(node);
            self.nodes.len() as NodeIndex - 1 + self.offset
        }

        fn child_builder(&self) -> DbColumnTypeBuilder {
            let offset = self.nodes.len() as NodeIndex + 1 + self.offset;
            DbColumnTypeBuilder::new_with_offset(offset)
        }

        fn add(&mut self, value: postgres_types::DbColumnType) -> NodeIndex {
            match value {
                postgres_types::DbColumnType::Character => {
                    self.add_node(DbColumnTypeNode::Character)
                }
                postgres_types::DbColumnType::Int2 => self.add_node(DbColumnTypeNode::Int2),
                postgres_types::DbColumnType::Int4 => self.add_node(DbColumnTypeNode::Int4),
                postgres_types::DbColumnType::Int8 => self.add_node(DbColumnTypeNode::Int8),
                postgres_types::DbColumnType::Numeric => self.add_node(DbColumnTypeNode::Numeric),
                postgres_types::DbColumnType::Float4 => self.add_node(DbColumnTypeNode::Float4),
                postgres_types::DbColumnType::Float8 => self.add_node(DbColumnTypeNode::Float8),
                postgres_types::DbColumnType::Boolean => self.add_node(DbColumnTypeNode::Boolean),
                postgres_types::DbColumnType::Timestamp => {
                    self.add_node(DbColumnTypeNode::Timestamp)
                }
                postgres_types::DbColumnType::Timestamptz => {
                    self.add_node(DbColumnTypeNode::Timestamptz)
                }
                postgres_types::DbColumnType::Time => self.add_node(DbColumnTypeNode::Time),
                postgres_types::DbColumnType::Timetz => self.add_node(DbColumnTypeNode::Timetz),
                postgres_types::DbColumnType::Date => self.add_node(DbColumnTypeNode::Date),
                postgres_types::DbColumnType::Interval => self.add_node(DbColumnTypeNode::Interval),
                postgres_types::DbColumnType::Text => self.add_node(DbColumnTypeNode::Text),
                postgres_types::DbColumnType::Varchar => self.add_node(DbColumnTypeNode::Varchar),
                postgres_types::DbColumnType::Bpchar => self.add_node(DbColumnTypeNode::Bpchar),
                postgres_types::DbColumnType::Bytea => self.add_node(DbColumnTypeNode::Bytea),
                postgres_types::DbColumnType::Json => self.add_node(DbColumnTypeNode::Json),
                postgres_types::DbColumnType::Jsonb => self.add_node(DbColumnTypeNode::Jsonb),
                postgres_types::DbColumnType::Jsonpath => self.add_node(DbColumnTypeNode::Jsonpath),
                postgres_types::DbColumnType::Xml => self.add_node(DbColumnTypeNode::Xml),
                postgres_types::DbColumnType::Uuid => self.add_node(DbColumnTypeNode::Uuid),
                postgres_types::DbColumnType::Bit => self.add_node(DbColumnTypeNode::Bit),
                postgres_types::DbColumnType::Varbit => self.add_node(DbColumnTypeNode::Varbit),
                postgres_types::DbColumnType::Inet => self.add_node(DbColumnTypeNode::Inet),
                postgres_types::DbColumnType::Cidr => self.add_node(DbColumnTypeNode::Cidr),
                postgres_types::DbColumnType::Macaddr => self.add_node(DbColumnTypeNode::Macaddr),
                postgres_types::DbColumnType::Tsrange => self.add_node(DbColumnTypeNode::Tsrange),
                postgres_types::DbColumnType::Tstzrange => {
                    self.add_node(DbColumnTypeNode::Tstzrange)
                }
                postgres_types::DbColumnType::Daterange => {
                    self.add_node(DbColumnTypeNode::Daterange)
                }
                postgres_types::DbColumnType::Int4range => {
                    self.add_node(DbColumnTypeNode::Int4range)
                }
                postgres_types::DbColumnType::Int8range => {
                    self.add_node(DbColumnTypeNode::Int8range)
                }
                postgres_types::DbColumnType::Numrange => self.add_node(DbColumnTypeNode::Numrange),
                postgres_types::DbColumnType::Oid => self.add_node(DbColumnTypeNode::Oid),
                postgres_types::DbColumnType::Money => self.add_node(DbColumnTypeNode::Money),
                postgres_types::DbColumnType::Enum(v) => {
                    self.add_node(DbColumnTypeNode::Enumeration(v.into()))
                }
                postgres_types::DbColumnType::Composite(v) => {
                    let mut attributes: Vec<(String, NodeIndex)> =
                        Vec::with_capacity(v.attributes.len());
                    let mut child_builder = self.child_builder();
                    for (n, v) in v.attributes {
                        let i = child_builder.add(v);
                        attributes.push((n, i));
                    }
                    self.add_nodes(
                        DbColumnTypeNode::Composite(CompositeType {
                            name: v.name,
                            attributes,
                        }),
                        child_builder.nodes,
                    )
                }
                postgres_types::DbColumnType::Domain(v) => {
                    let mut child_builder = self.child_builder();
                    let value_node_index = child_builder.add(*v.base_type);
                    self.add_nodes(
                        DbColumnTypeNode::Domain(DomainType {
                            name: v.name,
                            base_type: value_node_index,
                        }),
                        child_builder.nodes,
                    )
                }
                postgres_types::DbColumnType::Array(v) => {
                    let mut child_builder = self.child_builder();
                    let value_node_index = child_builder.add(*v);
                    self.add_nodes(
                        DbColumnTypeNode::Array(value_node_index),
                        child_builder.nodes,
                    )
                }
            }
        }

        fn build(self) -> DbColumnType {
            DbColumnType { nodes: self.nodes }
        }
    }

    #[cfg(test)]
    pub mod tests {
        use crate::durable_host::rdbms::postgres::postgres_utils;
        use crate::services::rdbms::postgres::types as postgres_types;
        use assert2::check;
        use bigdecimal::BigDecimal;
        use chrono::Offset;
        use serde_json::json;
        use sqlx::types::mac_address::MacAddress;
        use sqlx::types::BitVec;
        use std::collections::Bound;
        use std::net::{IpAddr, Ipv4Addr};
        use test_r::test;
        use uuid::Uuid;

        fn check_db_value(value: postgres_types::DbValue) {
            let wit = postgres_utils::from_db_value(value.clone());

            // println!("wit {:?}", wit);
            let value2 = postgres_utils::to_db_value(wit).unwrap();

            check!(value == value2);
        }

        #[test]
        fn test_db_values_conversions() {
            let tstzbounds = postgres_types::ValuesRange::new(
                Bound::Included(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 3, 2).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                Bound::Excluded(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
            );
            let tsbounds = postgres_types::ValuesRange::new(
                Bound::Included(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2022, 2, 2).unwrap(),
                    chrono::NaiveTime::from_hms_opt(16, 50, 30).unwrap(),
                )),
                Bound::Excluded(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )),
            );

            let params = vec![
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Enum(postgres_types::Enum::new(
                        "a_test_enum".to_string(),
                        "second".to_string(),
                    )),
                    postgres_types::DbValue::Enum(postgres_types::Enum::new(
                        "a_test_enum".to_string(),
                        "third".to_string(),
                    )),
                ]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Character(2)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int2(1)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int4(2)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int8(3)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Float4(4.0)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Float8(5.0)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Numeric(
                    BigDecimal::from(48888),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Boolean(true)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                    "text".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Varchar(
                    "varchar".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bpchar(
                    "0123456789".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timestamp(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timestamptz(
                    chrono::DateTime::from_naive_utc_and_offset(
                        chrono::NaiveDateTime::new(
                            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                            chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                        ),
                        chrono::Utc,
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Date(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Time(
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timetz(
                    postgres_types::TimeTz::new(
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                        chrono::Utc.fix(),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Interval(
                    postgres_types::Interval::new(10, 20, 30),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bytea(
                    "bytea".as_bytes().to_vec(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Uuid(Uuid::new_v4())]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Json(json!(
                       {
                          "id": 2
                       }
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Jsonb(json!(
                       {
                          "index": 4
                       }
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Inet(IpAddr::V4(
                    Ipv4Addr::new(127, 0, 0, 1),
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Cidr(IpAddr::V4(
                    Ipv4Addr::new(198, 168, 0, 0),
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Macaddr(
                    MacAddress::new([0, 1, 2, 3, 4, 1]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bit(
                    BitVec::from_iter(vec![true, false, true]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Varbit(
                    BitVec::from_iter(vec![true, false, false]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Xml(
                    "<foo>200</foo>".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int4range(
                    postgres_types::ValuesRange::new(Bound::Included(1), Bound::Excluded(4)),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int8range(
                    postgres_types::ValuesRange::new(Bound::Included(1), Bound::Unbounded),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Numrange(
                    postgres_types::ValuesRange::new(
                        Bound::Included(BigDecimal::from(11)),
                        Bound::Excluded(BigDecimal::from(221)),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Tsrange(tsbounds)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Tstzrange(
                    tstzbounds,
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Daterange(
                    postgres_types::ValuesRange::new(
                        Bound::Included(chrono::NaiveDate::from_ymd_opt(2023, 2, 3).unwrap()),
                        Bound::Unbounded,
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Money(1234)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Jsonpath(
                    "$.user.addresses[0].city".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                    "'a' 'and' 'ate' 'cat' 'fat' 'mat' 'on' 'rat' 'sat'".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                    "'fat' & 'rat' & !'cat'".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "a_inventory_item".to_string(),
                        vec![
                            postgres_types::DbValue::Uuid(Uuid::new_v4()),
                            postgres_types::DbValue::Text("text".to_string()),
                            postgres_types::DbValue::Int4(3),
                            postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                        ],
                    )),
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "a_inventory_item".to_string(),
                        vec![
                            postgres_types::DbValue::Uuid(Uuid::new_v4()),
                            postgres_types::DbValue::Text("text".to_string()),
                            postgres_types::DbValue::Int4(4),
                            postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                        ],
                    )),
                ]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Domain(postgres_types::Domain::new(
                        "posint8".to_string(),
                        postgres_types::DbValue::Int8(1),
                    )),
                    postgres_types::DbValue::Domain(postgres_types::Domain::new(
                        "posint8".to_string(),
                        postgres_types::DbValue::Int8(2),
                    )),
                ]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "ccc".to_string(),
                        vec![
                            postgres_types::DbValue::Varchar("v1".to_string()),
                            postgres_types::DbValue::Int2(1),
                            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Domain(
                                postgres_types::Domain::new(
                                    "ddd".to_string(),
                                    postgres_types::DbValue::Varchar("v11".to_string()),
                                ),
                            )]),
                        ],
                    )),
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "ccc".to_string(),
                        vec![
                            postgres_types::DbValue::Varchar("v2".to_string()),
                            postgres_types::DbValue::Int2(2),
                            postgres_types::DbValue::Array(vec![
                                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                    "ddd".to_string(),
                                    postgres_types::DbValue::Varchar("v21".to_string()),
                                )),
                                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                    "ddd".to_string(),
                                    postgres_types::DbValue::Varchar("v22".to_string()),
                                )),
                            ]),
                        ],
                    )),
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "ccc".to_string(),
                        vec![
                            postgres_types::DbValue::Varchar("v3".to_string()),
                            postgres_types::DbValue::Int2(3),
                            postgres_types::DbValue::Array(vec![
                                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                    "ddd".to_string(),
                                    postgres_types::DbValue::Varchar("v31".to_string()),
                                )),
                                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                    "ddd".to_string(),
                                    postgres_types::DbValue::Varchar("v32".to_string()),
                                )),
                                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                    "ddd".to_string(),
                                    postgres_types::DbValue::Varchar("v33".to_string()),
                                )),
                            ]),
                        ],
                    )),
                ]),
                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                    "ddd".to_string(),
                    postgres_types::DbValue::Varchar("tag2".to_string()),
                )),
            ];

            for param in params {
                check_db_value(param);
            }
        }

        fn check_db_column_type(value: postgres_types::DbColumnType) {
            let wit = postgres_utils::from_db_column_type(value.clone());

            // println!("wit {:?}", wit);
            let value2 = postgres_utils::to_db_column_type(wit).unwrap();

            check!(value == value2);
        }

        #[test]
        fn test_db_column_types_conversions() {
            let value =
                postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
                    "inventory_item".to_string(),
                    vec![
                        ("product_id".to_string(), postgres_types::DbColumnType::Uuid),
                        ("name".to_string(), postgres_types::DbColumnType::Text),
                        (
                            "supplier_id".to_string(),
                            postgres_types::DbColumnType::Int4,
                        ),
                        ("price".to_string(), postgres_types::DbColumnType::Numeric),
                    ],
                ));
            check_db_column_type(value.clone());

            check_db_column_type(value.clone().into_array());

            let value = postgres_types::DbColumnType::Domain(postgres_types::DomainType::new(
                "posint8".to_string(),
                postgres_types::DbColumnType::Int8,
            ));

            check_db_column_type(value.clone());

            check_db_column_type(value.clone().into_array());
        }
    }
}
