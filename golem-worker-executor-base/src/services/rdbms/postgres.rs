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

pub(crate) const POSTGRES: &str = "postgres";

#[derive(Debug, Clone, Default)]
pub struct PostgresType;

impl PostgresType {
    pub fn new_rdbms(config: RdbmsConfig) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        sqlx_rdbms::new(config)
    }
}

impl RdbmsType for PostgresType {
    type DbColumn = types::DbColumn;
    type DbValue = types::DbValue;
}

impl Display for PostgresType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", POSTGRES)
    }
}

pub(crate) mod sqlx_rdbms {
    use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
    use crate::services::rdbms::postgres::types::{
        get_plain_values, DbColumn, DbColumnType, DbColumnTypePrimitive, DbValue, DbValuePrimitive,
    };
    use crate::services::rdbms::postgres::{PostgresType, POSTGRES};
    use crate::services::rdbms::sqlx_common::{
        PoolCreator, QueryExecutor, QueryParamsBinder, SqlxRdbms, StreamDbResultSet,
    };
    use crate::services::rdbms::{DbResultSet, DbRow, Error, Rdbms, RdbmsPoolKey};
    use async_trait::async_trait;
    use bigdecimal::BigDecimal;
    use futures_util::stream::BoxStream;
    use sqlx::postgres::types::{Oid, PgInterval, PgRange, PgTimeTz};
    use sqlx::postgres::{PgConnectOptions, PgTypeKind};
    use sqlx::types::BitVec;
    use sqlx::{Column, ConnectOptions, Pool, Row, TypeInfo};
    use std::net::IpAddr;
    use std::ops::Bound;
    use std::sync::Arc;
    use uuid::Uuid;

    pub(crate) fn new(config: RdbmsConfig) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        let sqlx: SqlxRdbms<PostgresType, sqlx::postgres::Postgres> = SqlxRdbms::new(config);
        Arc::new(sqlx)
    }

    #[async_trait]
    impl PoolCreator<sqlx::Postgres> for RdbmsPoolKey {
        async fn create_pool(
            &self,
            config: &RdbmsPoolConfig,
        ) -> Result<Pool<sqlx::Postgres>, Error> {
            if self.address.scheme() != POSTGRES && self.address.scheme() != "postgresql" {
                Err(Error::ConnectionFailure(format!(
                    "scheme '{}' in url is invalid",
                    self.address.scheme()
                )))?
            }
            let options = PgConnectOptions::from_url(&self.address)
                .map_err(|e| Error::ConnectionFailure(e.to_string()))?;
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(config.max_connections)
                .connect_with(options)
                .await
                .map_err(|e| Error::ConnectionFailure(e.to_string()))
        }
    }

    #[async_trait]
    impl QueryExecutor<PostgresType> for sqlx::Pool<sqlx::Postgres> {
        async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, Error> {
            let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
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
        ) -> Result<Arc<dyn DbResultSet<PostgresType> + Send + Sync>, Error> {
            let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
                sqlx::query(statement.to_string().leak()).bind_params(params)?;

            let stream: BoxStream<Result<sqlx::postgres::PgRow, sqlx::Error>> = query.fetch(self);

            let response: StreamDbResultSet<PostgresType, sqlx::postgres::Postgres> =
                StreamDbResultSet::create(stream, batch).await?;
            Ok(Arc::new(response))
        }
    }

    impl<'q> QueryParamsBinder<'q, PostgresType, sqlx::Postgres>
        for sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>
    {
        fn bind_params(
            mut self,
            params: Vec<DbValue>,
        ) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, Error>
        {
            for param in params {
                self = bind_value(self, param).map_err(Error::QueryParameterFailure)?;
            }
            Ok(self)
        }
    }

    fn bind_value(
        query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        value: DbValue,
    ) -> Result<sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>, String> {
        match value {
            DbValue::Primitive(v) => bind_value_primitive(query, v),
            DbValue::Array(vs) if !vs.is_empty() => {
                let first = &vs[0];
                match first {
                    DbValuePrimitive::Character(_) => {
                        let values: Vec<i8> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Character(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Int2(_) => {
                        let values: Vec<i16> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Int2(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Int4(_) => {
                        let values: Vec<i32> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Int4(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Int8(_) => {
                        let values: Vec<i64> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Int8(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Numeric(_) => {
                        let values: Vec<BigDecimal> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Numeric(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Float4(_) => {
                        let values: Vec<f32> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Float4(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }

                    DbValuePrimitive::Float8(_) => {
                        let values: Vec<f64> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Float8(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Boolean(_) => {
                        let values: Vec<bool> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Boolean(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Text(_) => {
                        let values: Vec<String> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Text(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Varchar(_) => {
                        let values: Vec<String> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Varchar(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Bpchar(_) => {
                        let values: Vec<String> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Bpchar(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Bytea(_) => {
                        let values: Vec<Vec<u8>> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Bytea(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Uuid(_) => {
                        let values: Vec<Uuid> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Uuid(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Json(_) => {
                        let values: Vec<serde_json::Value> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Json(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Jsonb(_) => {
                        let values: Vec<serde_json::Value> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Jsonb(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Xml(_) => {
                        let values: Vec<String> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Xml(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Timestamptz(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Timestamptz(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Timestamp(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Timestamp(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Date(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Date(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Time(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Time(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Timetz(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Timetz((v, o)) = v {
                                Some(PgTimeTz { time: v, offset: o })
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Interval(_) => {
                        let values: Vec<chrono::Duration> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Interval(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Inet(_) => {
                        let values: Vec<IpAddr> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Inet(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Bit(_) => {
                        let values: Vec<BitVec> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Bit(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Varbit(_) => {
                        let values: Vec<BitVec> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Varbit(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Int4range(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Int4range(v) = v {
                                Some(get_range(v))
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Int8range(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Int8range(v) = v {
                                Some(get_range(v))
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Numrange(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Numrange(v) = v {
                                Some(get_range(v))
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Tsrange(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Tsrange(v) = v {
                                Some(get_range(v))
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Tstzrange(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Tstzrange(v) = v {
                                Some(get_range(v))
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Oid(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Oid(v) = v {
                                Some(Oid(v))
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::CustomEnum(_) => {
                        let values: Vec<_> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::CustomEnum(v) = v {
                                Some(v)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    DbValuePrimitive::Null => {
                        let values: Vec<Option<String>> = get_plain_values(vs, |v| {
                            if let DbValuePrimitive::Null = v {
                                Some(None)
                            } else {
                                None
                            }
                        })?;
                        Ok(query.bind(values))
                    }
                    _ => Err(format!(
                        "Array param element '{}' with index 0 is not supported",
                        first
                    )),
                }
            }
            _ => Ok(query),
        }
    }

    fn bind_value_primitive(
        query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>,
        value: DbValuePrimitive,
    ) -> Result<sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments>, String> {
        match value {
            DbValuePrimitive::Character(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int2(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int4(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int8(v) => Ok(query.bind(v)),
            DbValuePrimitive::Float4(v) => Ok(query.bind(v)),
            DbValuePrimitive::Float8(v) => Ok(query.bind(v)),
            DbValuePrimitive::Numeric(v) => Ok(query.bind(v)),
            DbValuePrimitive::Boolean(v) => Ok(query.bind(v)),
            DbValuePrimitive::Text(v) => Ok(query.bind(v)),
            DbValuePrimitive::Varchar(v) => Ok(query.bind(v)),
            DbValuePrimitive::Bpchar(v) => Ok(query.bind(v)),
            DbValuePrimitive::Bytea(v) => Ok(query.bind(v)),
            DbValuePrimitive::Uuid(v) => Ok(query.bind(v)),
            DbValuePrimitive::Json(v) => Ok(query.bind(v)),
            DbValuePrimitive::Jsonb(v) => Ok(query.bind(v)),
            DbValuePrimitive::Xml(v) => Ok(query.bind(v)),
            DbValuePrimitive::Timestamp(v) => Ok(query.bind(v)),
            DbValuePrimitive::Timestamptz(v) => Ok(query.bind(v)),
            DbValuePrimitive::Time(v) => Ok(query.bind(v)),
            DbValuePrimitive::Timetz((v, o)) => Ok(query.bind(PgTimeTz { time: v, offset: o })),
            DbValuePrimitive::Date(v) => Ok(query.bind(v)),
            DbValuePrimitive::Interval(v) => Ok(query.bind(v)),
            DbValuePrimitive::Inet(v) => Ok(query.bind(v)),
            DbValuePrimitive::Bit(v) => Ok(query.bind(v)),
            DbValuePrimitive::Varbit(v) => Ok(query.bind(v)),
            DbValuePrimitive::Int4range(v) => Ok(query.bind(get_range(v))),
            DbValuePrimitive::Int8range(v) => Ok(query.bind(get_range(v))),
            DbValuePrimitive::Numrange(v) => Ok(query.bind(get_range(v))),
            DbValuePrimitive::Tsrange(v) => Ok(query.bind(get_range(v))),
            DbValuePrimitive::Tstzrange(v) => Ok(query.bind(get_range(v))),
            DbValuePrimitive::Daterange(v) => Ok(query.bind(get_range(v))),
            DbValuePrimitive::Oid(v) => Ok(query.bind(Oid(v))),
            DbValuePrimitive::CustomEnum(v) => Ok(query.bind(v)),
            DbValuePrimitive::Null => Ok(query.bind(None::<String>)),
            // _ => Err(format!("Type '{}' is not supported", value)),
        }
    }

    impl TryFrom<&sqlx::postgres::PgRow> for DbRow<DbValue> {
        type Error = String;

        fn try_from(value: &sqlx::postgres::PgRow) -> Result<Self, Self::Error> {
            let count = value.len();
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(get_db_value(index, value)?);
            }
            Ok(DbRow { values })
        }
    }

    fn get_db_value(index: usize, row: &sqlx::postgres::PgRow) -> Result<DbValue, String> {
        let column = &row.columns()[index];
        let type_name = column.type_info().name();
        let value = match type_name {
            pg_type_name::BOOL => {
                let v: Option<bool> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Boolean(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::CHAR => {
                let v: Option<i8> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Character(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INT2 => {
                let v: Option<i16> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int2(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INT4 => {
                let v: Option<i32> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int4(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INT8 => {
                let v: Option<i64> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int8(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::FLOAT4 => {
                let v: Option<f32> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Float4(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::FLOAT8 => {
                let v: Option<f64> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Float8(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TEXT => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Text(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::VARCHAR => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Varchar(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::BPCHAR => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Bpchar(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::JSON => {
                let v: Option<serde_json::Value> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Json(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::JSONB => {
                let v: Option<serde_json::Value> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Jsonb(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::XML => {
                let v: Option<String> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Xml(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::BYTEA => {
                let v: Option<Vec<u8>> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Bytea(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::UUID => {
                let v: Option<Uuid> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Uuid(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INTERVAL => {
                let v: Option<PgInterval> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => {
                        let d = get_duration(v)?;
                        DbValue::Primitive(DbValuePrimitive::Interval(d))
                    }
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TIMESTAMP => {
                let v: Option<chrono::DateTime<chrono::Utc>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Timestamp(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TIMESTAMPTZ => {
                let v: Option<chrono::DateTime<chrono::Utc>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Timestamptz(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::DATE => {
                let v: Option<chrono::NaiveDate> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Date(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TIME => {
                let v: Option<chrono::NaiveTime> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Time(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TIMETZ => {
                let v: Option<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Timetz((v.time, v.offset))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INET => {
                let v: Option<IpAddr> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Inet(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::BIT => {
                let v: Option<BitVec> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Bit(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::VARBIT => {
                let v: Option<BitVec> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Varbit(v)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INT4RANGE => {
                let v: Option<PgRange<i32>> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int4range(get_bounds(v))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::INT8RANGE => {
                let v: Option<PgRange<i64>> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Int8range(get_bounds(v))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::NUMRANGE => {
                let v: Option<PgRange<BigDecimal>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Numrange(get_bounds(v))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TSRANGE => {
                let v: Option<PgRange<chrono::DateTime<chrono::Utc>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Tsrange(get_bounds(v))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::TSTZRANGE => {
                let v: Option<PgRange<chrono::DateTime<chrono::Utc>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Tstzrange(get_bounds(v))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::DATERANGE => {
                let v: Option<PgRange<chrono::NaiveDate>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Daterange(get_bounds(v))),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::OID => {
                let v: Option<Oid> = row.try_get(index).map_err(|e| e.to_string())?;
                match v {
                    Some(v) => DbValue::Primitive(DbValuePrimitive::Oid(v.0)),
                    None => DbValue::Primitive(DbValuePrimitive::Null),
                }
            }
            pg_type_name::BOOL_ARRAY => {
                let vs: Option<Vec<bool>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Boolean).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INT2_ARRAY => {
                let vs: Option<Vec<i16>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Int2).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INT4_ARRAY => {
                let vs: Option<Vec<i32>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Int4).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INT8_ARRAY => {
                let vs: Option<Vec<i64>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Int8).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::FLOAT4_ARRAY => {
                let vs: Option<Vec<f32>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Float4).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::FLOAT8_ARRAY => {
                let vs: Option<Vec<f64>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Float8).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TEXT_ARRAY => {
                let vs: Option<Vec<String>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Text).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::VARCHAR_ARRAY => {
                let vs: Option<Vec<String>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Varchar).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::BPCHAR_ARRAY => {
                let vs: Option<Vec<String>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Bpchar).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::JSON_ARRAY => {
                let vs: Option<Vec<serde_json::Value>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Json).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::JSONB_ARRAY => {
                let vs: Option<Vec<serde_json::Value>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Jsonb).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::XML_ARRAY => {
                let vs: Option<Vec<String>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(vs.into_iter().map(DbValuePrimitive::Xml).collect()),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::BYTEA_ARRAY => {
                let vs: Option<Vec<Vec<u8>>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Bytea).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::UUID_ARRAY => {
                let vs: Option<Vec<Uuid>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Uuid).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INTERVAL_ARRAY => {
                let vs: Option<Vec<PgInterval>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        let mut values = Vec::with_capacity(vs.len());
                        for v in vs.into_iter() {
                            let d = get_duration(v)?;
                            values.push(DbValuePrimitive::Interval(d));
                        }
                        DbValue::Array(values)
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TIMESTAMP_ARRAY => {
                let vs: Option<Vec<chrono::DateTime<chrono::Utc>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Timestamp).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TIMESTAMPTZ_ARRAY => {
                let vs: Option<Vec<chrono::DateTime<chrono::Utc>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Timestamptz).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::DATE_ARRAY => {
                let vs: Option<Vec<chrono::NaiveDate>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Date).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TIME_ARRAY => {
                let vs: Option<Vec<chrono::NaiveTime>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Time).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TIMETZ_ARRAY => {
                let vs: Option<Vec<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|t| DbValuePrimitive::Timetz((t.time, t.offset)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INET_ARRAY => {
                let vs: Option<Vec<IpAddr>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Inet).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::BIT_ARRAY => {
                let vs: Option<Vec<BitVec>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(vs.into_iter().map(DbValuePrimitive::Bit).collect()),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::VARBIT_ARRAY => {
                let vs: Option<Vec<BitVec>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(DbValuePrimitive::Varbit).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INT4RANGE_ARRAY => {
                let vs: Option<Vec<PgRange<i32>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|v| DbValuePrimitive::Int4range(get_bounds(v)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::INT8RANGE_ARRAY => {
                let vs: Option<Vec<PgRange<i64>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|v| DbValuePrimitive::Int8range(get_bounds(v)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::NUMRANGE_ARRAY => {
                let vs: Option<Vec<PgRange<BigDecimal>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|v| DbValuePrimitive::Numrange(get_bounds(v)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TSRANGE_ARRAY => {
                let vs: Option<Vec<PgRange<chrono::DateTime<chrono::Utc>>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|v| DbValuePrimitive::Tsrange(get_bounds(v)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::TSTZRANGE_ARRAY => {
                let vs: Option<Vec<PgRange<chrono::DateTime<chrono::Utc>>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|v| DbValuePrimitive::Tstzrange(get_bounds(v)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::DATERANGE_ARRAY => {
                let vs: Option<Vec<PgRange<chrono::NaiveDate>>> =
                    row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => DbValue::Array(
                        vs.into_iter()
                            .map(|v| DbValuePrimitive::Daterange(get_bounds(v)))
                            .collect(),
                    ),
                    None => DbValue::Array(vec![]),
                }
            }
            pg_type_name::OID_ARRAY => {
                let vs: Option<Vec<Oid>> = row.try_get(index).map_err(|e| e.to_string())?;
                match vs {
                    Some(vs) => {
                        DbValue::Array(vs.into_iter().map(|v| DbValuePrimitive::Oid(v.0)).collect())
                    }
                    None => DbValue::Array(vec![]),
                }
            }
            _ => match column.type_info().kind() {
                // enum in postgres is custom type
                PgTypeKind::Enum(_) => {
                    let v: Option<PgEnum> = row.try_get(index).map_err(|e| e.to_string())?;
                    match v {
                        Some(v) => DbValue::Primitive(DbValuePrimitive::CustomEnum(v.0)),
                        None => DbValue::Primitive(DbValuePrimitive::Null),
                    }
                }
                PgTypeKind::Array(element) if matches!(element.kind(), PgTypeKind::Enum(_)) => {
                    let vs: Option<Vec<PgEnum>> = row.try_get(index).map_err(|e| e.to_string())?;
                    match vs {
                        Some(vs) => DbValue::Array(
                            vs.into_iter()
                                .map(|v| DbValuePrimitive::CustomEnum(v.0))
                                .collect(),
                        ),
                        None => DbValue::Array(vec![]),
                    }
                }
                _ => Err(format!("Type '{}' is not supported", type_name))?,
            },
            // _ => Err(format!("Type '{}' is not supported", type_name))?,
        };
        Ok(value)
    }

    impl TryFrom<&sqlx::postgres::PgColumn> for DbColumn {
        type Error = String;

        fn try_from(value: &sqlx::postgres::PgColumn) -> Result<Self, Self::Error> {
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

    impl TryFrom<&sqlx::postgres::PgTypeInfo> for DbColumnType {
        type Error = String;

        fn try_from(value: &sqlx::postgres::PgTypeInfo) -> Result<Self, Self::Error> {
            let type_name = value.name();
            let type_kind: &PgTypeKind = value.kind();

            match type_name {
                pg_type_name::BOOL => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Boolean)),
                pg_type_name::CHAR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Character)),
                pg_type_name::INT2 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int2)),
                pg_type_name::INT4 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int4)),
                pg_type_name::INT8 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int8)),
                pg_type_name::NUMERIC => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Numeric))
                }
                pg_type_name::FLOAT4 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float4)),
                pg_type_name::FLOAT8 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float8)),
                pg_type_name::UUID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Uuid)),
                pg_type_name::TEXT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Text)),
                pg_type_name::VARCHAR => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Varchar))
                }
                pg_type_name::BPCHAR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Bpchar)),
                pg_type_name::JSON => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Json)),
                pg_type_name::JSONB => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Jsonb)),
                pg_type_name::XML => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Xml)),
                pg_type_name::TIMESTAMP => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamp))
                }
                pg_type_name::TIMESTAMPTZ => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamptz))
                }
                pg_type_name::DATE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Date)),
                pg_type_name::TIME => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Time)),
                pg_type_name::TIMETZ => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timetz)),
                pg_type_name::INTERVAL => {
                    Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Interval))
                }
                pg_type_name::BYTEA => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Bytea)),
                pg_type_name::INET => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Inet)),
                pg_type_name::BIT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Bit)),
                pg_type_name::VARBIT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Varbit)),
                pg_type_name::OID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Oid)),
                pg_type_name::CHAR_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Character))
                }
                pg_type_name::BOOL_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Boolean)),
                pg_type_name::INT2_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int2)),
                pg_type_name::INT4_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int4)),
                pg_type_name::INT8_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int8)),
                pg_type_name::NUMERIC_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Numeric))
                }
                pg_type_name::FLOAT4_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Float4))
                }
                pg_type_name::FLOAT8_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Float8))
                }
                pg_type_name::UUID_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Uuid)),
                pg_type_name::TEXT_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Text)),
                pg_type_name::VARCHAR_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Varchar))
                }
                pg_type_name::BPCHAR_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Bpchar))
                }
                pg_type_name::JSON_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Json)),
                pg_type_name::JSONB_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Jsonb)),
                pg_type_name::XML_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Xml)),
                pg_type_name::TIMESTAMP_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Timestamp))
                }
                pg_type_name::TIMESTAMPTZ_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Timestamptz))
                }
                pg_type_name::DATE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Date)),
                pg_type_name::TIME_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Time)),
                pg_type_name::TIMETZ_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Timetz))
                }
                pg_type_name::INTERVAL_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Interval))
                }
                pg_type_name::BYTEA_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Bytea)),
                pg_type_name::INET_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Inet)),
                pg_type_name::BIT_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Bit)),
                pg_type_name::VARBIT_ARRAY => {
                    Ok(DbColumnType::Array(DbColumnTypePrimitive::Varbit))
                }
                pg_type_name::OID_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Oid)),
                _ => match type_kind {
                    PgTypeKind::Enum(_) => Ok(DbColumnType::Primitive(
                        DbColumnTypePrimitive::CustomEnum(type_name.to_string()),
                    )),
                    PgTypeKind::Array(element) if matches!(element.kind(), PgTypeKind::Enum(_)) => {
                        Ok(DbColumnType::Array(DbColumnTypePrimitive::CustomEnum(
                            type_name.to_string(),
                        )))
                    }
                    _ => Err(format!("Type '{}' is not supported", type_name))?,
                },
            }
        }
    }

    fn get_duration(interval: PgInterval) -> Result<chrono::Duration, String> {
        if interval.months != 0 {
            Err("postgres 'INTERVAL' with months is not supported".to_string())
        } else {
            let mut d = chrono::Duration::days(interval.days as i64);
            d += chrono::Duration::microseconds(interval.microseconds);
            Ok(d)
        }
    }

    fn get_bounds<T>(range: PgRange<T>) -> (Bound<T>, Bound<T>) {
        (range.start, range.end)
    }

    fn get_range<T>(bounds: (Bound<T>, Bound<T>)) -> PgRange<T> {
        PgRange {
            start: bounds.0,
            end: bounds.1,
        }
    }

    struct PgEnum(String);

    impl From<PgEnum> for String {
        fn from(value: PgEnum) -> Self {
            value.0
        }
    }

    impl sqlx::types::Type<sqlx::Postgres> for PgEnum {
        fn type_info() -> sqlx::postgres::PgTypeInfo {
            <&str as sqlx::types::Type<sqlx::Postgres>>::type_info()
        }

        fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
            matches!(ty.kind(), PgTypeKind::Enum(_))
        }
    }

    impl sqlx::postgres::PgHasArrayType for PgEnum {
        fn array_type_info() -> sqlx::postgres::PgTypeInfo {
            <&str as sqlx::postgres::PgHasArrayType>::array_type_info()
        }

        fn array_compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
            matches!(ty.kind(), PgTypeKind::Array(ty) if <PgEnum as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
        }
    }

    impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgEnum {
        fn decode(
            value: sqlx::postgres::PgValueRef<'r>,
        ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
            Ok(Self(<String as sqlx::Decode<sqlx::Postgres>>::decode(
                value,
            )?))
        }
    }

    /// sqlx::postgres::type_info::PgType is not publicly accessible.
    ///
    #[allow(dead_code)]
    pub(crate) mod pg_type_name {
        pub(crate) const BOOL: &str = "BOOL";
        pub(crate) const BYTEA: &str = "BYTEA";
        pub(crate) const CHAR: &str = "\"CHAR\"";
        pub(crate) const NAME: &str = "NAME";
        pub(crate) const INT8: &str = "INT8";
        pub(crate) const INT2: &str = "INT2";
        pub(crate) const INT4: &str = "INT4";
        pub(crate) const TEXT: &str = "TEXT";
        pub(crate) const OID: &str = "OID";
        pub(crate) const JSON: &str = "JSON";
        pub(crate) const JSON_ARRAY: &str = "JSON[]";
        pub(crate) const POINT: &str = "POINT";
        pub(crate) const LSEG: &str = "LSEG";
        pub(crate) const PATH: &str = "PATH";
        pub(crate) const BOX: &str = "BOX";
        pub(crate) const POLYGON: &str = "POLYGON";
        pub(crate) const LINE: &str = "LINE";
        pub(crate) const LINE_ARRAY: &str = "LINE[]";
        pub(crate) const CIDR: &str = "CIDR";
        pub(crate) const CIDR_ARRAY: &str = "CIDR[]";
        pub(crate) const FLOAT4: &str = "FLOAT4";
        pub(crate) const FLOAT8: &str = "FLOAT8";
        pub(crate) const UNKNOWN: &str = "UNKNOWN";
        pub(crate) const CIRCLE: &str = "CIRCLE";
        pub(crate) const CIRCLE_ARRAY: &str = "CIRCLE[]";
        pub(crate) const MACADDR8: &str = "MACADDR8";
        pub(crate) const MACADDR8_ARRAY: &str = "MACADDR8[]";
        pub(crate) const MACADDR: &str = "MACADDR";
        pub(crate) const INET: &str = "INET";
        pub(crate) const BOOL_ARRAY: &str = "BOOL[]";
        pub(crate) const BYTEA_ARRAY: &str = "BYTEA[]";
        pub(crate) const CHAR_ARRAY: &str = "\"CHAR\"[]";
        pub(crate) const NAME_ARRAY: &str = "NAME[]";
        pub(crate) const INT2_ARRAY: &str = "INT2[]";
        pub(crate) const INT4_ARRAY: &str = "INT4[]";
        pub(crate) const TEXT_ARRAY: &str = "TEXT[]";
        pub(crate) const BPCHAR_ARRAY: &str = "CHAR[]";
        pub(crate) const VARCHAR_ARRAY: &str = "VARCHAR[]";
        pub(crate) const INT8_ARRAY: &str = "INT8[]";
        pub(crate) const POINT_ARRAY: &str = "POINT[]";
        pub(crate) const LSEG_ARRAY: &str = "LSEG[]";
        pub(crate) const PATH_ARRAY: &str = "PATH[]";
        pub(crate) const BOX_ARRAY: &str = "BOX[]";
        pub(crate) const FLOAT4_ARRAY: &str = "FLOAT4[]";
        pub(crate) const FLOAT8_ARRAY: &str = "FLOAT8[]";
        pub(crate) const POLYGON_ARRAY: &str = "POLYGON[]";
        pub(crate) const OID_ARRAY: &str = "OID[]";
        pub(crate) const MACADDR_ARRAY: &str = "MACADDR[]";
        pub(crate) const INET_ARRAY: &str = "INET[]";
        pub(crate) const BPCHAR: &str = "CHAR";
        pub(crate) const VARCHAR: &str = "VARCHAR";
        pub(crate) const DATE: &str = "DATE";
        pub(crate) const TIME: &str = "TIME";
        pub(crate) const TIMESTAMP: &str = "TIMESTAMP";
        pub(crate) const TIMESTAMP_ARRAY: &str = "TIMESTAMP[]";
        pub(crate) const DATE_ARRAY: &str = "DATE[]";
        pub(crate) const TIME_ARRAY: &str = "TIME[]";
        pub(crate) const TIMESTAMPTZ: &str = "TIMESTAMPTZ";
        pub(crate) const TIMESTAMPTZ_ARRAY: &str = "TIMESTAMPTZ[]";
        pub(crate) const INTERVAL: &str = "INTERVAL";
        pub(crate) const INTERVAL_ARRAY: &str = "INTERVAL[]";
        pub(crate) const NUMERIC_ARRAY: &str = "NUMERIC[]";
        pub(crate) const TIMETZ: &str = "TIMETZ";
        pub(crate) const TIMETZ_ARRAY: &str = "TIMETZ[]";
        pub(crate) const BIT: &str = "BIT";
        pub(crate) const BIT_ARRAY: &str = "BIT[]";
        pub(crate) const VARBIT: &str = "VARBIT";
        pub(crate) const VARBIT_ARRAY: &str = "VARBIT[]";
        pub(crate) const NUMERIC: &str = "NUMERIC";
        pub(crate) const RECORD: &str = "RECORD";
        pub(crate) const RECORD_ARRAY: &str = "RECORD[]";
        pub(crate) const UUID: &str = "UUID";
        pub(crate) const UUID_ARRAY: &str = "UUID[]";
        pub(crate) const JSONB: &str = "JSONB";
        pub(crate) const JSONB_ARRAY: &str = "JSONB[]";
        pub(crate) const INT4RANGE: &str = "INT4RANGE";
        pub(crate) const INT4RANGE_ARRAY: &str = "INT4RANGE[]";
        pub(crate) const NUMRANGE: &str = "NUMRANGE";
        pub(crate) const NUMRANGE_ARRAY: &str = "NUMRANGE[]";
        pub(crate) const TSRANGE: &str = "TSRANGE";
        pub(crate) const TSRANGE_ARRAY: &str = "TSRANGE[]";
        pub(crate) const TSTZRANGE: &str = "TSTZRANGE";
        pub(crate) const TSTZRANGE_ARRAY: &str = "TSTZRANGE[]";
        pub(crate) const DATERANGE: &str = "DATERANGE";
        pub(crate) const DATERANGE_ARRAY: &str = "DATERANGE[]";
        pub(crate) const INT8RANGE: &str = "INT8RANGE";
        pub(crate) const INT8RANGE_ARRAY: &str = "INT8RANGE[]";
        pub(crate) const JSONPATH: &str = "JSONPATH";
        pub(crate) const JSONPATH_ARRAY: &str = "JSONPATH[]";
        pub(crate) const MONEY: &str = "MONEY";
        pub(crate) const MONEY_ARRAY: &str = "MONEY[]";
        pub(crate) const VOID: &str = "VOID";
        pub(crate) const XML: &str = "XML";
        pub(crate) const XML_ARRAY: &str = "XML_ARRAY";
    }
}

pub mod types {
    use bigdecimal::BigDecimal;
    use itertools::Itertools;
    use sqlx::types::BitVec;
    use std::fmt::Display;
    use std::net::IpAddr;
    use std::ops::Bound;
    use uuid::Uuid;

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum DbColumnTypePrimitive {
        Character,
        Int2,
        Int4,
        Int8,
        Float4,
        Float8,
        Numeric,
        Boolean,
        Text,
        Varchar,
        Bpchar,
        Timestamp,
        Timestamptz,
        Date,
        Time,
        Timetz,
        Interval,
        Bytea,
        Uuid,
        Xml,
        Json,
        Jsonb,
        Inet,
        Bit,
        Varbit,
        Int4range,
        Int8range,
        Numrange,
        Tsrange,
        Tstzrange,
        Daterange,
        CustomEnum(String),
        Oid,
    }

    impl Display for DbColumnTypePrimitive {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DbColumnTypePrimitive::Character => write!(f, "char"),
                DbColumnTypePrimitive::Int2 => write!(f, "int2"),
                DbColumnTypePrimitive::Int4 => write!(f, "int4"),
                DbColumnTypePrimitive::Int8 => write!(f, "int8"),
                DbColumnTypePrimitive::Float4 => write!(f, "float4"),
                DbColumnTypePrimitive::Float8 => write!(f, "float8"),
                DbColumnTypePrimitive::Numeric => write!(f, "numeric"),
                DbColumnTypePrimitive::Boolean => write!(f, "boolean"),
                DbColumnTypePrimitive::Timestamp => write!(f, "timestamp"),
                DbColumnTypePrimitive::Date => write!(f, "date"),
                DbColumnTypePrimitive::Time => write!(f, "time"),
                DbColumnTypePrimitive::Timestamptz => write!(f, "timestamptz"),
                DbColumnTypePrimitive::Timetz => write!(f, "timetz"),
                DbColumnTypePrimitive::Interval => write!(f, "interval"),
                DbColumnTypePrimitive::Text => write!(f, "text"),
                DbColumnTypePrimitive::Varchar => write!(f, "varchar"),
                DbColumnTypePrimitive::Bpchar => write!(f, "bpchar"),
                DbColumnTypePrimitive::Bytea => write!(f, "bytea"),
                DbColumnTypePrimitive::Json => write!(f, "json"),
                DbColumnTypePrimitive::Jsonb => write!(f, "jsonb"),
                DbColumnTypePrimitive::Xml => write!(f, "xml"),
                DbColumnTypePrimitive::Uuid => write!(f, "uuid"),
                DbColumnTypePrimitive::Inet => write!(f, "inet"),
                DbColumnTypePrimitive::Bit => write!(f, "bit"),
                DbColumnTypePrimitive::Varbit => write!(f, "varbit"),
                DbColumnTypePrimitive::Int4range => write!(f, "int4range"),
                DbColumnTypePrimitive::Int8range => write!(f, "int8range"),
                DbColumnTypePrimitive::Numrange => write!(f, "numrange"),
                DbColumnTypePrimitive::Tsrange => write!(f, "tsrange"),
                DbColumnTypePrimitive::Tstzrange => write!(f, "tstzrange"),
                DbColumnTypePrimitive::Daterange => write!(f, "daterange"),
                DbColumnTypePrimitive::Oid => write!(f, "oid"),
                DbColumnTypePrimitive::CustomEnum(v) => write!(f, "custom {}", v),
            }
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum DbColumnType {
        Primitive(DbColumnTypePrimitive),
        Array(DbColumnTypePrimitive),
    }

    impl Display for DbColumnType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DbColumnType::Primitive(v) => write!(f, "{}", v),
                DbColumnType::Array(v) => write!(f, "{}[]", v),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum DbValuePrimitive {
        Character(i8),
        Int2(i16),
        Int4(i32),
        Int8(i64),
        Float4(f32),
        Float8(f64),
        Numeric(BigDecimal),
        Boolean(bool),
        Timestamp(chrono::DateTime<chrono::Utc>),
        Timestamptz(chrono::DateTime<chrono::Utc>),
        Date(chrono::NaiveDate),
        Time(chrono::NaiveTime),
        Timetz((chrono::NaiveTime, chrono::FixedOffset)),
        Interval(chrono::Duration),
        Text(String),
        Varchar(String),
        Bpchar(String),
        Bytea(Vec<u8>),
        Json(serde_json::Value),
        Jsonb(serde_json::Value),
        Xml(String),
        Uuid(Uuid),
        Inet(IpAddr),
        Bit(BitVec),
        Varbit(BitVec),
        Int4range((Bound<i32>, Bound<i32>)),
        Int8range((Bound<i64>, Bound<i64>)),
        Numrange((Bound<BigDecimal>, Bound<BigDecimal>)),
        Tsrange(
            (
                Bound<chrono::DateTime<chrono::Utc>>,
                Bound<chrono::DateTime<chrono::Utc>>,
            ),
        ),
        Tstzrange(
            (
                Bound<chrono::DateTime<chrono::Utc>>,
                Bound<chrono::DateTime<chrono::Utc>>,
            ),
        ),
        Daterange((Bound<chrono::NaiveDate>, Bound<chrono::NaiveDate>)),
        CustomEnum(String),
        Oid(u32),
        Null,
    }

    impl Display for DbValuePrimitive {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DbValuePrimitive::Character(v) => write!(f, "{}", v),
                DbValuePrimitive::Int2(v) => write!(f, "{}", v),
                DbValuePrimitive::Int4(v) => write!(f, "{}", v),
                DbValuePrimitive::Int8(v) => write!(f, "{}", v),
                DbValuePrimitive::Float4(v) => write!(f, "{}", v),
                DbValuePrimitive::Float8(v) => write!(f, "{}", v),
                DbValuePrimitive::Numeric(v) => write!(f, "{}", v),
                DbValuePrimitive::Boolean(v) => write!(f, "{}", v),
                DbValuePrimitive::Timestamp(v) => write!(f, "{}", v),
                DbValuePrimitive::Timestamptz(v) => write!(f, "{}", v),
                DbValuePrimitive::Date(v) => write!(f, "{}", v),
                DbValuePrimitive::Time(v) => write!(f, "{}", v),
                DbValuePrimitive::Timetz(v) => write!(f, "{} {}", v.0, v.1),
                DbValuePrimitive::Interval(v) => write!(f, "{}", v),
                DbValuePrimitive::Text(v) => write!(f, "{}", v),
                DbValuePrimitive::Varchar(v) => write!(f, "{}", v),
                DbValuePrimitive::Bpchar(v) => write!(f, "{}", v),
                DbValuePrimitive::Bytea(v) => write!(f, "{:?}", v),
                DbValuePrimitive::Json(v) => write!(f, "{}", v),
                DbValuePrimitive::Jsonb(v) => write!(f, "{}", v),
                DbValuePrimitive::Xml(v) => write!(f, "{}", v),
                DbValuePrimitive::Uuid(v) => write!(f, "{}", v),
                DbValuePrimitive::Inet(v) => write!(f, "{}", v),
                DbValuePrimitive::Bit(v) => write!(f, "{:?}", v),
                DbValuePrimitive::Varbit(v) => write!(f, "{:?}", v),
                DbValuePrimitive::Int4range(v) => write!(f, "{:?}, {:?}", v.0, v.1),
                DbValuePrimitive::Int8range(v) => write!(f, "{:?}, {:?}", v.0, v.1),
                DbValuePrimitive::Numrange(v) => write!(f, "{:?}, {:?}", v.0, v.1),
                DbValuePrimitive::Tsrange(v) => write!(f, "{:?}, {:?}", v.0, v.1),
                DbValuePrimitive::Tstzrange(v) => write!(f, "{:?}, {:?}", v.0, v.1),
                DbValuePrimitive::Daterange(v) => write!(f, "{:?}, {:?}", v.0, v.1),
                DbValuePrimitive::Oid(v) => write!(f, "{}", v),
                DbValuePrimitive::CustomEnum(v) => write!(f, "{}", v),
                DbValuePrimitive::Null => write!(f, "NULL"),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum DbValue {
        Primitive(DbValuePrimitive),
        Array(Vec<DbValuePrimitive>),
    }

    impl Display for DbValue {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                DbValue::Primitive(v) => write!(f, "{}", v),
                DbValue::Array(v) => write!(f, "[{}]", v.iter().format(", ")),
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

    pub(crate) fn get_plain_values<T>(
        values: Vec<DbValuePrimitive>,
        f: impl Fn(DbValuePrimitive) -> Option<T>,
    ) -> Result<Vec<T>, String> {
        let mut result: Vec<T> = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            if let Some(v) = f(value.clone()) {
                result.push(v);
            } else {
                Err(format!(
                    "Array element '{}' with index {} has different type than expected",
                    value.clone(),
                    index
                ))?
            }
        }
        Ok(result)
    }
}
