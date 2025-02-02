// Copyright 2024-2025 Golem Cloud
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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::wasi::rdbms::mysql::{
    DbColumn, DbColumnType, DbResult, DbRow, DbValue, Error, Host, HostDbConnection,
    HostDbResultStream, HostDbTransaction,
};
use crate::services::rdbms::mysql::types as mysql_types;
use crate::services::rdbms::mysql::MysqlType;
// use crate::services::rdbms::{Error as RdbmsError};
use crate::services::rdbms::RdbmsPoolKey;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bit_vec::BitVec;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;
// use golem_common::model::oplog::DurableFunctionType;
// use crate::durable_host::serialized::SerializableError;

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

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
        self.observe_function_call("rdbms::mysql::db-connection", "open");

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

    async fn query_stream(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultStreamEntry>, Error>> {
        self.observe_function_call("rdbms::mysql::db-connection", "query-stream");
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
                    .query_stream(&pool_key, &worker_id, &statement, params)
                    .await;

                match result {
                    Ok(result) => {
                        let entry = DbResultStreamEntry::new(result);
                        let resource = self.as_wasi_view().table().push(entry)?;
                        Ok(Ok(resource))
                    }
                    Err(e) => Ok(Err(e.into())),
                }
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    async fn query(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<DbResult, Error>> {
        self.observe_function_call("rdbms::mysql::db-connection", "query");
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
                    .await
                    .map(DbResult::from)
                    .map_err(Error::from);
                Ok(result)
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
        self.observe_function_call("rdbms::mysql::db-connection", "execute");
        let worker_id = self.state.owned_worker_id.worker_id.clone();

        // let durability = Durability::<Result<u64, RdbmsError>, SerializableError>::new(
        //     self,
        //     "golem rdbms::mysql::db-connection",
        //     "execute",
        //     DurableFunctionType::WriteRemote,
        // )
        //     .await?;
        // let result = if durability.is_live() {
        //     let pool_key = self
        //         .as_wasi_view()
        //         .table()
        //         .get::<MysqlDbConnection>(&self_)?
        //         .pool_key
        //         .clone();
        //
        //     let params = params
        //         .into_iter()
        //         .map(|v| v.try_into())
        //         .collect::<Result<Vec<mysql_types::DbValue>, String>>().map_err(RdbmsError::QueryParameterFailure);
        //
        //     let result = match params
        //     {
        //         Ok(params) => {
        //             self
        //                 .state
        //                 .rdbms_service
        //                 .mysql()
        //                 .execute(&pool_key, &worker_id, &statement, params)
        //                 .await
        //         }
        //         Err(error) => Err(error),
        //     };
        //     let input = statement.clone();
        //     durability.persist(self, input, Ok(result)).await
        // } else {
        //     durability.replay(self).await
        // };
        // match result {
        //     Ok(result) => Ok(result.map_err(Error::from)),
        //     Err(error) => {
        //         Ok(Err(error))
        //     }
        // }

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

    async fn begin_transaction(
        &mut self,
        self_: Resource<MysqlDbConnection>,
    ) -> anyhow::Result<Result<Resource<DbTransactionEntry>, Error>> {
        self.observe_function_call("rdbms::mysql::db-connection", "begin-transaction");
        let worker_id = self.state.owned_worker_id.worker_id.clone();
        let pool_key = self
            .as_wasi_view()
            .table()
            .get::<MysqlDbConnection>(&self_)?
            .pool_key
            .clone();

        let result = self
            .state
            .rdbms_service
            .mysql()
            .begin_transaction(&pool_key, &worker_id)
            .await;

        match result {
            Ok(result) => {
                let entry = DbTransactionEntry::new(result);
                let resource = self.as_wasi_view().table().push(entry)?;
                Ok(Ok(resource))
            }
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn drop(&mut self, rep: Resource<MysqlDbConnection>) -> anyhow::Result<()> {
        self.observe_function_call("rdbms::mysql::db-connection", "drop");

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

pub struct DbResultStreamEntry {
    pub internal: Arc<dyn crate::services::rdbms::DbResultStream<MysqlType> + Send + Sync>,
}

impl DbResultStreamEntry {
    pub fn new(
        internal: Arc<dyn crate::services::rdbms::DbResultStream<MysqlType> + Send + Sync>,
    ) -> Self {
        Self { internal }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbResultStream for DurableWorkerCtx<Ctx> {
    async fn get_columns(
        &mut self,
        self_: Resource<DbResultStreamEntry>,
    ) -> anyhow::Result<Vec<DbColumn>> {
        self.observe_function_call("rdbms::mysql::db-result-stream", "get-columns");

        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbResultStreamEntry>(&self_)?
            .internal
            .clone();

        let columns = internal.deref().get_columns().await.map_err(Error::from)?;

        let columns = columns.into_iter().map(|c| c.into()).collect();

        Ok(columns)
    }

    async fn get_next(
        &mut self,
        self_: Resource<DbResultStreamEntry>,
    ) -> anyhow::Result<Option<Vec<DbRow>>> {
        self.observe_function_call("rdbms::mysql::db-result-stream", "get-next");
        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbResultStreamEntry>(&self_)?
            .internal
            .clone();

        let rows = internal.deref().get_next().await.map_err(Error::from)?;

        let rows = rows.map(|r| r.into_iter().map(|r| r.into()).collect());

        Ok(rows)
    }

    async fn drop(&mut self, rep: Resource<DbResultStreamEntry>) -> anyhow::Result<()> {
        self.observe_function_call("rdbms::mysql::db-result-stream", "drop");
        self.as_wasi_view()
            .table()
            .delete::<DbResultStreamEntry>(rep)?;
        Ok(())
    }
}

pub struct DbTransactionEntry {
    pub internal: Arc<dyn crate::services::rdbms::DbTransaction<MysqlType> + Send + Sync>,
}

impl DbTransactionEntry {
    pub fn new(
        internal: Arc<dyn crate::services::rdbms::DbTransaction<MysqlType> + Send + Sync>,
    ) -> Self {
        Self { internal }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDbTransaction for DurableWorkerCtx<Ctx> {
    async fn query(
        &mut self,
        self_: Resource<DbTransactionEntry>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<DbResult, Error>> {
        self.observe_function_call("rdbms::mysql::db-transaction", "query");
        match params
            .into_iter()
            .map(|v| v.try_into())
            .collect::<Result<Vec<_>, String>>()
        {
            Ok(params) => {
                let internal = self
                    .as_wasi_view()
                    .table()
                    .get::<DbTransactionEntry>(&self_)?
                    .internal
                    .clone();
                let result = internal
                    .query(&statement, params)
                    .await
                    .map(DbResult::from)
                    .map_err(Error::from);
                Ok(result)
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    async fn query_stream(
        &mut self,
        self_: Resource<DbTransactionEntry>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultStreamEntry>, Error>> {
        self.observe_function_call("rdbms::mysql::db-transaction", "query-stream");
        match params
            .into_iter()
            .map(|v| v.try_into())
            .collect::<Result<Vec<_>, String>>()
        {
            Ok(params) => {
                let internal = self
                    .as_wasi_view()
                    .table()
                    .get::<DbTransactionEntry>(&self_)?
                    .internal
                    .clone();
                let result = internal.query_stream(&statement, params).await;
                match result {
                    Ok(result) => {
                        let entry = DbResultStreamEntry::new(result);
                        let resource = self.as_wasi_view().table().push(entry)?;
                        Ok(Ok(resource))
                    }
                    Err(e) => Ok(Err(e.into())),
                }
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    async fn execute(
        &mut self,
        self_: Resource<DbTransactionEntry>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        self.observe_function_call("rdbms::mysql::db-transaction", "execute");
        match params
            .into_iter()
            .map(|v| v.try_into())
            .collect::<Result<Vec<_>, String>>()
        {
            Ok(params) => {
                let internal = self
                    .as_wasi_view()
                    .table()
                    .get::<DbTransactionEntry>(&self_)?
                    .internal
                    .clone();
                let result = internal
                    .execute(&statement, params)
                    .await
                    .map_err(Error::from);
                Ok(result)
            }
            Err(error) => Ok(Err(Error::QueryParameterFailure(error))),
        }
    }

    async fn commit(
        &mut self,
        self_: Resource<DbTransactionEntry>,
    ) -> anyhow::Result<Result<(), Error>> {
        self.observe_function_call("rdbms::mysql::db-transaction", "commit");
        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbTransactionEntry>(&self_)?
            .internal
            .clone();
        let result = internal.commit().await.map_err(Error::from);
        Ok(result)
    }

    async fn rollback(
        &mut self,
        self_: Resource<DbTransactionEntry>,
    ) -> anyhow::Result<Result<(), Error>> {
        self.observe_function_call("rdbms::mysql::db-transaction", "query");
        let internal = self
            .as_wasi_view()
            .table()
            .get::<DbTransactionEntry>(&self_)?
            .internal
            .clone();
        let result = internal.rollback().await.map_err(Error::from);
        Ok(result)
    }

    async fn drop(&mut self, rep: Resource<DbTransactionEntry>) -> anyhow::Result<()> {
        self.observe_function_call("rdbms::mysql::db-result-stream", "drop");
        let entry = self
            .as_wasi_view()
            .table()
            .delete::<DbTransactionEntry>(rep)?;
        let _ = entry.internal.rollback_if_open().await;
        Ok(())
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

impl From<crate::services::rdbms::DbRow<MysqlType>> for DbRow {
    fn from(value: crate::services::rdbms::DbRow<MysqlType>) -> Self {
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
            mysql_types::DbColumnType::Time => Self::Time,
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

impl From<DbColumnType> for mysql_types::DbColumnType {
    fn from(value: DbColumnType) -> Self {
        match value {
            DbColumnType::Boolean => Self::Boolean,
            DbColumnType::Tinyint => Self::Tinyint,
            DbColumnType::Smallint => Self::Smallint,
            DbColumnType::Mediumint => Self::Mediumint,
            DbColumnType::Int => Self::Int,
            DbColumnType::Bigint => Self::Bigint,
            DbColumnType::IntUnsigned => Self::IntUnsigned,
            DbColumnType::TinyintUnsigned => Self::TinyintUnsigned,
            DbColumnType::SmallintUnsigned => Self::SmallintUnsigned,
            DbColumnType::MediumintUnsigned => Self::MediumintUnsigned,
            DbColumnType::BigintUnsigned => Self::BigintUnsigned,
            DbColumnType::Float => Self::Float,
            DbColumnType::Double => Self::Double,
            DbColumnType::Decimal => Self::Decimal,
            DbColumnType::Text => Self::Text,
            DbColumnType::Varchar => Self::Varchar,
            DbColumnType::Fixchar => Self::Fixchar,
            DbColumnType::Blob => Self::Blob,
            DbColumnType::Json => Self::Json,
            DbColumnType::Timestamp => Self::Timestamp,
            DbColumnType::Date => Self::Date,
            DbColumnType::Time => Self::Time,
            DbColumnType::Datetime => Self::Datetime,
            DbColumnType::Year => Self::Year,
            DbColumnType::Bit => Self::Bit,
            DbColumnType::Binary => Self::Binary,
            DbColumnType::Varbinary => Self::Varbinary,
            DbColumnType::Tinyblob => Self::Tinyblob,
            DbColumnType::Mediumblob => Self::Mediumblob,
            DbColumnType::Longblob => Self::Longblob,
            DbColumnType::Tinytext => Self::Tinytext,
            DbColumnType::Mediumtext => Self::Mediumtext,
            DbColumnType::Longtext => Self::Longtext,
            DbColumnType::Enumeration => Self::Enumeration,
            DbColumnType::Set => Self::Set,
        }
    }
}

impl From<crate::services::rdbms::DbResult<MysqlType>> for DbResult {
    fn from(value: crate::services::rdbms::DbResult<MysqlType>) -> Self {
        Self {
            columns: value.columns.into_iter().map(|v| v.into()).collect(),
            rows: value.rows.into_iter().map(|v| v.into()).collect(),
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

#[cfg(test)]
pub mod tests {
    use crate::preview2::wasi::rdbms::mysql::{DbColumnType, DbValue};
    use crate::services::rdbms::mysql::types as mysql_types;
    use assert2::check;
    use bigdecimal::BigDecimal;
    use bit_vec::BitVec;
    use serde_json::json;
    use std::str::FromStr;
    use test_r::test;
    use uuid::Uuid;

    fn check_db_value(value: mysql_types::DbValue) {
        let wit: DbValue = value.clone().into();
        let value2: mysql_types::DbValue = wit.try_into().unwrap();
        check!(value == value2);
    }

    #[test]
    fn test_db_values_conversions() {
        let params = vec![
            mysql_types::DbValue::Tinyint(1),
            mysql_types::DbValue::Smallint(2),
            mysql_types::DbValue::Mediumint(3),
            mysql_types::DbValue::Int(4),
            mysql_types::DbValue::Bigint(5),
            mysql_types::DbValue::Float(6.0),
            mysql_types::DbValue::Double(7.0),
            mysql_types::DbValue::Decimal(BigDecimal::from_str("80.00").unwrap()),
            mysql_types::DbValue::Date(chrono::NaiveDate::from_ymd_opt(2030, 10, 12).unwrap()),
            mysql_types::DbValue::Datetime(chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
                chrono::Utc,
            )),
            mysql_types::DbValue::Timestamp(chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
                chrono::Utc,
            )),
            mysql_types::DbValue::Fixchar("0123456789".to_string()),
            mysql_types::DbValue::Varchar(format!("name-{}", Uuid::new_v4())),
            mysql_types::DbValue::Tinytext("Tinytext".to_string()),
            mysql_types::DbValue::Text("text".to_string()),
            mysql_types::DbValue::Mediumtext("Mediumtext".to_string()),
            mysql_types::DbValue::Longtext("Longtext".to_string()),
            mysql_types::DbValue::Binary(vec![66, 105, 110, 97, 114, 121]),
            mysql_types::DbValue::Varbinary("Varbinary".as_bytes().to_vec()),
            mysql_types::DbValue::Tinyblob("Tinyblob".as_bytes().to_vec()),
            mysql_types::DbValue::Blob("Blob".as_bytes().to_vec()),
            mysql_types::DbValue::Mediumblob("Mediumblob".as_bytes().to_vec()),
            mysql_types::DbValue::Longblob("Longblob".as_bytes().to_vec()),
            mysql_types::DbValue::Enumeration("value2".to_string()),
            mysql_types::DbValue::Set("value1,value2".to_string()),
            mysql_types::DbValue::Json(json!(
                   {
                      "id": 100
                   }
            )),
            mysql_types::DbValue::Bit(BitVec::from_iter([true, false, false])),
            mysql_types::DbValue::TinyintUnsigned(10),
            mysql_types::DbValue::SmallintUnsigned(20),
            mysql_types::DbValue::MediumintUnsigned(30),
            mysql_types::DbValue::IntUnsigned(40),
            mysql_types::DbValue::BigintUnsigned(50),
            mysql_types::DbValue::Year(2020),
            mysql_types::DbValue::Time(chrono::NaiveTime::from_hms_opt(1, 20, 30).unwrap()),
        ];

        for param in params {
            check_db_value(param);
        }
    }

    fn check_db_column_type(value: mysql_types::DbColumnType) {
        let wit: DbColumnType = value.clone().into();
        let value2: mysql_types::DbColumnType = wit.into();
        check!(value == value2);
    }

    #[test]
    fn test_db_column_types_conversions() {
        let values = vec![
            mysql_types::DbColumnType::Boolean,
            mysql_types::DbColumnType::Tinyint,
            mysql_types::DbColumnType::Smallint,
            mysql_types::DbColumnType::Mediumint,
            mysql_types::DbColumnType::Int,
            mysql_types::DbColumnType::Bigint,
            mysql_types::DbColumnType::TinyintUnsigned,
            mysql_types::DbColumnType::SmallintUnsigned,
            mysql_types::DbColumnType::MediumintUnsigned,
            mysql_types::DbColumnType::IntUnsigned,
            mysql_types::DbColumnType::BigintUnsigned,
            mysql_types::DbColumnType::Float,
            mysql_types::DbColumnType::Double,
            mysql_types::DbColumnType::Decimal,
            mysql_types::DbColumnType::Date,
            mysql_types::DbColumnType::Datetime,
            mysql_types::DbColumnType::Timestamp,
            mysql_types::DbColumnType::Time,
            mysql_types::DbColumnType::Year,
            mysql_types::DbColumnType::Fixchar,
            mysql_types::DbColumnType::Varchar,
            mysql_types::DbColumnType::Tinytext,
            mysql_types::DbColumnType::Text,
            mysql_types::DbColumnType::Mediumtext,
            mysql_types::DbColumnType::Longtext,
            mysql_types::DbColumnType::Binary,
            mysql_types::DbColumnType::Varbinary,
            mysql_types::DbColumnType::Tinyblob,
            mysql_types::DbColumnType::Blob,
            mysql_types::DbColumnType::Mediumblob,
            mysql_types::DbColumnType::Longblob,
            mysql_types::DbColumnType::Enumeration,
            mysql_types::DbColumnType::Set,
            mysql_types::DbColumnType::Bit,
            mysql_types::DbColumnType::Json,
        ];

        for value in values {
            check_db_column_type(value);
        }
    }
}
