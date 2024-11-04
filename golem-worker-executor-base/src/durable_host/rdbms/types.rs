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
    DbColumnType, DbColumnTypeFlags, DbColumnTypeMeta, DbColumnTypePrimitive, DbRow, DbValue,
    DbValuePrimitive, Error, Host, HostDbResultSet,
};
use crate::services::rdbms::types as rdbms_types;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use std::ops::Deref;
use std::sync::Arc;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {}

pub struct DbResultSetEntry {
    pub rdbms_type: RdbmsType,
    pub internal: Arc<dyn rdbms_types::DbResultSet + Send + Sync>,
}

impl DbResultSetEntry {
    pub fn new(
        rdbms_type: RdbmsType,
        internal: Arc<dyn rdbms_types::DbResultSet + Send + Sync>,
    ) -> Self {
        Self {
            rdbms_type,
            internal,
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for DurableWorkerCtx<Ctx> {
    async fn get_column_metadata(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumnTypeMeta>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-column-metadata");

        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)?
            .internal
            .clone();

        let columns = internal
            .deref()
            .get_column_metadata()
            .await
            .map_err(Error::Error)?;

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

        let rows = internal.deref().get_next().await.map_err(Error::Error)?;
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
    async fn get_column_metadata(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumnTypeMeta>> {
        (*self).get_column_metadata(self_).await
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
            DbValuePrimitive::Integer(i) => Self::Integer(i),
            DbValuePrimitive::Decimal(s) => Self::Decimal(s),
            DbValuePrimitive::Float(f) => Self::Float(f),
            DbValuePrimitive::Boolean(b) => Self::Boolean(b),
            DbValuePrimitive::Datetime(u) => Self::Datetime(u),
            DbValuePrimitive::Interval(u) => Self::Interval(u),
            DbValuePrimitive::Chars(u) => Self::Chars(u),
            DbValuePrimitive::Text(s) => Self::Text(s),
            DbValuePrimitive::Binary(u) => Self::Binary(u),
            DbValuePrimitive::Blob(u) => Self::Blob(u),
            DbValuePrimitive::Enumeration(v) => Self::Enumeration(v),
            DbValuePrimitive::Json(s) => Self::Json(s),
            DbValuePrimitive::Xml(s) => Self::Xml(s),
            DbValuePrimitive::Uuid((h, l)) => Self::Uuid(Uuid::from_u64_pair(h, l)),
            DbValuePrimitive::Spatial(v) => Self::Spatial(v),
            DbValuePrimitive::Other((n, v)) => Self::Other(n, v),
            DbValuePrimitive::DbNull => Self::DbNull,
        }
    }
}

impl From<rdbms_types::DbValuePrimitive> for DbValuePrimitive {
    fn from(value: rdbms_types::DbValuePrimitive) -> Self {
        match value {
            rdbms_types::DbValuePrimitive::Integer(i) => Self::Integer(i),
            rdbms_types::DbValuePrimitive::Decimal(s) => Self::Decimal(s),
            rdbms_types::DbValuePrimitive::Float(f) => Self::Float(f),
            rdbms_types::DbValuePrimitive::Boolean(b) => Self::Boolean(b),
            rdbms_types::DbValuePrimitive::Datetime(u) => Self::Datetime(u),
            rdbms_types::DbValuePrimitive::Interval(u) => Self::Interval(u),
            rdbms_types::DbValuePrimitive::Chars(u) => Self::Chars(u),
            rdbms_types::DbValuePrimitive::Text(s) => Self::Text(s),
            rdbms_types::DbValuePrimitive::Binary(u) => Self::Binary(u),
            rdbms_types::DbValuePrimitive::Blob(u) => Self::Blob(u),
            rdbms_types::DbValuePrimitive::Enumeration(v) => Self::Enumeration(v),
            rdbms_types::DbValuePrimitive::Json(s) => Self::Json(s),
            rdbms_types::DbValuePrimitive::Xml(s) => Self::Xml(s),
            rdbms_types::DbValuePrimitive::Uuid(uuid) => Self::Uuid(uuid.as_u64_pair()),
            rdbms_types::DbValuePrimitive::Spatial(v) => Self::Spatial(v),
            rdbms_types::DbValuePrimitive::Other(n, v) => Self::Other((n, v)),
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
            rdbms_types::DbColumnTypePrimitive::Integer(s) => Self::Integer(s),
            rdbms_types::DbColumnTypePrimitive::Decimal(p, s) => Self::Decimal((p, s)),
            rdbms_types::DbColumnTypePrimitive::Float => Self::Float,
            rdbms_types::DbColumnTypePrimitive::Boolean => Self::Boolean,
            rdbms_types::DbColumnTypePrimitive::Datetime => Self::Datetime,
            rdbms_types::DbColumnTypePrimitive::Interval => Self::Interval,
            rdbms_types::DbColumnTypePrimitive::Chars(s) => Self::Chars(s),
            rdbms_types::DbColumnTypePrimitive::Text => Self::Text,
            rdbms_types::DbColumnTypePrimitive::Binary(s) => Self::Binary(s),
            rdbms_types::DbColumnTypePrimitive::Blob => Self::Blob,
            rdbms_types::DbColumnTypePrimitive::Enumeration(vs) => Self::Enumeration(vs),
            rdbms_types::DbColumnTypePrimitive::Json => Self::Json,
            rdbms_types::DbColumnTypePrimitive::Xml => Self::Xml,
            rdbms_types::DbColumnTypePrimitive::Uuid => Self::Uuid,
            rdbms_types::DbColumnTypePrimitive::Spatial => Self::Spatial,
        }
    }
}

impl From<rdbms_types::DbColumnType> for DbColumnType {
    fn from(value: rdbms_types::DbColumnType) -> Self {
        match value {
            rdbms_types::DbColumnType::Primitive(p) => Self::Primitive(p.into()),
            rdbms_types::DbColumnType::Array(vs, p) => Self::Array((vs, p.into())),
        }
    }
}

impl From<rdbms_types::DbColumnTypeMeta> for DbColumnTypeMeta {
    fn from(value: rdbms_types::DbColumnTypeMeta) -> Self {
        Self {
            name: value.name,
            db_type: value.db_type.into(),
            db_type_flags: value
                .db_type_flags
                .iter()
                .fold(DbColumnTypeFlags::empty(), |a, b| a | b.clone().into()),
            foreign_key: value.foreign_key,
        }
    }
}

impl From<rdbms_types::DbColumnTypeFlag> for DbColumnTypeFlags {
    fn from(value: rdbms_types::DbColumnTypeFlag) -> Self {
        match value {
            rdbms_types::DbColumnTypeFlag::PrimaryKey => DbColumnTypeFlags::PRIMARY_KEY,
            rdbms_types::DbColumnTypeFlag::ForeignKey => DbColumnTypeFlags::FOREIGN_KEY,
            rdbms_types::DbColumnTypeFlag::Unique => DbColumnTypeFlags::UNIQUE,
            rdbms_types::DbColumnTypeFlag::Nullable => DbColumnTypeFlags::NULLABLE,
            rdbms_types::DbColumnTypeFlag::Generated => DbColumnTypeFlags::GENERATED,
            rdbms_types::DbColumnTypeFlag::AutoIncrement => DbColumnTypeFlags::AUTO_INCREMENT,
            rdbms_types::DbColumnTypeFlag::DefaultValue => DbColumnTypeFlags::DEFAULT_VALUE,
            rdbms_types::DbColumnTypeFlag::Indexed => DbColumnTypeFlags::INDEXED,
        }
    }
}
