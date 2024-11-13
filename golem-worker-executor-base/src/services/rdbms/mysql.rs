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

use crate::services::rdbms::sqlx_common::{
    PoolCreator, QueryExecutor, QueryParamsBinder, SqlxRdbms, StreamDbResultSet,
};
use crate::services::rdbms::types::{
    DbColumn, DbColumnType, DbColumnTypePrimitive, DbResultSet, DbRow, DbValue, DbValuePrimitive,
    Error,
};
use crate::services::rdbms::{RdbmsConfig, RdbmsPoolConfig, RdbmsPoolKey};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use golem_common::model::WorkerId;
use sqlx::{Column, Pool, Row, TypeInfo};
use std::sync::Arc;

#[async_trait]
pub trait Mysql {
    async fn create(&self, worker_id: &WorkerId, address: &str) -> Result<RdbmsPoolKey, Error>;

    fn exists(&self, worker_id: &WorkerId, key: &RdbmsPoolKey) -> bool;

    fn remove(&self, worker_id: &WorkerId, key: &RdbmsPoolKey) -> bool;

    async fn execute(
        &self,
        worker_id: &WorkerId,
        key: &RdbmsPoolKey,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<u64, Error>;

    async fn query(
        &self,
        worker_id: &WorkerId,
        key: &RdbmsPoolKey,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error>;
}

#[derive(Clone)]
pub struct MysqlDefault {
    rdbms: Arc<SqlxRdbms<sqlx::MySql>>,
}

impl MysqlDefault {
    pub fn new(config: RdbmsConfig) -> Self {
        let rdbms = Arc::new(SqlxRdbms::new("mysql", config));
        Self { rdbms }
    }
}

#[async_trait]
impl Mysql for MysqlDefault {
    async fn create(&self, worker_id: &WorkerId, address: &str) -> Result<RdbmsPoolKey, Error> {
        self.rdbms.create(worker_id, address).await
    }

    fn exists(&self, worker_id: &WorkerId, key: &RdbmsPoolKey) -> bool {
        self.rdbms.exists(worker_id, key)
    }

    fn remove(&self, worker_id: &WorkerId, key: &RdbmsPoolKey) -> bool {
        self.rdbms.remove(worker_id, key)
    }

    async fn execute(
        &self,
        worker_id: &WorkerId,
        key: &RdbmsPoolKey,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<u64, Error> {
        self.rdbms.execute(worker_id, key, statement, params).await
    }

    async fn query(
        &self,
        worker_id: &WorkerId,
        key: &RdbmsPoolKey,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
        self.rdbms.query(worker_id, key, statement, params).await
    }
}

impl Default for MysqlDefault {
    fn default() -> Self {
        Self::new(RdbmsConfig::default())
    }
}

#[async_trait]
impl PoolCreator<sqlx::MySql> for RdbmsPoolKey {
    async fn create_pool(
        &self,
        config: &RdbmsPoolConfig,
    ) -> Result<Pool<sqlx::MySql>, sqlx::Error> {
        sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&self.address)
            .await
    }
}

#[async_trait]
impl QueryExecutor for Pool<sqlx::MySql> {
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
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
        let query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments> =
            sqlx::query(statement.to_string().leak()).bind_params(params)?;

        let stream: BoxStream<Result<sqlx::mysql::MySqlRow, sqlx::Error>> = query.fetch(self);

        let response: StreamDbResultSet<sqlx::mysql::MySql> =
            StreamDbResultSet::create(stream, batch).await?;
        Ok(Arc::new(response))
    }
}

impl<'q> QueryParamsBinder<'q, sqlx::MySql>
    for sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>
{
    fn bind_params(
        mut self,
        params: Vec<DbValue>,
    ) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, Error> {
        for param in params {
            self =
                bind_value(self, param).map_err(|e| Error::QueryParameterFailure(e.to_string()))?;
        }
        Ok(self)
    }
}

fn bind_value(
    query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>,
    value: DbValue,
) -> Result<sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>, String> {
    match value {
        DbValue::Primitive(v) => bind_value_primitive(query, v),
        DbValue::Array(_) => Err("Array param not supported".to_string()),
    }
}

fn bind_value_primitive(
    query: sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>,
    value: DbValuePrimitive,
) -> Result<sqlx::query::Query<sqlx::MySql, sqlx::mysql::MySqlArguments>, String> {
    match value {
        DbValuePrimitive::Int8(v) => Ok(query.bind(v)),
        DbValuePrimitive::Int16(v) => Ok(query.bind(v)),
        DbValuePrimitive::Int32(v) => Ok(query.bind(v)),
        DbValuePrimitive::Int64(v) => Ok(query.bind(v)),
        DbValuePrimitive::Decimal(v) => Ok(query.bind(v)),
        DbValuePrimitive::Float(v) => Ok(query.bind(v)),
        DbValuePrimitive::Boolean(v) => Ok(query.bind(v)),
        DbValuePrimitive::Text(v) => Ok(query.bind(v)),
        DbValuePrimitive::Blob(v) => Ok(query.bind(v)),
        DbValuePrimitive::Uuid(v) => Ok(query.bind(v)),
        DbValuePrimitive::Json(v) => Ok(query.bind(v)),
        // DbValuePrimitive::Xml(v) => Ok(query.bind(v)),
        DbValuePrimitive::Timestamp(v) => {
            Ok(query.bind(chrono::DateTime::from_timestamp_millis(v)))
        }
        // DbValuePrimitive::Interval(v) => Ok(query.bind(chrono::Duration::milliseconds(v))),
        DbValuePrimitive::DbNull => Ok(query.bind(None::<String>)),
        _ => Err(format!("Unsupported value: {:?}", value)),
    }
}

impl TryFrom<&sqlx::mysql::MySqlRow> for DbRow {
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
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Boolean(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::TINYINT => {
            let v: Option<i8> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Int8(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::SMALLINT => {
            let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Int16(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::INT => {
            let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Int32(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::BIGINT => {
            let v: Option<i64> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Int64(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::FLOAT => {
            let v: Option<f32> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Float(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::DOUBLE => {
            let v: Option<f64> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Double(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::TEXT | mysql_type_name::VARCHAR | mysql_type_name::CHAR => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Text(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::JSON => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Json(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::ENUM => {
            let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Text(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        mysql_type_name::VARBINARY | mysql_type_name::BINARY | mysql_type_name::BLOB => {
            let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Blob(v)),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        // mysql_type_name::UUID => {
        //     let v: Option<Uuid> = row.try_get(index).map_err(|e| e.to_string())?;
        //     match v {
        //         Some(v) => DbValue::Primitive(DbValuePrimitive::Uuid(v)),
        //         None => DbValue::Primitive(DbValuePrimitive::DbNull),
        //     }
        // }
        mysql_type_name::TIMESTAMP | mysql_type_name::DATETIME => {
            let v: Option<chrono::DateTime<chrono::Utc>> =
                row.try_get(index).map_err(|e| e.to_string())?;
            match v {
                Some(v) => DbValue::Primitive(DbValuePrimitive::Timestamp(v.timestamp_millis())),
                None => DbValue::Primitive(DbValuePrimitive::DbNull),
            }
        }
        _ => Err(format!("Unsupported type: {:?}", type_name))?,
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
            mysql_type_name::BOOLEAN => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Boolean)),
            mysql_type_name::TINYINT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int8)),
            mysql_type_name::SMALLINT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int16)),
            mysql_type_name::INT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int32)),
            mysql_type_name::BIGINT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int64)),
            mysql_type_name::DECIMAL => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Decimal)),
            mysql_type_name::FLOAT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float)),
            mysql_type_name::DOUBLE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Double)),
            // mysql_type_name::UUID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Uuid)),
            mysql_type_name::TEXT | mysql_type_name::VARCHAR | mysql_type_name::CHAR => {
                Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Text))
            }
            mysql_type_name::JSON => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Json)),
            // mysql_type_name::XML => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Xml)),
            mysql_type_name::TIMESTAMP | mysql_type_name::DATETIME => {
                Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamp))
            }
            mysql_type_name::DATE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Date)),
            mysql_type_name::TIME => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Time)),
            mysql_type_name::VARBINARY | mysql_type_name::BINARY | mysql_type_name::BLOB => {
                Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Blob))
            }
            mysql_type_name::ENUM => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Text)),
            _ => Err(format!("Unsupported type: {:?}", value)),
        }
    }
}

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
