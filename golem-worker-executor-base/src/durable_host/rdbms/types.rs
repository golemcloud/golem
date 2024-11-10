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

use crate::durable_host::rdbms::RdbmsType;
use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::rdbms::types::{
    DbColumn, DbColumnType, DbColumnTypePrimitive, DbRow, DbValue, DbValuePrimitive, Error, Host,
    HostDbResultSet,
};
use crate::services::rdbms::types as rdbms_types;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::OwnedWorkerId;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {}

pub struct DbResultSetEntry {
    pub rdbms_type: RdbmsType,
    pub worker_id: OwnedWorkerId,
    pub internal: Arc<dyn rdbms_types::DbResultSet + Send + Sync>,
}

impl DbResultSetEntry {
    pub fn new(
        rdbms_type: RdbmsType,
        worker_id: OwnedWorkerId,
        internal: Arc<dyn rdbms_types::DbResultSet + Send + Sync>,
    ) -> Self {
        Self {
            rdbms_type,
            worker_id,
            internal,
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-column-metadata");

        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let columns = internal.deref().get_columns().await.map_err(Error::from)?;

        let columns = columns.into_iter().map(|c| c.into()).collect();
        Ok(columns)
    }

    async fn get_next(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-next");
        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let rows = internal.deref().get_next().await.map_err(Error::from)?;

        let rows = rows.map(|r| r.into_iter().map(|r| r.into()).collect());
        Ok(rows)
    }

    fn drop(&mut self, rep: Resource<DbResultSetEntry>) -> anyhow::Result<()> {
        record_host_function_call("rdbms::types::db-result-set", "drop");
        self.as_wasi_view()
            .table()
            .delete::<DbResultSetEntry>(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for &mut DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        (*self).get_columns(self_).await
    }

    async fn get_next(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        (*self).get_next(self_).await
    }

    fn drop(&mut self, rep: Resource<DbResultSetEntry>) -> anyhow::Result<()> {
        (*self).drop(rep)
    }
}

impl From<DbValuePrimitive> for rdbms_types::DbValuePrimitive {
    fn from(value: DbValuePrimitive) -> Self {
        match value {
            DbValuePrimitive::Int8(i) => Self::Int8(i),
            DbValuePrimitive::Int16(i) => Self::Int16(i),
            DbValuePrimitive::Int32(i) => Self::Int32(i),
            DbValuePrimitive::Int64(i) => Self::Int64(i),
            DbValuePrimitive::Decimal(s) => {
                Self::Decimal(bigdecimal::BigDecimal::from_str(&s).unwrap())
            } // FIXME change to TryFrom
            DbValuePrimitive::Float(f) => Self::Float(f),
            DbValuePrimitive::Double(f) => Self::Double(f),
            DbValuePrimitive::Boolean(b) => Self::Boolean(b),
            DbValuePrimitive::Timestamp(u) => Self::Timestamp(u),
            DbValuePrimitive::Time(u) => Self::Time(u),
            DbValuePrimitive::Interval(u) => Self::Interval(u),
            DbValuePrimitive::Date(u) => Self::Date(u),
            DbValuePrimitive::Text(s) => Self::Text(s),
            DbValuePrimitive::Blob(u) => Self::Blob(u),
            DbValuePrimitive::Json(s) => Self::Json(s),
            DbValuePrimitive::Xml(s) => Self::Xml(s),
            DbValuePrimitive::Uuid((h, l)) => Self::Uuid(Uuid::from_u64_pair(h, l)),
            DbValuePrimitive::DbNull => Self::DbNull,
        }
    }
}

impl From<rdbms_types::DbValuePrimitive> for DbValuePrimitive {
    fn from(value: rdbms_types::DbValuePrimitive) -> Self {
        match value {
            rdbms_types::DbValuePrimitive::Int8(i) => Self::Int8(i),
            rdbms_types::DbValuePrimitive::Int16(i) => Self::Int16(i),
            rdbms_types::DbValuePrimitive::Int32(i) => Self::Int32(i),
            rdbms_types::DbValuePrimitive::Int64(i) => Self::Int64(i),
            rdbms_types::DbValuePrimitive::Decimal(s) => Self::Decimal(s.to_string()),
            rdbms_types::DbValuePrimitive::Float(f) => Self::Float(f),
            rdbms_types::DbValuePrimitive::Double(f) => Self::Double(f),
            rdbms_types::DbValuePrimitive::Boolean(b) => Self::Boolean(b),
            rdbms_types::DbValuePrimitive::Timestamp(u) => Self::Timestamp(u),
            rdbms_types::DbValuePrimitive::Time(u) => Self::Time(u),
            rdbms_types::DbValuePrimitive::Interval(u) => Self::Interval(u),
            rdbms_types::DbValuePrimitive::Date(u) => Self::Date(u),
            rdbms_types::DbValuePrimitive::Text(s) => Self::Text(s),
            rdbms_types::DbValuePrimitive::Blob(u) => Self::Blob(u),
            rdbms_types::DbValuePrimitive::Json(s) => Self::Json(s),
            rdbms_types::DbValuePrimitive::Xml(s) => Self::Xml(s),
            rdbms_types::DbValuePrimitive::Uuid(uuid) => Self::Uuid(uuid.as_u64_pair()),
            rdbms_types::DbValuePrimitive::DbNull => Self::DbNull,
        }
    }
}

impl From<DbValue> for rdbms_types::DbValue {
    fn from(value: DbValue) -> Self {
        match value {
            DbValue::Primitive(p) => Self::Primitive(p.into()),
            DbValue::Array(vs) => Self::Array(vs.into_iter().map(|v| v.into()).collect()),
        }
    }
}

impl From<rdbms_types::DbValue> for DbValue {
    fn from(value: rdbms_types::DbValue) -> Self {
        match value {
            rdbms_types::DbValue::Primitive(p) => Self::Primitive(p.into()),
            rdbms_types::DbValue::Array(vs) => {
                Self::Array(vs.into_iter().map(|v| v.into()).collect())
            }
        }
    }
}

impl From<rdbms_types::DbRow> for DbRow {
    fn from(value: rdbms_types::DbRow) -> Self {
        Self {
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl From<rdbms_types::DbColumnTypePrimitive> for DbColumnTypePrimitive {
    fn from(value: rdbms_types::DbColumnTypePrimitive) -> Self {
        match value {
            rdbms_types::DbColumnTypePrimitive::Int8 => Self::Int8,
            rdbms_types::DbColumnTypePrimitive::Int16 => Self::Int16,
            rdbms_types::DbColumnTypePrimitive::Int32 => Self::Int32,
            rdbms_types::DbColumnTypePrimitive::Int64 => Self::Int64,
            rdbms_types::DbColumnTypePrimitive::Decimal => Self::Decimal,
            rdbms_types::DbColumnTypePrimitive::Float => Self::Float,
            rdbms_types::DbColumnTypePrimitive::Double => Self::Double,
            rdbms_types::DbColumnTypePrimitive::Boolean => Self::Boolean,
            rdbms_types::DbColumnTypePrimitive::Timestamp => Self::Timestamp,
            rdbms_types::DbColumnTypePrimitive::Interval => Self::Interval,
            rdbms_types::DbColumnTypePrimitive::Time => Self::Time,
            rdbms_types::DbColumnTypePrimitive::Date => Self::Date,
            rdbms_types::DbColumnTypePrimitive::Text => Self::Text,
            rdbms_types::DbColumnTypePrimitive::Blob => Self::Blob,
            rdbms_types::DbColumnTypePrimitive::Json => Self::Json,
            rdbms_types::DbColumnTypePrimitive::Xml => Self::Xml,
            rdbms_types::DbColumnTypePrimitive::Uuid => Self::Uuid,
        }
    }
}

impl From<rdbms_types::DbColumnType> for DbColumnType {
    fn from(value: rdbms_types::DbColumnType) -> Self {
        match value {
            rdbms_types::DbColumnType::Primitive(p) => Self::Primitive(p.into()),
            rdbms_types::DbColumnType::Array(p) => Self::Array(p.into()),
        }
    }
}

impl From<rdbms_types::DbColumn> for DbColumn {
    fn from(value: rdbms_types::DbColumn) -> Self {
        Self {
            ordinal: value.ordinal,
            name: value.name,
            db_type: value.db_type.into(),
            db_type_name: value.db_type_name,
        }
    }
}

impl From<rdbms_types::Error> for Error {
    fn from(value: rdbms_types::Error) -> Self {
        match value {
            rdbms_types::Error::ConnectionFailure(v) => Self::ConnectionFailure(v),
            rdbms_types::Error::QueryParameterFailure(v) => Self::QueryParameterFailure(v),
            rdbms_types::Error::QueryExecutionFailure(v) => Self::QueryExecutionFailure(v),
            rdbms_types::Error::QueryResponseFailure(v) => Self::QueryResponseFailure(v),
            rdbms_types::Error::Other(v) => Self::Other(v),
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
