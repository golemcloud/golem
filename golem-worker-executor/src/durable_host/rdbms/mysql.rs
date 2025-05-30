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
use crate::durable_host::DurableWorkerCtx;
use crate::preview2::golem::rdbms::mysql::{
    DbColumn, DbColumnType, DbResult, DbRow, DbValue, Error, Host, HostDbConnection,
    HostDbResultStream, HostDbTransaction,
};
use crate::services::rdbms::mysql::types as mysql_types;
use crate::services::rdbms::mysql::MysqlType;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bit_vec::BitVec;
use std::str::FromStr;
use wasmtime::component::{Resource, ResourceTable};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub type MysqlDbConnection = RdbmsConnection<MysqlType>;

impl<Ctx: WorkerCtx> HostDbConnection for DurableWorkerCtx<Ctx> {
    async fn open(
        &mut self,
        address: String,
    ) -> anyhow::Result<Result<Resource<MysqlDbConnection>, Error>> {
        open_db_connection(address, self).await
    }

    async fn query_stream(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<Resource<DbResultStreamEntry>, Error>> {
        db_connection_durable_query_stream(statement, params, self, &self_).await
    }

    async fn query(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<DbResult, Error>> {
        db_connection_durable_query(statement, params, self, &self_).await
    }

    async fn execute(
        &mut self,
        self_: Resource<MysqlDbConnection>,
        statement: String,
        params: Vec<DbValue>,
    ) -> anyhow::Result<Result<u64, Error>> {
        db_connection_durable_execute(statement, params, self, &self_).await
    }

    async fn begin_transaction(
        &mut self,
        self_: Resource<MysqlDbConnection>,
    ) -> anyhow::Result<Result<Resource<DbTransactionEntry>, Error>> {
        begin_db_transaction(self, &self_).await
    }

    async fn drop(&mut self, rep: Resource<MysqlDbConnection>) -> anyhow::Result<()> {
        db_connection_drop(self, rep).await
    }
}

pub type DbResultStreamEntry = RdbmsResultStreamEntry<MysqlType>;

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

pub type DbTransactionEntry = RdbmsTransactionEntry<MysqlType>;

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
            DbValue::Decimal(v) => {
                let v = bigdecimal::BigDecimal::from_str(&v).map_err(|e| e.to_string())?;
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
            DbValue::Json(v) => Ok(Self::Json(v)),
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

impl FromRdbmsValue<crate::services::rdbms::DbRow<mysql_types::DbValue>> for DbRow {
    fn from(
        value: crate::services::rdbms::DbRow<mysql_types::DbValue>,
        _resource_table: &mut ResourceTable,
    ) -> Result<DbRow, String> {
        Ok(value.into())
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

impl FromRdbmsValue<crate::services::rdbms::DbResult<MysqlType>> for DbResult {
    fn from(
        value: crate::services::rdbms::DbResult<MysqlType>,
        _resource_table: &mut ResourceTable,
    ) -> Result<DbResult, String> {
        Ok(value.into())
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

impl FromRdbmsValue<mysql_types::DbColumn> for DbColumn {
    fn from(
        value: mysql_types::DbColumn,
        _resource_table: &mut ResourceTable,
    ) -> Result<DbColumn, String> {
        Ok(value.into())
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

impl FromRdbmsValue<DbValue> for mysql_types::DbValue {
    fn from(value: DbValue, _resource_table: &mut ResourceTable) -> Result<Self, String> {
        value.try_into()
    }
}

impl FromRdbmsValue<mysql_types::DbValue> for DbValue {
    fn from(
        value: mysql_types::DbValue,
        _resource_table: &mut ResourceTable,
    ) -> Result<DbValue, String> {
        Ok(value.into())
    }
}

#[cfg(test)]
pub mod tests {
    use crate::preview2::golem::rdbms::mysql::{DbColumnType, DbValue};
    use crate::services::rdbms::mysql::types as mysql_types;
    use assert2::check;
    use test_r::test;

    fn check_db_value(value: mysql_types::DbValue) {
        let wit: DbValue = value.clone().into();
        let value2: mysql_types::DbValue = wit.try_into().unwrap();
        check!(value2 == value);
    }

    #[test]
    fn test_db_values_conversions() {
        let values = mysql_types::tests::get_test_db_values();

        for value in values {
            check_db_value(value);
        }
    }

    fn check_db_column_type(value: mysql_types::DbColumnType) {
        let wit: DbColumnType = value.clone().into();
        let value2: mysql_types::DbColumnType = wit.into();
        check!(value2 == value);
    }

    #[test]
    fn test_db_column_types_conversions() {
        let values = mysql_types::tests::get_test_db_column_types();

        for value in values {
            check_db_column_type(value);
        }
    }
}
