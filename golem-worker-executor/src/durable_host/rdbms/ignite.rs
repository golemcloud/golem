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
    FromRdbmsValue, RdbmsConnection, RdbmsDurabilityPairs, RdbmsResultStreamEntry,
    RdbmsTransactionEntry,
};
use crate::durable_host::DurableWorkerCtx;
use crate::preview2::golem::rdbms::ignite2::{
    DbColumn, DbResult, DbRow, DbValue, Error, Host, HostDbConnection, HostDbResultStream,
    HostDbTransaction,
};
use crate::services::rdbms::ignite::types as ignite_types;
use crate::services::rdbms::ignite::IgniteType;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::*;
use std::str::FromStr;
use wasmtime::component::{Resource, ResourceTable};

impl RdbmsDurabilityPairs for IgniteType {
    type ConnExecute = RdbmsIgnite2DbConnectionExecute;
    type ConnQuery = RdbmsIgnite2DbConnectionQuery;
    type ConnQueryStream = RdbmsIgnite2DbConnectionQueryStream;
    type TxnExecute = RdbmsIgnite2DbTransactionExecute;
    type TxnQuery = RdbmsIgnite2DbTransactionQuery;
    type TxnQueryStream = RdbmsIgnite2DbTransactionQueryStream;
    type StreamGetColumns = RdbmsIgnite2DbResultStreamGetColumns;
    type StreamGetNext = RdbmsIgnite2DbResultStreamGetNext;
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub type Ignite2DbConnection = RdbmsConnection<IgniteType>;

impl<Ctx: WorkerCtx> HostDbConnection for DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<Ignite2DbConnection>, Error>> {
        open_db_connection(address, self).await
    }

    async fn query(
        &mut self,
        self_: Resource<Ignite2DbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<DbResult, Error>> {
        db_connection_durable_query(statement, params, self, &self_).await
    }

    async fn query_stream(
        &mut self,
        self_: Resource<Ignite2DbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultStreamEntry>, Error>> {
        db_connection_durable_query_stream(statement, params, self, &self_).await
    }

    async fn execute(
        &mut self,
        self_: Resource<Ignite2DbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<i64, Error>> {
        let result: anyhow::Result<Result<u64, Error>> =
            db_connection_durable_execute(statement, params, self, &self_).await;
        result.map(|r| r.map(|n| n as i64))
    }

    async fn begin_transaction(
        &mut self,
        self_: Resource<Ignite2DbConnection>,
    ) -> anyhow::Result<Result<Resource<DbTransactionEntry>, Error>> {
        begin_db_transaction(self, &self_).await
    }

    async fn drop(&mut self, rep: Resource<Ignite2DbConnection>) -> anyhow::Result<()> {
        db_connection_drop(self, rep).await
    }
}

pub type DbResultStreamEntry = RdbmsResultStreamEntry<IgniteType>;

impl<Ctx: WorkerCtx> HostDbResultStream for DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<DbResultStreamEntry>,
    ) -> anyhow::Result<Result<Vec<DbColumn>, Error>> {
        let cols: anyhow::Result<Vec<DbColumn>> =
            db_result_stream_durable_get_columns(self, &self_).await;
        cols.map(|v| Ok(v))
    }

    async fn get_next(
        &mut self,
        self_: Resource<DbResultStreamEntry>,
    ) -> anyhow::Result<Result<Option<Vec<DbRow>>, Error>> {
        let rows: anyhow::Result<Option<Vec<DbRow>>> =
            db_result_stream_durable_get_next(self, &self_).await;
        rows.map(|v| Ok(v))
    }

    async fn drop(&mut self, rep: Resource<DbResultStreamEntry>) -> anyhow::Result<()> {
        db_result_stream_drop(self, rep).await
    }
}

pub type DbTransactionEntry = RdbmsTransactionEntry<IgniteType>;

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
    ) -> anyhow::Result<Result<i64, Error>> {
        let result: anyhow::Result<Result<u64, Error>> =
            db_transaction_durable_execute(statement, params, self, &self_).await;
        result.map(|r| r.map(|n| n as i64))
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

// ── FromRdbmsValue impls ──────────────────────────────────────────────────────

// WIT params → service DbValue (used by durable execute/query helpers)
impl FromRdbmsValue<DbValue> for ignite_types::DbValue {
    fn from(value: DbValue, _resource_table: &mut ResourceTable) -> Result<Self, String> {
        value.try_into()
    }
}

// Service DbColumn → WIT DbColumn (used by get_columns helper)
impl FromRdbmsValue<ignite_types::DbColumn> for DbColumn {
    fn from(
        value: ignite_types::DbColumn,
        _resource_table: &mut ResourceTable,
    ) -> Result<Self, String> {
        Ok(value.into())
    }
}

// ── Error conversion ──────────────────────────────────────────────────────────

impl From<crate::services::rdbms::RdbmsError> for Error {
    fn from(value: crate::services::rdbms::RdbmsError) -> Self {
        match value {
            crate::services::rdbms::RdbmsError::ConnectionFailure(v) => Self::ConnectionFailure(v),
            crate::services::rdbms::RdbmsError::QueryParameterFailure(v) => {
                Self::QueryParameterFailure(v)
            }
            crate::services::rdbms::RdbmsError::QueryExecutionFailure(v) => {
                Self::QueryExecutionFailure(v)
            }
            crate::services::rdbms::RdbmsError::QueryResponseFailure(v) => {
                Self::QueryResponseFailure(v)
            }
            crate::services::rdbms::RdbmsError::Other(v) => Self::Other(v),
        }
    }
}

// ── WIT DbValue ↔ service DbValue ─────────────────────────────────────────────

impl TryFrom<DbValue> for ignite_types::DbValue {
    type Error = String;

    fn try_from(value: DbValue) -> Result<Self, Self::Error> {
        match value {
            DbValue::DbNull => Ok(Self::Null),
            DbValue::DbBoolean(v) => Ok(Self::Boolean(v)),
            DbValue::DbByte(v) => Ok(Self::Byte(v)),
            DbValue::DbShort(v) => Ok(Self::Short(v)),
            DbValue::DbInt(v) => Ok(Self::Int(v)),
            DbValue::DbLong(v) => Ok(Self::Long(v)),
            DbValue::DbFloat(v) => Ok(Self::Float(v)),
            DbValue::DbDouble(v) => Ok(Self::Double(v)),
            DbValue::DbChar(v) => Ok(Self::Char(v)),
            DbValue::DbString(v) => Ok(Self::String(v)),
            DbValue::DbUuid((hi, lo)) => Ok(Self::Uuid(uuid::Uuid::from_u64_pair(hi, lo))),
            DbValue::DbDate(ms) => Ok(Self::Date(ms)),
            DbValue::DbTimestamp((ms, ns)) => Ok(Self::Timestamp(ms, ns)),
            DbValue::DbTime(ns) => {
                // WIT uses nanoseconds since midnight; service layer uses ms
                Ok(Self::Time(ns / 1_000_000))
            }
            DbValue::DbDecimal(v) => {
                let bd = bigdecimal::BigDecimal::from_str(&v).map_err(|e| e.to_string())?;
                Ok(Self::Decimal(bd))
            }
            DbValue::DbByteArray(v) => Ok(Self::ByteArray(v)),
        }
    }
}

impl From<ignite_types::DbValue> for DbValue {
    fn from(value: ignite_types::DbValue) -> Self {
        match value {
            ignite_types::DbValue::Null => Self::DbNull,
            ignite_types::DbValue::Boolean(v) => Self::DbBoolean(v),
            ignite_types::DbValue::Byte(v) => Self::DbByte(v),
            ignite_types::DbValue::Short(v) => Self::DbShort(v),
            ignite_types::DbValue::Int(v) => Self::DbInt(v),
            ignite_types::DbValue::Long(v) => Self::DbLong(v),
            ignite_types::DbValue::Float(v) => Self::DbFloat(v),
            ignite_types::DbValue::Double(v) => Self::DbDouble(v),
            ignite_types::DbValue::Char(v) => Self::DbChar(v),
            ignite_types::DbValue::String(v) => Self::DbString(v),
            ignite_types::DbValue::Uuid(u) => {
                let (hi, lo) = u.as_u64_pair();
                Self::DbUuid((hi, lo))
            }
            ignite_types::DbValue::Date(ms) => Self::DbDate(ms),
            ignite_types::DbValue::Timestamp(ms, ns) => Self::DbTimestamp((ms, ns)),
            ignite_types::DbValue::Time(ms) => {
                // service layer uses ms; WIT uses nanoseconds
                Self::DbTime(ms * 1_000_000)
            }
            ignite_types::DbValue::Decimal(bd) => Self::DbDecimal(bd.to_string()),
            ignite_types::DbValue::ByteArray(v) => Self::DbByteArray(v),
        }
    }
}

// ── DbColumn ──────────────────────────────────────────────────────────────────

impl From<ignite_types::DbColumn> for DbColumn {
    fn from(value: ignite_types::DbColumn) -> Self {
        Self {
            ordinal: value.ordinal as u64,
            name: value.name,
            db_type_name: "unknown".to_string(),
        }
    }
}

// ── DbRow ─────────────────────────────────────────────────────────────────────

impl FromRdbmsValue<crate::services::rdbms::DbRow<ignite_types::DbValue>> for DbRow {
    fn from(
        value: crate::services::rdbms::DbRow<ignite_types::DbValue>,
        _resource_table: &mut ResourceTable,
    ) -> Result<DbRow, String> {
        Ok(value.into())
    }
}

impl From<crate::services::rdbms::DbRow<ignite_types::DbValue>> for DbRow {
    fn from(value: crate::services::rdbms::DbRow<ignite_types::DbValue>) -> Self {
        Self {
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

// ── DbResult ──────────────────────────────────────────────────────────────────

impl FromRdbmsValue<crate::services::rdbms::DbResult<IgniteType>> for DbResult {
    fn from(
        value: crate::services::rdbms::DbResult<IgniteType>,
        _resource_table: &mut ResourceTable,
    ) -> Result<DbResult, String> {
        Ok(DbResult {
            columns: value.columns.into_iter().map(|c| c.into()).collect(),
            rows: value
                .rows
                .into_iter()
                .map(|r| DbRow {
                    values: r.values.into_iter().map(|v| v.into()).collect(),
                })
                .collect(),
        })
    }
}

