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

use crate::durable_host::rdbms::{
    begin_db_transaction, db_connection_drop, db_connection_durable_execute,
    db_connection_durable_query, db_connection_durable_query_stream, db_result_stream_drop,
    db_result_stream_durable_get_columns, db_result_stream_durable_get_next, db_transaction_drop,
    db_transaction_durable_commit, db_transaction_durable_execute, db_transaction_durable_query,
    db_transaction_durable_query_stream, db_transaction_durable_rollback, open_db_connection,
    FromRdbmsValue, RdbmsConnection, RdbmsResultStreamEntry, RdbmsTransactionEntry,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::rdbms::postgres::{
    Composite, CompositeType, Datebound, Daterange, DbColumn, DbColumnType, DbResult, DbRow,
    DbValue, Domain, DomainType, Enumeration, EnumerationType, Error, Host, HostDbConnection,
    HostDbResultStream, HostDbTransaction, HostLazyDbColumnType, HostLazyDbValue, Int4bound,
    Int4range, Int8bound, Int8range, Interval, Numbound, Numrange, Range, RangeType, Tsbound,
    Tsrange, Tstzbound, Tstzrange, ValueBound, ValuesRange,
};
use crate::preview2::golem::rdbms::types::Timetz;
use crate::services::rdbms::postgres::types as postgres_types;
use crate::services::rdbms::postgres::PostgresType;
use crate::workerctx::WorkerCtx;
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use std::ops::Bound;
use std::str::FromStr;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::IoView;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub type PostgresDbConnection = RdbmsConnection<PostgresType>;

impl<Ctx: WorkerCtx> HostDbConnection for DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<PostgresDbConnection>, Error>> {
        open_db_connection(address, self).await
    }

    async fn query_stream(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultStreamEntry>, Error>> {
        db_connection_durable_query_stream(statement, params, self, &self_).await
    }

    async fn query(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<DbResult, Error>> {
        db_connection_durable_query(statement, params, self, &self_).await
    }

    async fn execute(
        &mut self,
        self_: Resource<PostgresDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        db_connection_durable_execute(statement, params, self, &self_).await
    }

    async fn begin_transaction(
        &mut self,
        self_: Resource<PostgresDbConnection>,
    ) -> anyhow::Result<Result<Resource<DbTransactionEntry>, Error>> {
        begin_db_transaction(self, &self_).await
    }

    async fn drop(&mut self, rep: Resource<PostgresDbConnection>) -> anyhow::Result<()> {
        db_connection_drop(self, rep).await
    }
}

pub type DbResultStreamEntry = RdbmsResultStreamEntry<PostgresType>;

impl<Ctx: WorkerCtx> HostDbResultStream for DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<DbResultStreamEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        db_result_stream_durable_get_columns(self, &self_).await
    }

    async fn get_next(
        &mut self,
        self_: Resource<DbResultStreamEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        db_result_stream_durable_get_next(self, &self_).await
    }

    async fn drop(&mut self, rep: Resource<DbResultStreamEntry>) -> anyhow::Result<()> {
        db_result_stream_drop(self, rep).await
    }
}

pub type DbTransactionEntry = RdbmsTransactionEntry<PostgresType>;

impl<Ctx: WorkerCtx> HostDbTransaction for DurableWorkerCtx<Ctx> {
    async fn query(
        &mut self,
        self_: Resource<DbTransactionEntry>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<DbResult, Error>> {
        db_transaction_durable_query(statement, params, self, &self_).await
    }

    async fn query_stream(
        &mut self,
        self_: Resource<DbTransactionEntry>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultStreamEntry>, Error>> {
        db_transaction_durable_query_stream(statement, params, self, &self_).await
    }

    async fn execute(
        &mut self,
        self_: Resource<DbTransactionEntry>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        db_transaction_durable_execute(statement, params, self, &self_).await
    }

    async fn commit(
        &mut self,
        self_: Resource<DbTransactionEntry>,
    ) -> anyhow::Result<Result<(), Error>> {
        db_transaction_durable_commit(self, &self_).await
    }

    async fn rollback(
        &mut self,
        self_: Resource<DbTransactionEntry>,
    ) -> anyhow::Result<Result<(), Error>> {
        db_transaction_durable_rollback(self, &self_).await
    }

    async fn drop(&mut self, rep: Resource<DbTransactionEntry>) -> anyhow::Result<()> {
        db_transaction_drop(self, rep).await
    }
}

pub struct LazyDbColumnTypeEntry {
    value: DbColumnTypeWithResourceRep,
}

impl LazyDbColumnTypeEntry {
    fn new(value: DbColumnTypeWithResourceRep) -> Self {
        Self { value }
    }
}

impl<Ctx: WorkerCtx> HostLazyDbColumnType for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        value: DbColumnType,
    ) -> anyhow::Result<Resource<LazyDbColumnTypeEntry>> {
        self.observe_function_call("rdbms::postgres::lazy-db-column-type", "new");
        let value = to_db_column_type(value, self.as_wasi_view().table()).map_err(Error::Other)?;
        let result = self
            .as_wasi_view()
            .table()
            .push(LazyDbColumnTypeEntry::new(value))?;
        Ok(result)
    }

    async fn get(
        &mut self,
        self_: Resource<LazyDbColumnTypeEntry>,
    ) -> anyhow::Result<DbColumnType> {
        self.observe_function_call("rdbms::postgres::lazy-db-column-type", "get");
        let value = self
            .as_wasi_view()
            .table()
            .get::<LazyDbColumnTypeEntry>(&self_)?
            .value
            .clone();
        let current_resource_rep = value.resource_rep.clone();
        let (result, new_resource_rep) =
            from_db_column_type(value, self.as_wasi_view().table()).map_err(Error::Other)?;
        if new_resource_rep != current_resource_rep {
            self.as_wasi_view()
                .table()
                .get_mut::<LazyDbColumnTypeEntry>(&self_)?
                .value
                .update_resource_rep(new_resource_rep)
                .map_err(Error::Other)?;
        }
        Ok(result)
    }

    async fn drop(&mut self, rep: Resource<LazyDbColumnTypeEntry>) -> anyhow::Result<()> {
        self.observe_function_call("rdbms::postgres::lazy-db-column-type", "drop");
        self.as_wasi_view()
            .table()
            .delete::<LazyDbColumnTypeEntry>(rep)?;
        Ok(())
    }
}

pub struct LazyDbValueEntry {
    value: DbValueWithResourceRep,
}

impl LazyDbValueEntry {
    fn new(value: DbValueWithResourceRep) -> Self {
        Self { value }
    }
}

impl<Ctx: WorkerCtx> HostLazyDbValue for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, value: DbValue) -> anyhow::Result<Resource<LazyDbValueEntry>> {
        self.observe_function_call("rdbms::postgres::lazy-db-value", "new");
        let value = to_db_value(value, self.as_wasi_view().table()).map_err(Error::Other)?;
        let result = self
            .as_wasi_view()
            .table()
            .push(LazyDbValueEntry::new(value))?;
        Ok(result)
    }

    async fn get(&mut self, self_: Resource<LazyDbValueEntry>) -> anyhow::Result<DbValue> {
        self.observe_function_call("rdbms::postgres::lazy-db-value", "get");
        let value = self
            .as_wasi_view()
            .table()
            .get::<LazyDbValueEntry>(&self_)?
            .value
            .clone();
        let current_resource_rep = value.resource_rep.clone();
        let (result, new_resource_rep) =
            from_db_value(value, self.as_wasi_view().table()).map_err(Error::Other)?;
        if new_resource_rep != current_resource_rep {
            self.as_wasi_view()
                .table()
                .get_mut::<LazyDbValueEntry>(&self_)?
                .value
                .update_resource_rep(new_resource_rep)
                .map_err(Error::Other)?;
        }
        Ok(result)
    }

    async fn drop(&mut self, rep: Resource<LazyDbValueEntry>) -> anyhow::Result<()> {
        self.observe_function_call("rdbms::postgres::lazy-db-value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<LazyDbValueEntry>(rep)?;
        Ok(())
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

impl From<Interval> for postgres_types::Interval {
    fn from(v: Interval) -> Self {
        Self {
            months: v.months,
            days: v.days,
            microseconds: v.microseconds,
        }
    }
}

impl TryFrom<Timetz> for postgres_types::TimeTz {
    type Error = String;

    fn try_from(value: Timetz) -> Result<Self, Self::Error> {
        let time = value.time.try_into()?;
        let offset = chrono::offset::FixedOffset::west_opt(value.offset)
            .ok_or("Offset value is not valid")?;
        Ok(Self {
            time,
            offset: offset.utc_minus_local(),
        })
    }
}

impl From<postgres_types::TimeTz> for Timetz {
    fn from(v: postgres_types::TimeTz) -> Self {
        let time = v.time.into();
        let offset = v.offset;
        Timetz { time, offset }
    }
}

impl From<Enumeration> for postgres_types::Enumeration {
    fn from(v: Enumeration) -> Self {
        Self {
            name: v.name,
            value: v.value,
        }
    }
}

impl From<EnumerationType> for postgres_types::EnumerationType {
    fn from(v: EnumerationType) -> Self {
        Self { name: v.name }
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

impl From<postgres_types::Enumeration> for Enumeration {
    fn from(v: postgres_types::Enumeration) -> Self {
        Self {
            name: v.name,
            value: v.value,
        }
    }
}

impl From<postgres_types::EnumerationType> for EnumerationType {
    fn from(v: postgres_types::EnumerationType) -> Self {
        Self { name: v.name }
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct DbColumnTypeWithResourceRep {
    value: postgres_types::DbColumnType,
    resource_rep: DbColumnTypeResourceRep,
}

impl DbColumnTypeWithResourceRep {
    fn new(
        value: postgres_types::DbColumnType,
        resource_rep: DbColumnTypeResourceRep,
    ) -> Result<DbColumnTypeWithResourceRep, String> {
        if resource_rep.is_valid_for(&value) {
            Ok(Self {
                value,
                resource_rep,
            })
        } else {
            Err("Resource reference is not valid".to_string())
        }
    }

    fn new_resource_none(value: postgres_types::DbColumnType) -> Self {
        Self {
            value,
            resource_rep: DbColumnTypeResourceRep::None,
        }
    }

    fn update_resource_rep(&mut self, resource_rep: DbColumnTypeResourceRep) -> Result<(), String> {
        if resource_rep.is_valid_for(&self.value) {
            self.resource_rep = resource_rep;
            Ok(())
        } else {
            Err("Resource reference is not valid".to_string())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DbColumnTypeResourceRep {
    None,
    Domain(u32),
    Array(u32),
    Composite(Vec<u32>),
    Range(u32),
}

impl DbColumnTypeResourceRep {
    fn is_valid_for(&self, value: &postgres_types::DbColumnType) -> bool {
        match self {
            DbColumnTypeResourceRep::Domain(_)
                if !matches!(value, postgres_types::DbColumnType::Domain(_)) =>
            {
                false
            }
            DbColumnTypeResourceRep::Array(_)
                if !matches!(value, postgres_types::DbColumnType::Array(_)) =>
            {
                false
            }
            DbColumnTypeResourceRep::Composite(_)
                if !matches!(value, postgres_types::DbColumnType::Composite(_)) =>
            {
                false
            }
            DbColumnTypeResourceRep::Range(_)
                if !matches!(value, postgres_types::DbColumnType::Range(_)) =>
            {
                false
            }
            _ => true,
        }
    }

    fn get_composite_rep(&self, index: usize) -> Option<u32> {
        match self {
            DbColumnTypeResourceRep::Composite(reps) => reps.get(index).cloned(),
            _ => None,
        }
    }

    fn get_domain_rep(&self) -> Option<u32> {
        match self {
            DbColumnTypeResourceRep::Domain(id) => Some(*id),
            _ => None,
        }
    }

    fn get_array_rep(&self) -> Option<u32> {
        match self {
            DbColumnTypeResourceRep::Array(id) => Some(*id),
            _ => None,
        }
    }

    fn get_range_rep(&self) -> Option<u32> {
        match self {
            DbColumnTypeResourceRep::Range(id) => Some(*id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DbValueWithResourceRep {
    value: postgres_types::DbValue,
    resource_rep: DbValueResourceRep,
}

impl DbValueWithResourceRep {
    fn new(
        value: postgres_types::DbValue,
        resource_rep: DbValueResourceRep,
    ) -> Result<DbValueWithResourceRep, String> {
        if resource_rep.is_valid_for(&value) {
            Ok(Self {
                value,
                resource_rep,
            })
        } else {
            Err("Resource reference is not valid".to_string())
        }
    }

    fn new_resource_none(value: postgres_types::DbValue) -> Self {
        Self {
            value,
            resource_rep: DbValueResourceRep::None,
        }
    }

    fn update_resource_rep(&mut self, resource_rep: DbValueResourceRep) -> Result<(), String> {
        if resource_rep.is_valid_for(&self.value) {
            self.resource_rep = resource_rep;
            Ok(())
        } else {
            Err("Resource reference is not valid".to_string())
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum DbValueResourceRep {
    None,
    Domain(u32),
    Array(Vec<u32>),
    Composite(Vec<u32>),
    Range((Option<u32>, Option<u32>)),
}

impl DbValueResourceRep {
    fn is_valid_for(&self, value: &postgres_types::DbValue) -> bool {
        match self {
            DbValueResourceRep::Domain(_)
                if !matches!(value, postgres_types::DbValue::Domain(_)) =>
            {
                false
            }
            DbValueResourceRep::Array(_) if !matches!(value, postgres_types::DbValue::Array(_)) => {
                false
            }
            DbValueResourceRep::Composite(_)
                if !matches!(value, postgres_types::DbValue::Composite(_)) =>
            {
                false
            }
            DbValueResourceRep::Range(_) if !matches!(value, postgres_types::DbValue::Range(_)) => {
                false
            }
            _ => true,
        }
    }

    fn get_array_rep(&self, index: usize) -> Option<u32> {
        match self {
            DbValueResourceRep::Array(reps) => reps.get(index).cloned(),
            _ => None,
        }
    }

    fn get_composite_rep(&self, index: usize) -> Option<u32> {
        match self {
            DbValueResourceRep::Composite(reps) => reps.get(index).cloned(),
            _ => None,
        }
    }

    fn get_domain_rep(&self) -> Option<u32> {
        match self {
            DbValueResourceRep::Domain(id) => Some(*id),
            _ => None,
        }
    }

    fn get_range_rep(&self) -> Option<(Option<u32>, Option<u32>)> {
        match self {
            DbValueResourceRep::Range(id) => Some(*id),
            _ => None,
        }
    }
}

impl FromRdbmsValue<DbValue> for postgres_types::DbValue {
    fn from(value: DbValue, resource_table: &mut ResourceTable) -> Result<Self, String> {
        to_db_value(value, resource_table).map(|v| v.value)
    }
}

fn to_db_value(
    value: DbValue,
    resource_table: &mut ResourceTable,
) -> Result<DbValueWithResourceRep, String> {
    match value {
        DbValue::Character(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Character(v),
        )),
        DbValue::Int2(i) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Int2(i),
        )),
        DbValue::Int4(i) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Int4(i),
        )),
        DbValue::Int8(i) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Int8(i),
        )),
        DbValue::Numeric(v) => {
            let v = bigdecimal::BigDecimal::from_str(&v).map_err(|e| e.to_string())?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Numeric(v),
            ))
        }
        DbValue::Float4(f) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Float4(f),
        )),
        DbValue::Float8(f) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Float8(f),
        )),
        DbValue::Boolean(b) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Boolean(b),
        )),
        DbValue::Timestamp(v) => {
            let value = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Timestamp(value),
            ))
        }
        DbValue::Timestamptz(v) => {
            let value = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Timestamptz(value),
            ))
        }
        DbValue::Time(v) => {
            let value = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Time(value),
            ))
        }
        DbValue::Timetz(v) => {
            let value = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Timetz(value),
            ))
        }
        DbValue::Date(v) => {
            let value = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Date(value),
            ))
        }
        DbValue::Interval(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Interval(v.into()),
        )),
        DbValue::Text(s) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Text(s.clone()),
        )),
        DbValue::Varchar(s) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Varchar(s.clone()),
        )),
        DbValue::Bpchar(s) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Bpchar(s.clone()),
        )),
        DbValue::Bytea(u) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Bytea(u.clone()),
        )),
        DbValue::Json(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Json(v),
        )),
        DbValue::Jsonb(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Jsonb(v),
        )),
        DbValue::Jsonpath(s) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Jsonpath(s.clone()),
        )),
        DbValue::Xml(s) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Xml(s),
        )),
        DbValue::Uuid(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Uuid(v.into()),
        )),
        DbValue::Bit(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Bit(BitVec::from_iter(v)),
        )),
        DbValue::Varbit(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Varbit(BitVec::from_iter(v)),
        )),
        DbValue::Oid(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Oid(v),
        )),
        DbValue::Inet(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Inet(v.into()),
        )),
        DbValue::Cidr(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Cidr(v.into()),
        )),
        DbValue::Macaddr(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Macaddr(v.into()),
        )),
        DbValue::Int4range(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Int4range(v.into()),
        )),
        DbValue::Int8range(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Int8range(v.into()),
        )),
        DbValue::Numrange(v) => {
            let v = v.clone().try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Numrange(v),
            ))
        }
        DbValue::Tsrange(v) => {
            let v = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Tsrange(v),
            ))
        }
        DbValue::Tstzrange(v) => {
            let v = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Tstzrange(v),
            ))
        }
        DbValue::Daterange(v) => {
            let v = v.try_into()?;
            Ok(DbValueWithResourceRep::new_resource_none(
                postgres_types::DbValue::Daterange(v),
            ))
        }
        DbValue::Money(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Money(v),
        )),
        DbValue::Enumeration(v) => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Enumeration(v.into()),
        )),
        DbValue::Array(vs) => {
            let mut values: Vec<postgres_types::DbValue> = Vec::with_capacity(vs.len());
            let mut reps: Vec<u32> = Vec::with_capacity(vs.len());
            for i in vs.iter() {
                let v = resource_table
                    .get::<LazyDbValueEntry>(i)
                    .map_err(|e| e.to_string())?
                    .value
                    .clone();
                values.push(v.value);
                reps.push(i.rep());
            }
            DbValueWithResourceRep::new(
                postgres_types::DbValue::Array(values),
                DbValueResourceRep::Array(reps),
            )
        }
        DbValue::Composite(v) => {
            let mut values: Vec<postgres_types::DbValue> = Vec::with_capacity(v.values.len());
            let mut reps: Vec<u32> = Vec::with_capacity(v.values.len());
            for i in v.values.iter() {
                let v = resource_table
                    .get::<LazyDbValueEntry>(i)
                    .map_err(|e| e.to_string())?
                    .value
                    .clone();
                values.push(v.value);
                reps.push(i.rep());
            }
            DbValueWithResourceRep::new(
                postgres_types::DbValue::Composite(postgres_types::Composite::new(v.name, values)),
                DbValueResourceRep::Composite(reps),
            )
        }
        DbValue::Domain(v) => {
            let value = resource_table
                .get::<LazyDbValueEntry>(&v.value)
                .map_err(|e| e.to_string())?
                .value
                .clone();
            DbValueWithResourceRep::new(
                postgres_types::DbValue::Domain(postgres_types::Domain::new(v.name, value.value)),
                DbValueResourceRep::Domain(v.value.rep()),
            )
        }
        DbValue::Range(v) => {
            let (start_value, start_rep) = to_bound(v.value.start, resource_table)?;
            let (end_value, end_rep) = to_bound(v.value.end, resource_table)?;

            DbValueWithResourceRep::new(
                postgres_types::DbValue::Range(postgres_types::Range::new(
                    v.name,
                    postgres_types::ValuesRange::new(start_value, end_value),
                )),
                DbValueResourceRep::Range((start_rep, end_rep)),
            )
        }
        DbValue::Null => Ok(DbValueWithResourceRep::new_resource_none(
            postgres_types::DbValue::Null,
        )),
    }
}

fn to_bound(
    value: ValueBound,
    resource_table: &mut ResourceTable,
) -> Result<(Bound<postgres_types::DbValue>, Option<u32>), String> {
    match value {
        ValueBound::Included(r) => {
            let value = resource_table
                .get::<LazyDbValueEntry>(&r)
                .map_err(|e| e.to_string())?
                .value
                .clone();
            Ok((Bound::Included(value.value), Some(r.rep())))
        }
        ValueBound::Excluded(r) => {
            let value = resource_table
                .get::<LazyDbValueEntry>(&r)
                .map_err(|e| e.to_string())?
                .value
                .clone();
            Ok((Bound::Excluded(value.value), Some(r.rep())))
        }
        ValueBound::Unbounded => Ok((Bound::Unbounded, None)),
    }
}

fn from_db_rows(
    values: Vec<crate::services::rdbms::DbRow<postgres_types::DbValue>>,
    resource_table: &mut ResourceTable,
) -> Result<Vec<DbRow>, String> {
    let mut result: Vec<DbRow> = Vec::with_capacity(values.len());
    for value in values {
        let v = from_db_row(value, resource_table)?;
        result.push(v);
    }
    Ok(result)
}

impl FromRdbmsValue<crate::services::rdbms::DbRow<postgres_types::DbValue>> for DbRow {
    fn from(
        value: crate::services::rdbms::DbRow<postgres_types::DbValue>,
        resource_table: &mut ResourceTable,
    ) -> Result<Self, String> {
        from_db_row(value, resource_table)
    }
}

fn from_db_row(
    value: crate::services::rdbms::DbRow<postgres_types::DbValue>,
    resource_table: &mut ResourceTable,
) -> Result<DbRow, String> {
    let mut values: Vec<DbValue> = Vec::with_capacity(value.values.len());
    for value in value.values {
        let v = from_db_value(
            DbValueWithResourceRep::new_resource_none(value),
            resource_table,
        )?;
        values.push(v.0);
    }
    Ok(DbRow { values })
}

fn from_db_value(
    value: DbValueWithResourceRep,
    resource_table: &mut ResourceTable,
) -> Result<(DbValue, DbValueResourceRep), String> {
    match value.value {
        postgres_types::DbValue::Character(s) => {
            Ok((DbValue::Character(s), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Int2(i) => Ok((DbValue::Int2(i), DbValueResourceRep::None)),
        postgres_types::DbValue::Int4(i) => Ok((DbValue::Int4(i), DbValueResourceRep::None)),
        postgres_types::DbValue::Int8(i) => Ok((DbValue::Int8(i), DbValueResourceRep::None)),
        postgres_types::DbValue::Numeric(s) => {
            Ok((DbValue::Numeric(s.to_string()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Float4(f) => Ok((DbValue::Float4(f), DbValueResourceRep::None)),
        postgres_types::DbValue::Float8(f) => Ok((DbValue::Float8(f), DbValueResourceRep::None)),
        postgres_types::DbValue::Boolean(b) => Ok((DbValue::Boolean(b), DbValueResourceRep::None)),
        postgres_types::DbValue::Timestamp(v) => {
            Ok((DbValue::Timestamp(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Timestamptz(v) => {
            Ok((DbValue::Timestamptz(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Time(v) => Ok((DbValue::Time(v.into()), DbValueResourceRep::None)),
        postgres_types::DbValue::Timetz(v) => {
            Ok((DbValue::Timetz(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Date(v) => Ok((DbValue::Date(v.into()), DbValueResourceRep::None)),
        postgres_types::DbValue::Interval(v) => {
            Ok((DbValue::Interval(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Text(s) => Ok((DbValue::Text(s), DbValueResourceRep::None)),
        postgres_types::DbValue::Varchar(s) => Ok((DbValue::Varchar(s), DbValueResourceRep::None)),
        postgres_types::DbValue::Bpchar(s) => Ok((DbValue::Bpchar(s), DbValueResourceRep::None)),
        postgres_types::DbValue::Bytea(u) => Ok((DbValue::Bytea(u), DbValueResourceRep::None)),
        postgres_types::DbValue::Json(s) => Ok((DbValue::Json(s), DbValueResourceRep::None)),
        postgres_types::DbValue::Jsonb(s) => Ok((DbValue::Jsonb(s), DbValueResourceRep::None)),
        postgres_types::DbValue::Jsonpath(s) => {
            Ok((DbValue::Jsonpath(s), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Xml(s) => Ok((DbValue::Xml(s), DbValueResourceRep::None)),
        postgres_types::DbValue::Uuid(uuid) => {
            Ok((DbValue::Uuid(uuid.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Bit(v) => {
            Ok((DbValue::Bit(v.iter().collect()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Varbit(v) => Ok((
            DbValue::Varbit(v.iter().collect()),
            DbValueResourceRep::None,
        )),
        postgres_types::DbValue::Inet(v) => Ok((DbValue::Inet(v.into()), DbValueResourceRep::None)),
        postgres_types::DbValue::Cidr(v) => Ok((DbValue::Cidr(v.into()), DbValueResourceRep::None)),
        postgres_types::DbValue::Macaddr(v) => {
            Ok((DbValue::Macaddr(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Tsrange(v) => {
            Ok((DbValue::Tsrange(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Tstzrange(v) => {
            Ok((DbValue::Tstzrange(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Daterange(v) => {
            Ok((DbValue::Daterange(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Int4range(v) => {
            Ok((DbValue::Int4range(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Int8range(v) => {
            Ok((DbValue::Int8range(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Numrange(v) => {
            Ok((DbValue::Numrange(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Oid(v) => Ok((DbValue::Oid(v), DbValueResourceRep::None)),
        postgres_types::DbValue::Money(v) => Ok((DbValue::Money(v), DbValueResourceRep::None)),
        postgres_types::DbValue::Enumeration(v) => {
            Ok((DbValue::Enumeration(v.into()), DbValueResourceRep::None))
        }
        postgres_types::DbValue::Composite(v) => {
            let mut values: Vec<Resource<LazyDbValueEntry>> = Vec::with_capacity(v.values.len());
            let mut new_resource_reps: Vec<u32> = Vec::with_capacity(v.values.len());
            for (i, v) in v.values.into_iter().enumerate() {
                let value = get_db_value_resource(
                    v,
                    value.resource_rep.get_composite_rep(i),
                    resource_table,
                )?;
                new_resource_reps.push(value.rep());
                values.push(value);
            }
            Ok((
                DbValue::Composite(Composite {
                    name: v.name,
                    values,
                }),
                DbValueResourceRep::Composite(new_resource_reps),
            ))
        }
        postgres_types::DbValue::Domain(v) => {
            let value = get_db_value_resource(
                *v.value,
                value.resource_rep.get_domain_rep(),
                resource_table,
            )?;
            let new_resource_rep = value.rep();
            Ok((
                DbValue::Domain(Domain {
                    name: v.name,
                    value,
                }),
                DbValueResourceRep::Domain(new_resource_rep),
            ))
        }
        postgres_types::DbValue::Array(vs) => {
            let mut values: Vec<Resource<LazyDbValueEntry>> = Vec::with_capacity(vs.len());
            let mut new_resource_reps: Vec<u32> = Vec::with_capacity(vs.len());
            for (i, v) in vs.into_iter().enumerate() {
                let value =
                    get_db_value_resource(v, value.resource_rep.get_array_rep(i), resource_table)?;
                new_resource_reps.push(value.rep());
                values.push(value);
            }

            Ok((
                DbValue::Array(values),
                DbValueResourceRep::Array(new_resource_reps),
            ))
        }
        postgres_types::DbValue::Range(v) => {
            let reps = value.resource_rep.get_range_rep();
            let (start, start_rep) =
                from_bound(v.value.start, reps.and_then(|r| r.0), resource_table)?;
            let (end, end_rep) = from_bound(v.value.end, reps.and_then(|r| r.1), resource_table)?;
            let value = ValuesRange { start, end };
            Ok((
                DbValue::Range(Range {
                    name: v.name,
                    value,
                }),
                DbValueResourceRep::Range((start_rep, end_rep)),
            ))
        }
        postgres_types::DbValue::Null => Ok((DbValue::Null, DbValueResourceRep::None)),
    }
}

fn get_db_value_resource(
    value: postgres_types::DbValue,
    resource_rep: Option<u32>,
    resource_table: &mut ResourceTable,
) -> Result<Resource<LazyDbValueEntry>, String> {
    if let Some(r) = resource_rep {
        Ok(Resource::new_own(r))
    } else {
        resource_table
            .push(LazyDbValueEntry::new(
                DbValueWithResourceRep::new_resource_none(value),
            ))
            .map_err(|e| e.to_string())
    }
}

fn from_bound(
    bound: Bound<postgres_types::DbValue>,
    resource_rep: Option<u32>,
    resource_table: &mut ResourceTable,
) -> Result<(ValueBound, Option<u32>), String> {
    match bound {
        Bound::Included(v) => {
            let value = get_db_value_resource(v, resource_rep, resource_table)?;
            let rep = value.rep();
            Ok((ValueBound::Included(value), Some(rep)))
        }
        Bound::Excluded(v) => {
            let value = get_db_value_resource(v, resource_rep, resource_table)?;
            let rep = value.rep();
            Ok((ValueBound::Excluded(value), Some(rep)))
        }
        Bound::Unbounded => Ok((ValueBound::Unbounded, None)),
    }
}

fn from_db_columns(
    values: Vec<postgres_types::DbColumn>,
    resource_table: &mut ResourceTable,
) -> Result<Vec<DbColumn>, String> {
    let mut result: Vec<DbColumn> = Vec::with_capacity(values.len());
    for value in values {
        let v = from_db_column(value, resource_table)?;
        result.push(v);
    }
    Ok(result)
}

impl FromRdbmsValue<postgres_types::DbColumn> for DbColumn {
    fn from(
        value: postgres_types::DbColumn,
        resource_table: &mut ResourceTable,
    ) -> Result<Self, String> {
        from_db_column(value, resource_table)
    }
}

fn from_db_column(
    value: postgres_types::DbColumn,
    resource_table: &mut ResourceTable,
) -> Result<DbColumn, String> {
    let (db_type, _) = from_db_column_type(
        DbColumnTypeWithResourceRep::new(value.db_type, DbColumnTypeResourceRep::None)?,
        resource_table,
    )?;
    Ok(DbColumn {
        ordinal: value.ordinal,
        name: value.name,
        db_type,
        db_type_name: value.db_type_name,
    })
}

fn from_db_column_type(
    value: DbColumnTypeWithResourceRep,
    resource_table: &mut ResourceTable,
) -> Result<(DbColumnType, DbColumnTypeResourceRep), String> {
    match value.value {
        postgres_types::DbColumnType::Character => {
            Ok((DbColumnType::Character, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Int2 => {
            Ok((DbColumnType::Int2, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Int4 => {
            Ok((DbColumnType::Int4, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Int8 => {
            Ok((DbColumnType::Int8, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Numeric => {
            Ok((DbColumnType::Numeric, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Float4 => {
            Ok((DbColumnType::Float4, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Float8 => {
            Ok((DbColumnType::Float8, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Boolean => {
            Ok((DbColumnType::Boolean, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Timestamp => {
            Ok((DbColumnType::Timestamp, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Timestamptz => {
            Ok((DbColumnType::Timestamptz, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Time => {
            Ok((DbColumnType::Time, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Timetz => {
            Ok((DbColumnType::Timetz, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Date => {
            Ok((DbColumnType::Date, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Interval => {
            Ok((DbColumnType::Interval, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Text => {
            Ok((DbColumnType::Text, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Varchar => {
            Ok((DbColumnType::Varchar, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Bpchar => {
            Ok((DbColumnType::Bpchar, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Bytea => {
            Ok((DbColumnType::Bytea, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Json => {
            Ok((DbColumnType::Json, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Jsonb => {
            Ok((DbColumnType::Jsonb, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Jsonpath => {
            Ok((DbColumnType::Jsonpath, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Xml => Ok((DbColumnType::Xml, DbColumnTypeResourceRep::None)),
        postgres_types::DbColumnType::Uuid => {
            Ok((DbColumnType::Uuid, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Bit => Ok((DbColumnType::Bit, DbColumnTypeResourceRep::None)),
        postgres_types::DbColumnType::Varbit => {
            Ok((DbColumnType::Varbit, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Inet => {
            Ok((DbColumnType::Inet, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Cidr => {
            Ok((DbColumnType::Cidr, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Macaddr => {
            Ok((DbColumnType::Macaddr, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Tsrange => {
            Ok((DbColumnType::Tsrange, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Tstzrange => {
            Ok((DbColumnType::Tstzrange, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Daterange => {
            Ok((DbColumnType::Daterange, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Int4range => {
            Ok((DbColumnType::Int4range, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Int8range => {
            Ok((DbColumnType::Int8range, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Numrange => {
            Ok((DbColumnType::Numrange, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Oid => Ok((DbColumnType::Oid, DbColumnTypeResourceRep::None)),
        postgres_types::DbColumnType::Money => {
            Ok((DbColumnType::Money, DbColumnTypeResourceRep::None))
        }
        postgres_types::DbColumnType::Enumeration(v) => Ok((
            DbColumnType::Enumeration(v.into()),
            DbColumnTypeResourceRep::None,
        )),
        postgres_types::DbColumnType::Composite(v) => {
            let mut attributes: Vec<(String, Resource<LazyDbColumnTypeEntry>)> =
                Vec::with_capacity(v.attributes.len());
            let mut new_resource_reps: Vec<u32> = Vec::with_capacity(v.attributes.len());
            for (i, (n, t)) in v.attributes.into_iter().enumerate() {
                let value = get_db_column_type_resource(
                    t,
                    value.resource_rep.get_composite_rep(i),
                    resource_table,
                )?;
                new_resource_reps.push(value.rep());
                attributes.push((n, value));
            }
            Ok((
                DbColumnType::Composite(CompositeType {
                    name: v.name,
                    attributes,
                }),
                DbColumnTypeResourceRep::Composite(new_resource_reps),
            ))
        }
        postgres_types::DbColumnType::Domain(v) => {
            let value = get_db_column_type_resource(
                *v.base_type,
                value.resource_rep.get_domain_rep(),
                resource_table,
            )?;
            let new_resource_rep = value.rep();
            Ok((
                DbColumnType::Domain(DomainType {
                    name: v.name,
                    base_type: value,
                }),
                DbColumnTypeResourceRep::Domain(new_resource_rep),
            ))
        }
        postgres_types::DbColumnType::Array(v) => {
            let value = get_db_column_type_resource(
                *v,
                value.resource_rep.get_array_rep(),
                resource_table,
            )?;
            let new_resource_rep = value.rep();
            Ok((
                DbColumnType::Array(value),
                DbColumnTypeResourceRep::Array(new_resource_rep),
            ))
        }
        postgres_types::DbColumnType::Range(v) => {
            let value = get_db_column_type_resource(
                *v.base_type,
                value.resource_rep.get_range_rep(),
                resource_table,
            )?;
            let new_resource_rep = value.rep();
            Ok((
                DbColumnType::Range(RangeType {
                    name: v.name,
                    base_type: value,
                }),
                DbColumnTypeResourceRep::Range(new_resource_rep),
            ))
        }
        postgres_types::DbColumnType::Null => Err("Type 'Null' is not supported".to_string()),
    }
}

fn get_db_column_type_resource(
    value: postgres_types::DbColumnType,
    resource_rep: Option<u32>,
    resource_table: &mut ResourceTable,
) -> Result<Resource<LazyDbColumnTypeEntry>, String> {
    if let Some(r) = resource_rep {
        Ok(Resource::new_own(r))
    } else {
        resource_table
            .push(LazyDbColumnTypeEntry::new(
                DbColumnTypeWithResourceRep::new_resource_none(value),
            ))
            .map_err(|e| e.to_string())
    }
}

fn to_db_column_type(
    value: DbColumnType,
    resource_table: &mut ResourceTable,
) -> Result<DbColumnTypeWithResourceRep, String> {
    match value {
        DbColumnType::Character => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Character,
        )),
        DbColumnType::Int2 => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Int2,
        )),
        DbColumnType::Int4 => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Int4,
        )),
        DbColumnType::Int8 => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Int8,
        )),
        DbColumnType::Numeric => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Numeric,
        )),
        DbColumnType::Float4 => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Float4,
        )),
        DbColumnType::Float8 => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Float8,
        )),
        DbColumnType::Boolean => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Boolean,
        )),
        DbColumnType::Timestamp => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Timestamp,
        )),
        DbColumnType::Timestamptz => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Timestamptz,
        )),
        DbColumnType::Time => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Time,
        )),
        DbColumnType::Timetz => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Timetz,
        )),
        DbColumnType::Date => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Date,
        )),
        DbColumnType::Interval => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Interval,
        )),
        DbColumnType::Bytea => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Bytea,
        )),
        DbColumnType::Text => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Text,
        )),
        DbColumnType::Varchar => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Varchar,
        )),
        DbColumnType::Bpchar => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Bpchar,
        )),
        DbColumnType::Json => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Json,
        )),
        DbColumnType::Jsonb => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Jsonb,
        )),
        DbColumnType::Jsonpath => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Jsonpath,
        )),
        DbColumnType::Uuid => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Uuid,
        )),
        DbColumnType::Xml => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Xml,
        )),
        DbColumnType::Bit => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Bit,
        )),
        DbColumnType::Varbit => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Varbit,
        )),
        DbColumnType::Inet => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Inet,
        )),
        DbColumnType::Cidr => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Cidr,
        )),
        DbColumnType::Macaddr => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Macaddr,
        )),
        DbColumnType::Tsrange => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Tsrange,
        )),
        DbColumnType::Tstzrange => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Tstzrange,
        )),
        DbColumnType::Daterange => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Daterange,
        )),
        DbColumnType::Int4range => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Int4range,
        )),
        DbColumnType::Int8range => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Int8range,
        )),
        DbColumnType::Numrange => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Numrange,
        )),
        DbColumnType::Oid => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Oid,
        )),
        DbColumnType::Money => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Money,
        )),
        DbColumnType::Enumeration(v) => Ok(DbColumnTypeWithResourceRep::new_resource_none(
            postgres_types::DbColumnType::Enumeration(v.into()),
        )),
        DbColumnType::Composite(v) => {
            let mut attributes: Vec<(String, postgres_types::DbColumnType)> =
                Vec::with_capacity(v.attributes.len());
            let mut resource_reps: Vec<u32> = Vec::with_capacity(v.attributes.len());
            for (name, resource) in v.attributes.iter() {
                let value = resource_table
                    .get::<LazyDbColumnTypeEntry>(resource)
                    .map_err(|e| e.to_string())?
                    .value
                    .clone();
                resource_reps.push(resource.rep());
                attributes.push((name.clone(), value.value));
            }
            DbColumnTypeWithResourceRep::new(
                postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
                    v.name, attributes,
                )),
                DbColumnTypeResourceRep::Composite(resource_reps),
            )
        }
        DbColumnType::Domain(v) => {
            let value = resource_table
                .get::<LazyDbColumnTypeEntry>(&v.base_type)
                .map_err(|e| e.to_string())?
                .value
                .clone();
            DbColumnTypeWithResourceRep::new(
                postgres_types::DbColumnType::Domain(postgres_types::DomainType::new(
                    v.name,
                    value.value,
                )),
                DbColumnTypeResourceRep::Domain(v.base_type.rep()),
            )
        }
        DbColumnType::Array(v) => {
            let value = resource_table
                .get::<LazyDbColumnTypeEntry>(&v)
                .map_err(|e| e.to_string())?
                .value
                .clone();
            DbColumnTypeWithResourceRep::new(
                postgres_types::DbColumnType::Array(Box::new(value.value)),
                DbColumnTypeResourceRep::Array(v.rep()),
            )
        }
        DbColumnType::Range(v) => {
            let value = resource_table
                .get::<LazyDbColumnTypeEntry>(&v.base_type)
                .map_err(|e| e.to_string())?
                .value
                .clone();
            DbColumnTypeWithResourceRep::new(
                postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
                    v.name,
                    value.value,
                )),
                DbColumnTypeResourceRep::Range(v.base_type.rep()),
            )
        }
    }
}

impl FromRdbmsValue<crate::services::rdbms::DbResult<PostgresType>> for DbResult {
    fn from(
        value: crate::services::rdbms::DbResult<PostgresType>,
        resource_table: &mut ResourceTable,
    ) -> Result<DbResult, String> {
        from_db_result(value, resource_table)
    }
}

fn from_db_result(
    result: crate::services::rdbms::DbResult<PostgresType>,
    resource_table: &mut ResourceTable,
) -> Result<DbResult, String> {
    let columns = from_db_columns(result.columns, resource_table)?;
    let rows = from_db_rows(result.rows, resource_table)?;
    Ok(DbResult { columns, rows })
}

#[cfg(test)]
pub mod tests {
    use crate::durable_host::rdbms::postgres::{
        from_db_column_type, from_db_value, to_db_column_type, to_db_value,
        DbColumnTypeResourceRep, DbColumnTypeWithResourceRep, DbValueResourceRep,
        DbValueWithResourceRep,
    };
    use crate::services::rdbms::postgres::types as postgres_types;
    use assert2::check;
    use test_r::test;
    use wasmtime::component::ResourceTable;

    fn check_db_value(value: postgres_types::DbValue, resource_table: &mut ResourceTable) {
        let value_with_rep = DbValueWithResourceRep::new_resource_none(value.clone());
        let (wit, new_resource_reps) = from_db_value(value_with_rep, resource_table).unwrap();

        let value_with_rep =
            DbValueWithResourceRep::new(value.clone(), new_resource_reps.clone()).unwrap();
        let (wit2, new_resource_reps2) = from_db_value(value_with_rep, resource_table).unwrap();

        check!(new_resource_reps == new_resource_reps2);

        if value.is_complex_type() {
            check!(new_resource_reps2 != DbValueResourceRep::None);
        } else {
            check!(new_resource_reps2 == DbValueResourceRep::None);
        }

        let result = to_db_value(wit, resource_table).unwrap();
        let result2 = to_db_value(wit2, resource_table).unwrap();

        check!(result.value == value);
        check!(result2.value == value);
    }

    #[test]
    fn test_db_values_conversions() {
        let mut resource_table = ResourceTable::new();
        let values = postgres_types::tests::get_test_db_values();

        for value in values {
            check_db_value(value, &mut resource_table);
        }
    }

    fn check_db_column_type(
        value: postgres_types::DbColumnType,
        resource_table: &mut ResourceTable,
    ) {
        let value_with_rep = DbColumnTypeWithResourceRep::new_resource_none(value.clone());
        let (wit, new_resource_reps) = from_db_column_type(value_with_rep, resource_table).unwrap();

        let value_with_rep =
            DbColumnTypeWithResourceRep::new(value.clone(), new_resource_reps.clone()).unwrap();
        let (wit2, new_resource_reps2) =
            from_db_column_type(value_with_rep, resource_table).unwrap();

        check!(new_resource_reps == new_resource_reps2);

        if value.is_complex_type() {
            check!(new_resource_reps2 != DbColumnTypeResourceRep::None);
        } else {
            check!(new_resource_reps2 == DbColumnTypeResourceRep::None);
        }

        let result = to_db_column_type(wit, resource_table).unwrap();
        let result2 = to_db_column_type(wit2, resource_table).unwrap();

        check!(result.value == value);
        check!(result2.value == value);
    }

    #[test]
    fn test_db_column_types_conversions() {
        let mut resource_table = ResourceTable::new();

        let values = postgres_types::tests::get_test_db_column_types();
        for value in values {
            check_db_column_type(value, &mut resource_table);
        }
    }
}
