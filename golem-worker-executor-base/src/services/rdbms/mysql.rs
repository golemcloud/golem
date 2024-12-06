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

use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::{Rdbms, RdbmsType};
use std::fmt::Display;
use std::sync::Arc;

pub(crate) const MYSQL: &str = "mysql";

#[derive(Debug, Clone, Default)]
pub struct MysqlType;

impl MysqlType {
    pub fn new_rdbms(config: RdbmsConfig) -> Arc<dyn Rdbms<MysqlType> + Send + Sync> {
        sqlx_rdbms::new(config)
    }
}

impl RdbmsType for MysqlType {
    type DbColumn = types::DbColumn;
    type DbValue = types::DbValue;
}

impl Display for MysqlType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", MYSQL)
    }
}

pub(crate) mod sqlx_rdbms {
    use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
    use crate::services::rdbms::mysql::types::{DbColumn, DbColumnType, DbValue};
    use crate::services::rdbms::mysql::{MysqlType, MYSQL};
    use crate::services::rdbms::sqlx_common::{
        PoolCreator, QueryExecutor, QueryParamsBinder, SqlxRdbms, StreamDbResultSet,
    };
    use crate::services::rdbms::{DbResultSet, DbRow, Error, Rdbms, RdbmsPoolKey};
    use async_trait::async_trait;
    use bigdecimal::BigDecimal;
    use futures_util::stream::BoxStream;
    use sqlx::types::BitVec;
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
                .map_err(|e| Error::ConnectionFailure(e.to_string()))?;
            sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(config.max_connections)
                .connect_with(options)
                .await
                .map_err(|e| Error::ConnectionFailure(e.to_string()))
        }
    }

    #[async_trait]
    impl QueryExecutor<MysqlType> for Pool<sqlx::MySql> {
        async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, Error> {
            let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
                sqlx::query(statement).bind_params(params)?;

            let result = query
                .execute(self)
                .await
                .map_err(|e| Error::QueryExecutionFailure(e.to_string()))?;
            Ok(result.rows_affected())
        }

        async fn query_stream(
            &self,
            statement: &str,
            params: Vec<DbValue>,
            batch: usize,
        ) -> Result<Arc<dyn DbResultSet<MysqlType> + Send + Sync>, Error> {
            let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
                sqlx::query(statement.to_string().leak()).bind_params(params)?;

            let stream: BoxStream<Result<sqlx::mysql::MySqlRow, sqlx::Error>> = query.fetch(self);

            let response: StreamDbResultSet<MysqlType, sqlx::mysql::MySql> =
                StreamDbResultSet::create(stream, batch).await?;
            Ok(Arc::new(response))
        }
    }

    impl<'q> QueryParamsBinder<'q, MysqlType, sqlx::MySql>
        for sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>
    {
        fn bind_params(
            mut self,
            params: Vec<DbValue>,
        ) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, Error>
        {
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
            DbValue::Decimal(v) => Ok(query.bind(v)),
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
            DbValue::Json(v) => Ok(query.bind(v)),
            DbValue::Timestamp(v) => Ok(query.bind(v)),
            DbValue::Datetime(v) => Ok(query.bind(v)),
            DbValue::Time(v) => Ok(query.bind(v)),
            DbValue::Year(v) => Ok(query.bind(v)),
            DbValue::Date(v) => Ok(query.bind(v)),
            DbValue::Enumeration(v) => Ok(query.bind(v)),
            DbValue::Set(v) => Ok(query.bind(v)),
            DbValue::Bit(v) => {
                let value = bit_vec_to_u64(v).ok_or("failed to convert bit vector to u64")?;
                Ok(query.bind(value))
            }
            DbValue::Null => Ok(query.bind(None::<String>)),
            // _ => Err(format!("Parameter type '{}' is not supported", value)),
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
        let type_name = column.type_info().name();
        let value = match type_name {
            mysql_type_name::BOOLEAN => {
                let v: Option<bool> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Boolean).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TINYINT => {
                let v: Option<i8> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Tinyint).unwrap_or(DbValue::Null)
            }
            mysql_type_name::SMALLINT => {
                let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Smallint).unwrap_or(DbValue::Null)
            }
            mysql_type_name::MEDIUMINT => {
                let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Mediumint).unwrap_or(DbValue::Null)
            }
            mysql_type_name::INT => {
                let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Int).unwrap_or(DbValue::Null)
            }
            mysql_type_name::BIGINT => {
                let v: Option<i64> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Bigint).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TINYINT_UNSIGNED => {
                let v: Option<u8> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::TinyintUnsigned).unwrap_or(DbValue::Null)
            }
            mysql_type_name::SMALLINT_UNSIGNED => {
                let v: Option<u16> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::SmallintUnsigned).unwrap_or(DbValue::Null)
            }
            mysql_type_name::MEDIUMINT_UNSIGNED => {
                let v: Option<u32> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::MediumintUnsigned).unwrap_or(DbValue::Null)
            }
            mysql_type_name::INT_UNSIGNED => {
                let v: Option<u32> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::IntUnsigned).unwrap_or(DbValue::Null)
            }
            mysql_type_name::BIGINT_UNSIGNED => {
                let v: Option<u64> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::BigintUnsigned).unwrap_or(DbValue::Null)
            }
            mysql_type_name::FLOAT => {
                let v: Option<f32> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Float).unwrap_or(DbValue::Null)
            }
            mysql_type_name::DOUBLE => {
                let v: Option<f64> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Double).unwrap_or(DbValue::Null)
            }
            mysql_type_name::DECIMAL => {
                let v: Option<BigDecimal> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Decimal).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TEXT => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Text).unwrap_or(DbValue::Null)
            }
            mysql_type_name::VARCHAR => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Varchar).unwrap_or(DbValue::Null)
            }
            mysql_type_name::CHAR => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Fixchar).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TINYTEXT => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Tinytext).unwrap_or(DbValue::Null)
            }
            mysql_type_name::MEDIUMTEXT => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Mediumtext).unwrap_or(DbValue::Null)
            }
            mysql_type_name::LONGTEXT => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Longtext).unwrap_or(DbValue::Null)
            }
            mysql_type_name::JSON => {
                let v: Option<serde_json::Value> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Json).unwrap_or(DbValue::Null)
            }
            mysql_type_name::ENUM => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Enumeration).unwrap_or(DbValue::Null)
            }
            mysql_type_name::VARBINARY => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Varbinary).unwrap_or(DbValue::Null)
            }
            mysql_type_name::BINARY => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Binary).unwrap_or(DbValue::Null)
            }
            mysql_type_name::BLOB => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Blob).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TINYBLOB => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Tinyblob).unwrap_or(DbValue::Null)
            }
            mysql_type_name::MEDIUMBLOB => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Mediumblob).unwrap_or(DbValue::Null)
            }
            mysql_type_name::LONGBLOB => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Longblob).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TIMESTAMP => {
                let v: Option<chrono::DateTime<chrono::Utc>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Timestamp).unwrap_or(DbValue::Null)
            }
            mysql_type_name::DATETIME => {
                let v: Option<chrono::DateTime<chrono::Utc>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Datetime).unwrap_or(DbValue::Null)
            }
            mysql_type_name::DATE => {
                let v: Option<chrono::NaiveDate> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Date).unwrap_or(DbValue::Null)
            }
            mysql_type_name::TIME => {
                let v: Option<chrono::NaiveTime> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Time).unwrap_or(DbValue::Null)
            }
            // mysql_type_name::YEAR => { // FIXME
            //     let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
            //     v.map(DbValue::Year).unwrap_or(DbValue::Null)
            // }
            mysql_type_name::SET => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(DbValue::Set).unwrap_or(DbValue::Null)
            }
            mysql_type_name::BIT => {
                let v: Option<u64> = row.try_get(index).map_err(|e| e.to_string())?;
                v.map(|v| {
                    let bv = u64_to_bit_vec(v);
                    DbValue::Bit(bv)
                })
                .unwrap_or(DbValue::Null)
            }
            _ => Err(format!("Value type '{}' is not supported", type_name))?,
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
            return None; // Too many bits for u64
        }
        let mut result = 0;
        for (i, bit) in bits.iter().enumerate() {
            if bit {
                result |= 1 << (bits.len() - 1 - i);
            }
        }
        Some(result)
    }

    fn u64_to_bit_vec(num: u64) -> BitVec {
        let mut bits = Vec::with_capacity(64);
        let mut n = num;

        for _ in 0..64 {
            bits.push(n & 1 == 1);
            n >>= 1;
        }

        bits.reverse();
        let mut vec = BitVec::from_iter(bits);
        vec.shrink_to_fit();
        vec
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
}

pub mod types {
    use bigdecimal::BigDecimal;
    use sqlx::types::BitVec;
    use std::fmt::Display;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum DbColumnType {
        Boolean,
        Tinyint,
        Smallint,
        Mediumint,
        Int,
        Bigint,
        TinyintUnsigned,
        SmallintUnsigned,
        MediumintUnsigned,
        IntUnsigned,
        BigintUnsigned,
        Float,
        Double,
        Decimal,
        Date,
        Datetime,
        Timestamp,
        Time,
        Year,
        Fixchar,
        Varchar,
        Tinytext,
        Text,
        Mediumtext,
        Longtext,
        Binary,
        Varbinary,
        Tinyblob,
        Blob,
        Mediumblob,
        Longblob,
        Enumeration,
        Set,
        Bit,
        Json,
    }

    impl Display for DbColumnType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DbColumnType::Boolean => write!(f, "boolean"),
                DbColumnType::Tinyint => write!(f, "tinyint"),
                DbColumnType::Smallint => write!(f, "smallint"),
                DbColumnType::Mediumint => write!(f, "mediumint"),
                DbColumnType::Int => write!(f, "int"),
                DbColumnType::Bigint => write!(f, "bigint"),
                DbColumnType::TinyintUnsigned => write!(f, "tinyint-unsigned"),
                DbColumnType::SmallintUnsigned => write!(f, "smallint-unsigned"),
                DbColumnType::MediumintUnsigned => write!(f, "mediumunint-signed"),
                DbColumnType::IntUnsigned => write!(f, "int-unsigned"),
                DbColumnType::BigintUnsigned => write!(f, "bigint-unsigned"),
                DbColumnType::Float => write!(f, "float"),
                DbColumnType::Double => write!(f, "double"),
                DbColumnType::Decimal => write!(f, "decimal"),
                DbColumnType::Date => write!(f, "date"),
                DbColumnType::Datetime => write!(f, "datetime"),
                DbColumnType::Timestamp => write!(f, "timestamp"),
                DbColumnType::Time => write!(f, "time"),
                DbColumnType::Year => write!(f, "year"),
                DbColumnType::Fixchar => write!(f, "fixchar"),
                DbColumnType::Varchar => write!(f, "varchar"),
                DbColumnType::Tinytext => write!(f, "tinytext"),
                DbColumnType::Text => write!(f, "text"),
                DbColumnType::Mediumtext => write!(f, "mediumtext"),
                DbColumnType::Longtext => write!(f, "longtext"),
                DbColumnType::Binary => write!(f, "binary"),
                DbColumnType::Varbinary => write!(f, "varbinary"),
                DbColumnType::Tinyblob => write!(f, "tinyblob"),
                DbColumnType::Blob => write!(f, "blob"),
                DbColumnType::Mediumblob => write!(f, "mediumblob"),
                DbColumnType::Longblob => write!(f, "longblob"),
                DbColumnType::Enumeration => write!(f, "enum"),
                DbColumnType::Set => write!(f, "set"),
                DbColumnType::Bit => write!(f, "bit"),
                DbColumnType::Json => write!(f, "json"),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum DbValue {
        Boolean(bool),
        Tinyint(i8),
        Smallint(i16),
        Mediumint(i32),
        /// s24
        Int(i32),
        Bigint(i64),
        TinyintUnsigned(u8),
        SmallintUnsigned(u16),
        MediumintUnsigned(u32),
        /// u24
        IntUnsigned(u32),
        BigintUnsigned(u64),
        Float(f32),
        Double(f64),
        Decimal(BigDecimal),
        Date(chrono::NaiveDate),
        Datetime(chrono::DateTime<chrono::Utc>),
        Timestamp(chrono::DateTime<chrono::Utc>),
        Time(chrono::NaiveTime),
        Year(i16),
        Fixchar(String),
        Varchar(String),
        Tinytext(String),
        Text(String),
        Mediumtext(String),
        Longtext(String),
        Binary(Vec<u8>),
        Varbinary(Vec<u8>),
        Tinyblob(Vec<u8>),
        Blob(Vec<u8>),
        Mediumblob(Vec<u8>),
        Longblob(Vec<u8>),
        Enumeration(String),
        Set(String),
        Bit(BitVec),
        Json(serde_json::Value),
        Null,
    }

    impl Display for DbValue {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DbValue::Boolean(v) => write!(f, "{}", v),
                DbValue::Tinyint(v) => write!(f, "{}", v),
                DbValue::Smallint(v) => write!(f, "{}", v),
                DbValue::Mediumint(v) => write!(f, "{}", v),
                DbValue::Int(v) => write!(f, "{}", v),
                DbValue::Bigint(v) => write!(f, "{}", v),
                DbValue::TinyintUnsigned(v) => write!(f, "{}", v),
                DbValue::SmallintUnsigned(v) => write!(f, "{}", v),
                DbValue::MediumintUnsigned(v) => write!(f, "{}", v),
                DbValue::IntUnsigned(v) => write!(f, "{}", v),
                DbValue::BigintUnsigned(v) => write!(f, "{}", v),
                DbValue::Float(v) => write!(f, "{}", v),
                DbValue::Double(v) => write!(f, "{}", v),
                DbValue::Decimal(v) => write!(f, "{}", v),
                DbValue::Date(v) => write!(f, "{}", v),
                DbValue::Datetime(v) => write!(f, "{}", v),
                DbValue::Timestamp(v) => write!(f, "{}", v),
                DbValue::Time(v) => write!(f, "{}", v),
                DbValue::Year(v) => write!(f, "{}", v),
                DbValue::Fixchar(v) => write!(f, "{}", v),
                DbValue::Varchar(v) => write!(f, "{}", v),
                DbValue::Tinytext(v) => write!(f, "{}", v),
                DbValue::Text(v) => write!(f, "{}", v),
                DbValue::Mediumtext(v) => write!(f, "{}", v),
                DbValue::Longtext(v) => write!(f, "{}", v),
                DbValue::Binary(v) => write!(f, "{:?}", v),
                DbValue::Varbinary(v) => write!(f, "{:?}", v),
                DbValue::Tinyblob(v) => write!(f, "{:?}", v),
                DbValue::Blob(v) => write!(f, "{:?}", v),
                DbValue::Mediumblob(v) => write!(f, "{:?}", v),
                DbValue::Longblob(v) => write!(f, "{:?}", v),
                DbValue::Enumeration(v) => write!(f, "{}", v),
                DbValue::Set(v) => write!(f, "{}", v),
                DbValue::Bit(v) => write!(f, "{:?}", v),
                DbValue::Json(v) => write!(f, "{}", v),
                DbValue::Null => write!(f, "NULL"),
            }
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct DbColumn {
        pub ordinal: u64,
        pub name: String,
        pub db_type: DbColumnType,
        pub db_type_name: String,
    }
}
