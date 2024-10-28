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
    DbValuePrimitive, HostDbResultSet,
};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

pub struct DbResultSetEntry {
    pub rdbms_type: RdbmsType,
    pub statement: String,
    pub params: Vec<DbValue>,
    pub columns: Vec<DbColumnTypeMeta>,
    pub rows: Option<Vec<DbRow>>,
}

impl DbResultSetEntry {
    pub fn new(
        rdbms_type: RdbmsType,
        statement: String,
        params: Vec<DbValue>,
        columns: Vec<DbColumnTypeMeta>,
        rows: Option<Vec<DbRow>>,
    ) -> Self {
        Self {
            rdbms_type,
            statement,
            params,
            columns,
            rows,
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultSet for &mut DurableWorkerCtx<Ctx> {
    async fn get_column_metadata(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Vec<DbColumnTypeMeta>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-column-metadata");
        let columns = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)
            .map(|e| e.columns.clone())?;

        Ok(columns)
    }

    async fn get_next(
        &mut self,
        self_: Resource<DbResultSetEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        let _permit = self.begin_async_host_function().await?;
        record_host_function_call("rdbms::types::db-result-set", "get-next");
        let rows = self
            .as_wasi_view()
            .table()
            .get::<DbResultSetEntry>(&self_)
            .map(|e| e.rows.clone())?;

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

impl From<DbValuePrimitive> for crate::services::rdbms::DbValuePrimitive {
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

impl From<crate::services::rdbms::DbValuePrimitive> for DbValuePrimitive {
    fn from(value: crate::services::rdbms::DbValuePrimitive) -> Self {
        match value {
            crate::services::rdbms::DbValuePrimitive::Integer(i) => Self::Integer(i),
            crate::services::rdbms::DbValuePrimitive::Decimal(s) => Self::Decimal(s),
            crate::services::rdbms::DbValuePrimitive::Float(f) => Self::Float(f),
            crate::services::rdbms::DbValuePrimitive::Boolean(b) => Self::Boolean(b),
            crate::services::rdbms::DbValuePrimitive::Datetime(u) => Self::Datetime(u),
            crate::services::rdbms::DbValuePrimitive::Interval(u) => Self::Interval(u),
            crate::services::rdbms::DbValuePrimitive::Chars(u) => Self::Chars(u),
            crate::services::rdbms::DbValuePrimitive::Text(s) => Self::Text(s),
            crate::services::rdbms::DbValuePrimitive::Binary(u) => Self::Binary(u),
            crate::services::rdbms::DbValuePrimitive::Blob(u) => Self::Blob(u),
            crate::services::rdbms::DbValuePrimitive::Enumeration(v) => Self::Enumeration(v),
            crate::services::rdbms::DbValuePrimitive::Json(s) => Self::Json(s),
            crate::services::rdbms::DbValuePrimitive::Xml(s) => Self::Xml(s),
            crate::services::rdbms::DbValuePrimitive::Uuid(uuid) => Self::Uuid(uuid.as_u64_pair()),
            crate::services::rdbms::DbValuePrimitive::Spatial(v) => Self::Spatial(v),
            crate::services::rdbms::DbValuePrimitive::Other(n, v) => Self::Other((n, v)),
            crate::services::rdbms::DbValuePrimitive::DbNull => Self::DbNull,
        }
    }
}

impl From<DbValue> for crate::services::rdbms::DbValue {
    fn from(value: DbValue) -> Self {
        match value {
            DbValue::Primitive(p) => Self::Primitive(p.into()),
            DbValue::Array(vs) => Self::Array(vs.into_iter().map(|v| v.into()).collect()),
        }
    }
}

impl From<crate::services::rdbms::DbValue> for DbValue {
    fn from(value: crate::services::rdbms::DbValue) -> Self {
        match value {
            crate::services::rdbms::DbValue::Primitive(p) => Self::Primitive(p.into()),
            crate::services::rdbms::DbValue::Array(vs) => {
                Self::Array(vs.into_iter().map(|v| v.into()).collect())
            }
        }
    }
}

impl From<crate::services::rdbms::DbColumnTypePrimitive> for DbColumnTypePrimitive {
    fn from(value: crate::services::rdbms::DbColumnTypePrimitive) -> Self {
        match value {
            crate::services::rdbms::DbColumnTypePrimitive::Integer(s) => Self::Integer(s),
            crate::services::rdbms::DbColumnTypePrimitive::Decimal(p, s) => Self::Decimal((p, s)),
            crate::services::rdbms::DbColumnTypePrimitive::Float => Self::Float,
            crate::services::rdbms::DbColumnTypePrimitive::Boolean => Self::Boolean,
            crate::services::rdbms::DbColumnTypePrimitive::Datetime => Self::Datetime,
            crate::services::rdbms::DbColumnTypePrimitive::Interval => Self::Interval,
            crate::services::rdbms::DbColumnTypePrimitive::Chars(s) => Self::Chars(s),
            crate::services::rdbms::DbColumnTypePrimitive::Text => Self::Text,
            crate::services::rdbms::DbColumnTypePrimitive::Binary(s) => Self::Binary(s),
            crate::services::rdbms::DbColumnTypePrimitive::Blob => Self::Blob,
            crate::services::rdbms::DbColumnTypePrimitive::Enumeration(vs) => Self::Enumeration(vs),
            crate::services::rdbms::DbColumnTypePrimitive::Json => Self::Json,
            crate::services::rdbms::DbColumnTypePrimitive::Xml => Self::Xml,
            crate::services::rdbms::DbColumnTypePrimitive::Uuid => Self::Uuid,
            crate::services::rdbms::DbColumnTypePrimitive::Spatial => Self::Spatial,
        }
    }
}

impl From<crate::services::rdbms::DbColumnType> for DbColumnType {
    fn from(value: crate::services::rdbms::DbColumnType) -> Self {
        match value {
            crate::services::rdbms::DbColumnType::Primitive(p) => Self::Primitive(p.into()),
            crate::services::rdbms::DbColumnType::Array(vs, p) => Self::Array((vs, p.into())),
        }
    }
}

impl From<crate::services::rdbms::DbColumnTypeMeta> for DbColumnTypeMeta {
    fn from(value: crate::services::rdbms::DbColumnTypeMeta) -> Self {
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

impl From<crate::services::rdbms::DbColumnTypeFlag> for DbColumnTypeFlags {
    fn from(value: crate::services::rdbms::DbColumnTypeFlag) -> Self {
        match value {
            crate::services::rdbms::DbColumnTypeFlag::PrimaryKey => DbColumnTypeFlags::PRIMARY_KEY,
            crate::services::rdbms::DbColumnTypeFlag::ForeignKey => DbColumnTypeFlags::FOREIGN_KEY,
            crate::services::rdbms::DbColumnTypeFlag::Unique => DbColumnTypeFlags::UNIQUE,
            crate::services::rdbms::DbColumnTypeFlag::Nullable => DbColumnTypeFlags::NULLABLE,
            crate::services::rdbms::DbColumnTypeFlag::Generated => DbColumnTypeFlags::GENERATED,
            crate::services::rdbms::DbColumnTypeFlag::AutoIncrement => {
                DbColumnTypeFlags::AUTO_INCREMENT
            }
            crate::services::rdbms::DbColumnTypeFlag::DefaultValue => {
                DbColumnTypeFlags::DEFAULT_VALUE
            }
            crate::services::rdbms::DbColumnTypeFlag::Indexed => DbColumnTypeFlags::INDEXED,
        }
    }
}
