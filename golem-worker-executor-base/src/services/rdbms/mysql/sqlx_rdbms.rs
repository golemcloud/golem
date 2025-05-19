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

use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
use crate::services::rdbms::mysql::types::{DbColumn, DbColumnType, DbValue};
use crate::services::rdbms::mysql::{MysqlType, MYSQL};
use crate::services::rdbms::sqlx_common::{
    create_db_result, PoolCreator, QueryExecutor, QueryParamsBinder, SqlxDbResultStream, SqlxRdbms,
};
use crate::services::rdbms::{DbResult, DbResultStream, DbRow, Error, Rdbms, RdbmsPoolKey};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use futures_util::stream::BoxStream;
use sqlx::{Column, ConnectOptions, Pool, Row, TypeInfo};
use std::sync::Arc;

pub(crate) fn new(config: RdbmsConfig) -> Arc<dyn Rdbms<MysqlType> + Send + Sync> {
    let sqlx: SqlxRdbms<MysqlType, sqlx::mysql::MySql> = SqlxRdbms::new(config);
    Arc::new(sqlx)
}

#[async_trait]
impl PoolCreator<sqlx::MySql> for RdbmsPoolKey {
    async fn create_pool(&self, config: &RdbmsPoolConfig) -> Result<Pool<sqlx::MySql>, Error> {
        if self.address.scheme() != MYSQL {
            Err(Error::ConnectionFailure(format!(
                "scheme '{}' in url is invalid",
                self.address.scheme()
            )))?
        }
        let options = sqlx::mysql::MySqlConnectOptions::from_url(&self.address)
            .map_err(Error::connection_failure)?;
        sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(config.max_connections)
            .connect_with(options)
            .await
            .map_err(Error::connection_failure)
    }
}

#[async_trait]
impl QueryExecutor<MysqlType, sqlx::MySql> for MysqlType {
    async fn execute<'c, E>(
        statement: &str,
        params: Vec<DbValue>,
        executor: E,
    ) -> Result<u64, Error>
    where
        E: sqlx::Executor<'c, Database = sqlx::MySql>,
    {
        let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
            sqlx::query(statement).bind_params(params)?;

        let result = query
            .execute(executor)
            .await
            .map_err(Error::query_execution_failure)?;
        Ok(result.rows_affected())
    }

    async fn query<'c, E>(
        statement: &str,
        params: Vec<DbValue>,
        executor: E,
    ) -> Result<DbResult<MysqlType>, Error>
    where
        E: sqlx::Executor<'c, Database = sqlx::MySql>,
    {
        let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
            sqlx::query(statement).bind_params(params)?;

        let result = query
            .fetch_all(executor)
            .await
            .map_err(Error::query_execution_failure)?;
        create_db_result::<MysqlType, sqlx::MySql>(result)
    }

    async fn query_stream<'c, E>(
        statement: &str,
        params: Vec<DbValue>,
        batch: usize,
        executor: E,
    ) -> Result<Arc<dyn DbResultStream<MysqlType> + Send + Sync + 'c>, Error>
    where
        E: sqlx::Executor<'c, Database = sqlx::MySql>,
    {
        let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
            sqlx::query(statement.to_string().leak()).bind_params(params)?;

        let stream: BoxStream<Result<sqlx::mysql::MySqlRow, sqlx::Error>> = query.fetch(executor);

        let response: SqlxDbResultStream<'c, MysqlType, sqlx::mysql::MySql> =
            SqlxDbResultStream::create(stream, batch).await?;
        Ok(Arc::new(response))
    }
}

impl<'q> QueryParamsBinder<'q, MysqlType, sqlx::MySql>
    for sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>
{
    fn bind_params(
        mut self,
        params: Vec<DbValue>,
    ) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, Error> {
        for param in params {
            self = bind_value(self, param).map_err(Error::QueryParameterFailure)?;
        }
        Ok(self)
    }
}

fn bind_value(
    query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>,
    value: DbValue,
) -> Result<sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>, String> {
    match value {
        DbValue::Tinyint(v) => Ok(query.bind(v)),
        DbValue::Smallint(v) => Ok(query.bind(v)),
        DbValue::Mediumint(v) => Ok(query.bind(v)),
        DbValue::Int(v) => Ok(query.bind(v)),
        DbValue::Bigint(v) => Ok(query.bind(v)),
        DbValue::TinyintUnsigned(v) => Ok(query.bind(v)),
        DbValue::SmallintUnsigned(v) => Ok(query.bind(v)),
        DbValue::MediumintUnsigned(v) => Ok(query.bind(v)),
        DbValue::IntUnsigned(v) => Ok(query.bind(v)),
        DbValue::BigintUnsigned(v) => Ok(query.bind(v)),
        DbValue::Decimal(v) => {
            // let v = bigdecimal::BigDecimal::from_str(&v).map_err(|e| e.to_string())?;
            Ok(query.bind(v))
        }
        DbValue::Float(v) => Ok(query.bind(v)),
        DbValue::Double(v) => Ok(query.bind(v)),
        DbValue::Boolean(v) => Ok(query.bind(v)),
        DbValue::Text(v) => Ok(query.bind(v)),
        DbValue::Tinytext(v) => Ok(query.bind(v)),
        DbValue::Mediumtext(v) => Ok(query.bind(v)),
        DbValue::Longtext(v) => Ok(query.bind(v)),
        DbValue::Varchar(v) => Ok(query.bind(v)),
        DbValue::Fixchar(v) => Ok(query.bind(v)),
        DbValue::Blob(v) => Ok(query.bind(v)),
        DbValue::Tinyblob(v) => Ok(query.bind(v)),
        DbValue::Mediumblob(v) => Ok(query.bind(v)),
        DbValue::Longblob(v) => Ok(query.bind(v)),
        DbValue::Binary(v) => Ok(query.bind(v)),
        DbValue::Varbinary(v) => Ok(query.bind(v)),
        DbValue::Json(v) => {
            let v: serde_json::Value = serde_json::from_str(&v).map_err(|e| e.to_string())?;
            Ok(query.bind(v))
        }
        DbValue::Timestamp(v) => Ok(query.bind(v)),
        DbValue::Datetime(v) => Ok(query.bind(v)),
        DbValue::Time(v) => Ok(query.bind(v)),
        DbValue::Year(v) => Ok(query.bind(v)),
        DbValue::Date(v) => Ok(query.bind(v)),
        DbValue::Enumeration(v) => Ok(query.bind(v)),
        DbValue::Set(v) => Ok(query.bind(v)),
        DbValue::Bit(v) => {
            let value = bit_vec_to_u64(v).ok_or("Bit vector is too large")?;
            Ok(query.bind(value))
        }
        DbValue::Null => Ok(query.bind(None::<String>)),
    }
}

impl TryFrom<&sqlx::mysql::MySqlRow> for DbRow<DbValue> {
    type Error = String;

    fn try_from(value: &sqlx::mysql::MySqlRow) -> Result<Self, Self::Error> {
        let count = value.len();
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            values.push(get_db_value(index, value)?);
        }
        Ok(DbRow { values })
    }
}

fn get_db_value(index: usize, row: &sqlx::mysql::MySqlRow) -> Result<DbValue, String> {
    let column = &row.columns()[index];
    let db_type: DbColumnType = column.type_info().try_into()?;
    let value = match db_type {
        DbColumnType::Boolean => {
            let v: Option<bool> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Boolean).unwrap_or(DbValue::Null)
        }
        DbColumnType::Tinyint => {
            let v: Option<i8> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Tinyint).unwrap_or(DbValue::Null)
        }
        DbColumnType::Smallint => {
            let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Smallint).unwrap_or(DbValue::Null)
        }
        DbColumnType::Mediumint => {
            let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Mediumint).unwrap_or(DbValue::Null)
        }
        DbColumnType::Int => {
            let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Int).unwrap_or(DbValue::Null)
        }
        DbColumnType::Bigint => {
            let v: Option<i64> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Bigint).unwrap_or(DbValue::Null)
        }
        DbColumnType::TinyintUnsigned => {
            let v: Option<u8> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::TinyintUnsigned).unwrap_or(DbValue::Null)
        }
        DbColumnType::SmallintUnsigned => {
            let v: Option<u16> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::SmallintUnsigned).unwrap_or(DbValue::Null)
        }
        DbColumnType::MediumintUnsigned => {
            let v: Option<u32> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::MediumintUnsigned).unwrap_or(DbValue::Null)
        }
        DbColumnType::IntUnsigned => {
            let v: Option<u32> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::IntUnsigned).unwrap_or(DbValue::Null)
        }
        DbColumnType::BigintUnsigned => {
            let v: Option<u64> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::BigintUnsigned).unwrap_or(DbValue::Null)
        }
        DbColumnType::Float => {
            let v: Option<f32> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Float).unwrap_or(DbValue::Null)
        }
        DbColumnType::Double => {
            let v: Option<f64> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Double).unwrap_or(DbValue::Null)
        }
        DbColumnType::Decimal => {
            let v: Option<BigDecimal> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Decimal).unwrap_or(DbValue::Null)
        }
        DbColumnType::Text => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Text).unwrap_or(DbValue::Null)
        }
        DbColumnType::Varchar => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Varchar).unwrap_or(DbValue::Null)
        }
        DbColumnType::Fixchar => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Fixchar).unwrap_or(DbValue::Null)
        }
        DbColumnType::Tinytext => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Tinytext).unwrap_or(DbValue::Null)
        }
        DbColumnType::Mediumtext => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Mediumtext).unwrap_or(DbValue::Null)
        }
        DbColumnType::Longtext => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Longtext).unwrap_or(DbValue::Null)
        }
        DbColumnType::Json => {
            let v: Option<serde_json::Value> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(|v| DbValue::Json(v.to_string()))
                .unwrap_or(DbValue::Null)
        }
        DbColumnType::Enumeration => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Enumeration).unwrap_or(DbValue::Null)
        }
        DbColumnType::Varbinary => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Varbinary).unwrap_or(DbValue::Null)
        }
        DbColumnType::Binary => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Binary).unwrap_or(DbValue::Null)
        }
        DbColumnType::Blob => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Blob).unwrap_or(DbValue::Null)
        }
        DbColumnType::Tinyblob => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Tinyblob).unwrap_or(DbValue::Null)
        }
        DbColumnType::Mediumblob => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Mediumblob).unwrap_or(DbValue::Null)
        }
        DbColumnType::Longblob => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Longblob).unwrap_or(DbValue::Null)
        }
        DbColumnType::Timestamp => {
            let v: Option<chrono::DateTime<chrono::Utc>> =
                row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Timestamp).unwrap_or(DbValue::Null)
        }
        DbColumnType::Datetime => {
            let v: Option<chrono::DateTime<chrono::Utc>> =
                row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Datetime).unwrap_or(DbValue::Null)
        }
        DbColumnType::Date => {
            let v: Option<chrono::NaiveDate> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Date).unwrap_or(DbValue::Null)
        }
        DbColumnType::Time => {
            let v: Option<chrono::NaiveTime> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Time).unwrap_or(DbValue::Null)
        }
        DbColumnType::Year => {
            let v: Option<u16> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Year).unwrap_or(DbValue::Null)
        }
        DbColumnType::Set => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(DbValue::Set).unwrap_or(DbValue::Null)
        }
        DbColumnType::Bit => {
            let v: Option<u64> = row.try_get(index).map_err(|e| e.to_string())?;
            v.map(|v| DbValue::Bit(u64_to_bit_vec(v)))
                .unwrap_or(DbValue::Null)
        }
    };
    Ok(value)
}

impl TryFrom<&sqlx::mysql::MySqlColumn> for DbColumn {
    type Error = String;

    fn try_from(value: &sqlx::mysql::MySqlColumn) -> Result<Self, Self::Error> {
        let ordinal = value.ordinal() as u64;
        let db_type: DbColumnType = value.type_info().try_into()?;
        let db_type_name = value.type_info().name().to_string();
        let name = value.name().to_string();
        Ok(DbColumn {
            ordinal,
            name,
            db_type,
            db_type_name,
        })
    }
}

impl TryFrom<&sqlx::mysql::MySqlTypeInfo> for DbColumnType {
    type Error = String;

    fn try_from(value: &sqlx::mysql::MySqlTypeInfo) -> Result<Self, Self::Error> {
        let type_name = value.name();

        match type_name {
            mysql_type_name::BOOLEAN => Ok(DbColumnType::Boolean),
            mysql_type_name::TINYINT => Ok(DbColumnType::Tinyint),
            mysql_type_name::SMALLINT => Ok(DbColumnType::Smallint),
            mysql_type_name::MEDIUMINT => Ok(DbColumnType::Mediumint),
            mysql_type_name::INT => Ok(DbColumnType::Int),
            mysql_type_name::BIGINT => Ok(DbColumnType::Bigint),
            mysql_type_name::TINYINT_UNSIGNED => Ok(DbColumnType::TinyintUnsigned),
            mysql_type_name::SMALLINT_UNSIGNED => Ok(DbColumnType::SmallintUnsigned),
            mysql_type_name::MEDIUMINT_UNSIGNED => Ok(DbColumnType::MediumintUnsigned),
            mysql_type_name::INT_UNSIGNED => Ok(DbColumnType::IntUnsigned),
            mysql_type_name::BIGINT_UNSIGNED => Ok(DbColumnType::BigintUnsigned),
            mysql_type_name::DECIMAL => Ok(DbColumnType::Decimal),
            mysql_type_name::FLOAT => Ok(DbColumnType::Float),
            mysql_type_name::DOUBLE => Ok(DbColumnType::Double),
            mysql_type_name::TEXT => Ok(DbColumnType::Text),
            mysql_type_name::TINYTEXT => Ok(DbColumnType::Tinytext),
            mysql_type_name::MEDIUMTEXT => Ok(DbColumnType::Mediumtext),
            mysql_type_name::LONGTEXT => Ok(DbColumnType::Longtext),
            mysql_type_name::VARCHAR => Ok(DbColumnType::Varchar),
            mysql_type_name::CHAR => Ok(DbColumnType::Fixchar),
            mysql_type_name::JSON => Ok(DbColumnType::Json),
            mysql_type_name::TIMESTAMP => Ok(DbColumnType::Timestamp),
            mysql_type_name::DATETIME => Ok(DbColumnType::Datetime),
            mysql_type_name::DATE => Ok(DbColumnType::Date),
            mysql_type_name::TIME => Ok(DbColumnType::Time),
            mysql_type_name::YEAR => Ok(DbColumnType::Year),
            mysql_type_name::VARBINARY => Ok(DbColumnType::Varbinary),
            mysql_type_name::BINARY => Ok(DbColumnType::Binary),
            mysql_type_name::BLOB => Ok(DbColumnType::Blob),
            mysql_type_name::TINYBLOB => Ok(DbColumnType::Tinyblob),
            mysql_type_name::MEDIUMBLOB => Ok(DbColumnType::Mediumblob),
            mysql_type_name::LONGBLOB => Ok(DbColumnType::Longblob),
            mysql_type_name::SET => Ok(DbColumnType::Set),
            mysql_type_name::BIT => Ok(DbColumnType::Bit),
            mysql_type_name::ENUM => Ok(DbColumnType::Enumeration),
            _ => Err(format!("Column type '{}' is not supported", type_name))?,
        }
    }
}

fn bit_vec_to_u64(bits: BitVec) -> Option<u64> {
    if bits.len() > 64 {
        None // Too many bits for u64
    } else {
        let mut result = 0;
        for (i, bit) in bits.iter().enumerate() {
            if bit {
                result |= 1 << (bits.len() - 1 - i);
            }
        }
        Some(result)
    }
}

fn u64_to_bit_vec(num: u64) -> BitVec {
    let mut bits = Vec::new();
    let mut n = num;

    while n > 0 {
        bits.push(n & 1 == 1);
        n >>= 1;
    }

    bits.reverse();
    BitVec::from_iter(bits)
}

/// sqlx_mysql::protocol::text::column::ColumnType is not publicly accessible.
///
#[allow(dead_code)]
pub(crate) mod mysql_type_name {
    pub(crate) const BOOLEAN: &str = "BOOLEAN";
    pub(crate) const TINYINT_UNSIGNED: &str = "TINYINT UNSIGNED";
    pub(crate) const SMALLINT_UNSIGNED: &str = "SMALLINT UNSIGNED";
    pub(crate) const INT_UNSIGNED: &str = "INT UNSIGNED";
    pub(crate) const MEDIUMINT_UNSIGNED: &str = "MEDIUMINT UNSIGNED";
    pub(crate) const BIGINT_UNSIGNED: &str = "BIGINT UNSIGNED";
    pub(crate) const TINYINT: &str = "TINYINT";
    pub(crate) const SMALLINT: &str = "SMALLINT";
    pub(crate) const INT: &str = "INT";
    pub(crate) const MEDIUMINT: &str = "MEDIUMINT";
    pub(crate) const BIGINT: &str = "BIGINT";
    pub(crate) const FLOAT: &str = "FLOAT";
    pub(crate) const DOUBLE: &str = "DOUBLE";
    pub(crate) const NULL: &str = "NULL";
    pub(crate) const TIMESTAMP: &str = "TIMESTAMP";
    pub(crate) const DATE: &str = "DATE";
    pub(crate) const TIME: &str = "TIME";
    pub(crate) const DATETIME: &str = "DATETIME";
    pub(crate) const YEAR: &str = "YEAR";
    pub(crate) const BIT: &str = "BIT";
    pub(crate) const ENUM: &str = "ENUM";
    pub(crate) const SET: &str = "SET";
    pub(crate) const DECIMAL: &str = "DECIMAL";
    pub(crate) const GEOMETRY: &str = "GEOMETRY";
    pub(crate) const JSON: &str = "JSON";

    pub(crate) const BINARY: &str = "BINARY";
    pub(crate) const VARBINARY: &str = "VARBINARY";

    pub(crate) const CHAR: &str = "CHAR";
    pub(crate) const VARCHAR: &str = "VARCHAR";

    pub(crate) const TINYBLOB: &str = "TINYBLOB";
    pub(crate) const TINYTEXT: &str = "TINYTEXT";

    pub(crate) const BLOB: &str = "BLOB";
    pub(crate) const TEXT: &str = "TEXT";

    pub(crate) const MEDIUMBLOB: &str = "MEDIUMBLOB";
    pub(crate) const MEDIUMTEXT: &str = "MEDIUMTEXT";

    pub(crate) const LONGBLOB: &str = "LONGBLOB";
    pub(crate) const LONGTEXT: &str = "LONGTEXT";
}
