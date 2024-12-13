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

use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
use crate::services::rdbms::postgres::types::{
    Composite, CompositeType, DbColumn, DbColumnType, DbColumnTypePrimitive, DbValue,
    DbValuePrimitive, Domain, DomainType, Enum, EnumType, Interval, NamedType, Range, TimeTz,
};
use crate::services::rdbms::postgres::{PostgresType, POSTGRES};
use crate::services::rdbms::sqlx_common::{
    PoolCreator, QueryExecutor, QueryParamsBinder, SqlxRdbms, StreamDbResultSet,
};
use crate::services::rdbms::{DbResultSet, DbRow, Error, Rdbms, RdbmsPoolKey};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use futures_util::stream::BoxStream;
use serde_json::json;
use sqlx::postgres::types::{Oid, PgInterval, PgMoney, PgRange, PgTimeTz};
use sqlx::postgres::{PgConnectOptions, PgTypeKind};
use sqlx::types::mac_address::MacAddress;
use sqlx::types::BitVec;
use sqlx::{Column, ConnectOptions, Pool, Row, Type, TypeInfo, ValueRef};
use std::net::IpAddr;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) fn new(config: RdbmsConfig) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
    let sqlx: SqlxRdbms<PostgresType, sqlx::postgres::Postgres> = SqlxRdbms::new(config);
    Arc::new(sqlx)
}

#[async_trait]
impl PoolCreator<sqlx::Postgres> for RdbmsPoolKey {
    async fn create_pool(&self, config: &RdbmsPoolConfig) -> Result<Pool<sqlx::Postgres>, Error> {
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
    ) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, Error> {
        for param in params {
            set_value(&mut self, param).map_err(Error::QueryParameterFailure)?;
        }
        Ok(self)
    }
}

fn set_value<'a, S: PgValueSetter<'a>>(setter: &mut S, value: DbValue) -> Result<(), String> {
    match value {
        DbValue::Primitive(v) => set_value_primitive(setter, v),
        DbValue::Array(vs) => set_value_array(setter, vs),
    }
}

fn set_value_primitive<'a, S: PgValueSetter<'a>>(
    setter: &mut S,
    value: DbValuePrimitive,
) -> Result<(), String> {
    match value {
        DbValuePrimitive::Character(v) => setter.try_set_value(v),
        DbValuePrimitive::Int2(v) => setter.try_set_value(v),
        DbValuePrimitive::Int4(v) => setter.try_set_value(v),
        DbValuePrimitive::Int8(v) => setter.try_set_value(v),
        DbValuePrimitive::Float4(v) => setter.try_set_value(v),
        DbValuePrimitive::Float8(v) => setter.try_set_value(v),
        DbValuePrimitive::Numeric(v) => setter.try_set_value(v),
        DbValuePrimitive::Boolean(v) => setter.try_set_value(v),
        DbValuePrimitive::Text(v) => setter.try_set_value(v),
        DbValuePrimitive::Varchar(v) => setter.try_set_value(v),
        DbValuePrimitive::Bpchar(v) => setter.try_set_value(v),
        DbValuePrimitive::Bytea(v) => setter.try_set_value(v),
        DbValuePrimitive::Uuid(v) => setter.try_set_value(v),
        DbValuePrimitive::Json(v) => setter.try_set_value(v),
        DbValuePrimitive::Jsonb(v) => setter.try_set_value(v),
        DbValuePrimitive::Jsonpath(v) => setter.try_set_value(PgJsonPath(v)),
        DbValuePrimitive::Xml(v) => setter.try_set_value(PgXml(v)),
        DbValuePrimitive::Timestamp(v) => setter.try_set_value(v),
        DbValuePrimitive::Timestamptz(v) => setter.try_set_value(v),
        DbValuePrimitive::Time(v) => setter.try_set_value(v),
        DbValuePrimitive::Timetz(v) => setter.try_set_value(PgTimeTz::from(v)),
        DbValuePrimitive::Date(v) => setter.try_set_value(v),
        DbValuePrimitive::Interval(v) => setter.try_set_value(PgInterval::from(v)),
        DbValuePrimitive::Inet(v) => setter.try_set_value(v),
        DbValuePrimitive::Cidr(v) => setter.try_set_value(v),
        DbValuePrimitive::Macaddr(v) => setter.try_set_value(v),
        DbValuePrimitive::Bit(v) => setter.try_set_value(v),
        DbValuePrimitive::Varbit(v) => setter.try_set_value(v),
        DbValuePrimitive::Int4range(v) => setter.try_set_value(PgRange::from(v)),
        DbValuePrimitive::Int8range(v) => setter.try_set_value(PgRange::from(v)),
        DbValuePrimitive::Numrange(v) => setter.try_set_value(PgRange::from(v)),
        DbValuePrimitive::Tsrange(v) => setter.try_set_value(PgRange::from(v)),
        DbValuePrimitive::Tstzrange(v) => setter.try_set_value(PgRange::from(v)),
        DbValuePrimitive::Daterange(v) => setter.try_set_value(PgRange::from(v)),
        DbValuePrimitive::Money(v) => setter.try_set_value(PgMoney(v)),
        DbValuePrimitive::Oid(v) => setter.try_set_value(Oid(v)),
        DbValuePrimitive::Enum(v) => setter.try_set_value(v),
        DbValuePrimitive::Composite(v) => setter.try_set_value(v),
        DbValuePrimitive::Domain(v) => setter.try_set_value(v),
        DbValuePrimitive::Null => setter.try_set_value(PgNull {}),
    }
}

fn set_value_array<'a, S: PgValueSetter<'a>>(
    setter: &mut S,
    values: Vec<DbValuePrimitive>,
) -> Result<(), String> {
    if values.is_empty() {
        setter.try_set_value(PgNull {})
    } else {
        let first = &values[0];
        match first {
            DbValuePrimitive::Character(_) => {
                let values: Vec<i8> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Character(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Int2(_) => {
                let values: Vec<i16> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Int2(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Int4(_) => {
                let values: Vec<i32> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Int4(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Int8(_) => {
                let values: Vec<i64> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Int8(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Numeric(_) => {
                let values: Vec<BigDecimal> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Numeric(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Float4(_) => {
                let values: Vec<f32> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Float4(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }

            DbValuePrimitive::Float8(_) => {
                let values: Vec<f64> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Float8(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Boolean(_) => {
                let values: Vec<bool> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Boolean(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Text(_) => {
                let values: Vec<String> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Text(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Varchar(_) => {
                let values: Vec<String> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Varchar(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Bpchar(_) => {
                let values: Vec<String> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Bpchar(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Bytea(_) => {
                let values: Vec<Vec<u8>> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Bytea(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Uuid(_) => {
                let values: Vec<Uuid> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Uuid(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Json(_) => {
                let values: Vec<serde_json::Value> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Json(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Jsonb(_) => {
                let values: Vec<serde_json::Value> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Jsonb(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Jsonpath(_) => {
                let values: Vec<PgJsonPath> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Jsonpath(v) = v {
                        Some(PgJsonPath(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Xml(_) => {
                let values: Vec<PgXml> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Xml(v) = v {
                        Some(PgXml(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Timestamptz(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Timestamptz(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Timestamp(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Timestamp(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Date(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Date(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Time(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Time(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Timetz(_) => {
                let values: Vec<PgTimeTz> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Timetz(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Interval(_) => {
                let values: Vec<PgInterval> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Interval(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Inet(_) => {
                let values: Vec<IpAddr> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Inet(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Cidr(_) => {
                let values: Vec<IpAddr> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Cidr(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Macaddr(_) => {
                let values: Vec<MacAddress> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Macaddr(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Bit(_) => {
                let values: Vec<BitVec> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Bit(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Varbit(_) => {
                let values: Vec<BitVec> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Varbit(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Int4range(_) => {
                let values: Vec<PgRange<i32>> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Int4range(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Int8range(_) => {
                let values: Vec<PgRange<i64>> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Int8range(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Numrange(_) => {
                let values: Vec<PgRange<BigDecimal>> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Numrange(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Tsrange(_) => {
                let values: Vec<PgRange<chrono::NaiveDateTime>> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Tsrange(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Tstzrange(_) => {
                let values: Vec<PgRange<chrono::DateTime<chrono::Utc>>> =
                    get_plain_values(values, |v| {
                        if let DbValuePrimitive::Tstzrange(v) = v {
                            Some(v.into())
                        } else {
                            None
                        }
                    })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Daterange(_) => {
                let values: Vec<PgRange<chrono::NaiveDate>> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Daterange(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Oid(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Oid(v) = v {
                        Some(Oid(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Money(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Money(v) = v {
                        Some(PgMoney(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValuePrimitive::Enum(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Enum(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(PgEnums(values))
            }
            DbValuePrimitive::Composite(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Composite(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(PgComposites(values))
            }
            DbValuePrimitive::Domain(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValuePrimitive::Domain(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(PgDomains(values))
            }
            DbValuePrimitive::Null => Err(format!(
                "Array param element '{}' with index 0 is not supported",
                first
            )),
        }
    }
}

fn get_plain_values<T>(
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

impl TryFrom<&sqlx::postgres::PgRow> for DbRow<DbValue> {
    type Error = String;

    fn try_from(value: &sqlx::postgres::PgRow) -> Result<Self, Self::Error> {
        let count = value.len();
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            values.push(get_column_db_value(index, value)?);
        }
        Ok(DbRow { values })
    }
}

fn get_column_db_value(index: usize, row: &sqlx::postgres::PgRow) -> Result<DbValue, String> {
    let column = &row.columns()[index];
    let db_type: DbColumnType = column.type_info().try_into()?;
    let mut getter = PgRowColumnValueGetter::new(index, row);
    get_db_value(&db_type, &mut getter)
}

fn get_db_value<G: PgValueGetter>(
    db_type: &DbColumnType,
    getter: &mut G,
) -> Result<DbValue, String> {
    match db_type {
        DbColumnType::Primitive(t) => get_db_value_primitive(t, getter),
        DbColumnType::Array(t) => get_db_value_array(t, getter),
    }
}

fn get_db_value_primitive<G: PgValueGetter>(
    db_type: &DbColumnTypePrimitive,
    getter: &mut G,
) -> Result<DbValue, String> {
    let value = match db_type {
        DbColumnTypePrimitive::Boolean => {
            let v: Option<bool> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Boolean))
        }
        DbColumnTypePrimitive::Character => {
            let v: Option<i8> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Character))
        }
        DbColumnTypePrimitive::Int2 => {
            let v: Option<i16> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Int2))
        }
        DbColumnTypePrimitive::Int4 => {
            let v: Option<i32> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Int4))
        }
        DbColumnTypePrimitive::Int8 => {
            let v: Option<i64> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Int8))
        }
        DbColumnTypePrimitive::Float4 => {
            let v: Option<f32> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Float4))
        }
        DbColumnTypePrimitive::Float8 => {
            let v: Option<f64> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Float8))
        }
        DbColumnTypePrimitive::Numeric => {
            let v: Option<BigDecimal> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Numeric))
        }
        DbColumnTypePrimitive::Text => {
            let v: Option<String> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Text))
        }
        DbColumnTypePrimitive::Varchar => {
            let v: Option<String> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Varchar))
        }
        DbColumnTypePrimitive::Bpchar => {
            let v: Option<String> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Bpchar))
        }
        DbColumnTypePrimitive::Json => {
            let v: Option<serde_json::Value> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Json))
        }
        DbColumnTypePrimitive::Jsonb => {
            let v: Option<serde_json::Value> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Jsonb))
        }
        DbColumnTypePrimitive::Jsonpath => {
            let v: Option<PgJsonPath> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Jsonpath(v.into())))
        }
        DbColumnTypePrimitive::Xml => {
            let v: Option<PgXml> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Xml(v.0)))
        }
        DbColumnTypePrimitive::Bytea => {
            let v: Option<Vec<u8>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Bytea))
        }
        DbColumnTypePrimitive::Uuid => {
            let v: Option<Uuid> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Uuid))
        }
        DbColumnTypePrimitive::Interval => {
            let v: Option<PgInterval> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Interval(v.into())))
        }
        DbColumnTypePrimitive::Timestamp => {
            let v: Option<chrono::NaiveDateTime> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Timestamp))
        }
        DbColumnTypePrimitive::Timestamptz => {
            let v: Option<chrono::DateTime<chrono::Utc>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Timestamptz))
        }
        DbColumnTypePrimitive::Date => {
            let v: Option<chrono::NaiveDate> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Date))
        }
        DbColumnTypePrimitive::Time => {
            let v: Option<chrono::NaiveTime> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Time))
        }
        DbColumnTypePrimitive::Timetz => {
            let v: Option<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>> =
                getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Timetz(v.into())))
        }
        DbColumnTypePrimitive::Inet => {
            let v: Option<IpAddr> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Inet))
        }
        DbColumnTypePrimitive::Cidr => {
            let v: Option<IpAddr> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Cidr))
        }
        DbColumnTypePrimitive::Macaddr => {
            let v: Option<MacAddress> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Macaddr))
        }
        DbColumnTypePrimitive::Bit => {
            let v: Option<BitVec> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Bit))
        }
        DbColumnTypePrimitive::Varbit => {
            let v: Option<BitVec> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Varbit))
        }
        DbColumnTypePrimitive::Int4range => {
            let v: Option<PgRange<i32>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Int4range(v.into())))
        }
        DbColumnTypePrimitive::Int8range => {
            let v: Option<PgRange<i64>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Int8range(v.into())))
        }
        DbColumnTypePrimitive::Numrange => {
            let v: Option<PgRange<BigDecimal>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Numrange(v.into())))
        }
        DbColumnTypePrimitive::Tsrange => {
            let v: Option<PgRange<chrono::NaiveDateTime>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Tsrange(v.into())))
        }
        DbColumnTypePrimitive::Tstzrange => {
            let v: Option<PgRange<chrono::DateTime<chrono::Utc>>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Tstzrange(v.into())))
        }
        DbColumnTypePrimitive::Daterange => {
            let v: Option<PgRange<chrono::NaiveDate>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Daterange(v.into())))
        }
        DbColumnTypePrimitive::Oid => {
            let v: Option<Oid> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Oid(v.0)))
        }
        DbColumnTypePrimitive::Money => {
            let v: Option<PgMoney> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValuePrimitive::Money(v.0)))
        }
        DbColumnTypePrimitive::Enum(_) => {
            let v: Option<Enum> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Enum))
        }
        DbColumnTypePrimitive::Composite(_) => {
            let v: Option<Composite> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Composite))
        }
        DbColumnTypePrimitive::Domain(_) => {
            let v: Option<Domain> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValuePrimitive::Domain))
        }
    };
    Ok(value)
}

fn get_db_value_array<G: PgValueGetter>(
    db_type: &DbColumnTypePrimitive,
    getter: &mut G,
) -> Result<DbValue, String> {
    let value = match db_type {
        DbColumnTypePrimitive::Boolean => {
            let vs: Option<Vec<bool>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Boolean)
        }
        DbColumnTypePrimitive::Character => {
            let vs: Option<Vec<i8>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Character)
        }
        DbColumnTypePrimitive::Int2 => {
            let vs: Option<Vec<i16>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Int2)
        }
        DbColumnTypePrimitive::Int4 => {
            let vs: Option<Vec<i32>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Int4)
        }
        DbColumnTypePrimitive::Int8 => {
            let vs: Option<Vec<i64>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Int8)
        }
        DbColumnTypePrimitive::Float4 => {
            let vs: Option<Vec<f32>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Float4)
        }
        DbColumnTypePrimitive::Float8 => {
            let vs: Option<Vec<f64>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Float8)
        }
        DbColumnTypePrimitive::Numeric => {
            let vs: Option<Vec<BigDecimal>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Numeric)
        }
        DbColumnTypePrimitive::Text => {
            let vs: Option<Vec<String>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Text)
        }
        DbColumnTypePrimitive::Varchar => {
            let vs: Option<Vec<String>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Varchar)
        }
        DbColumnTypePrimitive::Bpchar => {
            let vs: Option<Vec<String>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Bpchar)
        }
        DbColumnTypePrimitive::Json => {
            let vs: Option<Vec<serde_json::Value>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Json)
        }
        DbColumnTypePrimitive::Jsonb => {
            let vs: Option<Vec<serde_json::Value>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Jsonb)
        }
        DbColumnTypePrimitive::Jsonpath => {
            let vs: Option<Vec<PgJsonPath>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Jsonpath(v.into()))
        }
        DbColumnTypePrimitive::Xml => {
            let vs: Option<Vec<PgXml>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Xml(v.0))
        }
        DbColumnTypePrimitive::Bytea => {
            let vs: Option<Vec<Vec<u8>>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Bytea)
        }
        DbColumnTypePrimitive::Uuid => {
            let vs: Option<Vec<Uuid>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Uuid)
        }
        DbColumnTypePrimitive::Interval => {
            let vs: Option<Vec<PgInterval>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Interval(v.into()))
        }
        DbColumnTypePrimitive::Timestamp => {
            let vs: Option<Vec<chrono::NaiveDateTime>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Timestamp)
        }
        DbColumnTypePrimitive::Timestamptz => {
            let vs: Option<Vec<chrono::DateTime<chrono::Utc>>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Timestamptz)
        }
        DbColumnTypePrimitive::Date => {
            let vs: Option<Vec<chrono::NaiveDate>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Date)
        }
        DbColumnTypePrimitive::Time => {
            let vs: Option<Vec<chrono::NaiveTime>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Time)
        }
        DbColumnTypePrimitive::Timetz => {
            let vs: Option<Vec<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>>> =
                getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Timetz(v.into()))
        }
        DbColumnTypePrimitive::Inet => {
            let vs: Option<Vec<IpAddr>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Inet)
        }
        DbColumnTypePrimitive::Cidr => {
            let vs: Option<Vec<IpAddr>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Cidr)
        }
        DbColumnTypePrimitive::Macaddr => {
            let vs: Option<Vec<MacAddress>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Macaddr)
        }
        DbColumnTypePrimitive::Bit => {
            let vs: Option<Vec<BitVec>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Bit)
        }
        DbColumnTypePrimitive::Varbit => {
            let vs: Option<Vec<BitVec>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValuePrimitive::Varbit)
        }
        DbColumnTypePrimitive::Int4range => {
            let vs: Option<Vec<PgRange<i32>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Int4range(v.into()))
        }
        DbColumnTypePrimitive::Int8range => {
            let vs: Option<Vec<PgRange<i64>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Int8range(v.into()))
        }
        DbColumnTypePrimitive::Numrange => {
            let vs: Option<Vec<PgRange<BigDecimal>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Numrange(v.into()))
        }
        DbColumnTypePrimitive::Tsrange => {
            let vs: Option<Vec<PgRange<chrono::NaiveDateTime>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Tsrange(v.into()))
        }
        DbColumnTypePrimitive::Tstzrange => {
            let vs: Option<Vec<PgRange<chrono::DateTime<chrono::Utc>>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Tstzrange(v.into()))
        }
        DbColumnTypePrimitive::Daterange => {
            let vs: Option<Vec<PgRange<chrono::NaiveDate>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Daterange(v.into()))
        }
        DbColumnTypePrimitive::Money => {
            let vs: Option<Vec<PgMoney>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Money(v.0))
        }
        DbColumnTypePrimitive::Oid => {
            let vs: Option<Vec<Oid>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValuePrimitive::Oid(v.0))
        }
        DbColumnTypePrimitive::Enum(_) => {
            let vs: Option<PgEnums> = getter.try_get_value()?;
            DbValue::array_from(vs.map(|v| v.0), DbValuePrimitive::Enum)
        }
        DbColumnTypePrimitive::Composite(_) => {
            let vs: Option<PgComposites> = getter.try_get_value()?;
            DbValue::array_from(vs.map(|v| v.0), DbValuePrimitive::Composite)
        }
        DbColumnTypePrimitive::Domain(v) => {
            // println!("domain array- {}: ({})", v.name, v.base_type);
            // let base_type: DbColumnType = *v.base_type.clone();
            // get_db_value(&base_type.into_array(), getter)?
            let vs: Option<PgDomains> = getter.try_get_value()?;
            DbValue::array_from(vs.map(|v| v.0), DbValuePrimitive::Domain)
        }
    };
    Ok(value)
}

impl TryFrom<&sqlx::postgres::PgColumn> for DbColumn {
    type Error = String;

    fn try_from(value: &sqlx::postgres::PgColumn) -> Result<Self, Self::Error> {
        let ordinal = value.ordinal() as u64;
        let db_type: DbColumnType = value.type_info().try_into()?;
        let db_type_name = get_db_type_name(value.type_info());
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
        get_db_column_type(value)
    }
}

fn get_db_column_type(type_info: &sqlx::postgres::PgTypeInfo) -> Result<DbColumnType, String> {
    let type_name = get_db_type_name(type_info);
    let type_kind: &PgTypeKind = type_info.kind();

    match type_name.as_str() {
        pg_type_name::BOOL => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Boolean)),
        pg_type_name::CHAR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Character)),
        pg_type_name::INT2 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int2)),
        pg_type_name::INT4 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int4)),
        pg_type_name::INT8 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int8)),
        pg_type_name::NUMERIC => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Numeric)),
        pg_type_name::FLOAT4 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float4)),
        pg_type_name::FLOAT8 => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Float8)),
        pg_type_name::UUID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Uuid)),
        pg_type_name::TEXT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Text)),
        pg_type_name::VARCHAR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Varchar)),
        pg_type_name::BPCHAR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Bpchar)),
        pg_type_name::JSON => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Json)),
        pg_type_name::JSONB => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Jsonb)),
        pg_type_name::JSONPATH => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Jsonpath)),
        pg_type_name::XML => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Xml)),
        pg_type_name::TIMESTAMP => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamp)),
        pg_type_name::TIMESTAMPTZ => {
            Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timestamptz))
        }
        pg_type_name::DATE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Date)),
        pg_type_name::TIME => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Time)),
        pg_type_name::TIMETZ => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Timetz)),
        pg_type_name::INTERVAL => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Interval)),
        pg_type_name::BYTEA => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Bytea)),
        pg_type_name::INET => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Inet)),
        pg_type_name::CIDR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Cidr)),
        pg_type_name::MACADDR => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Macaddr)),
        pg_type_name::BIT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Bit)),
        pg_type_name::VARBIT => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Varbit)),
        pg_type_name::OID => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Oid)),
        pg_type_name::MONEY => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Money)),
        pg_type_name::INT4RANGE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int4range)),
        pg_type_name::INT8RANGE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Int8range)),
        pg_type_name::NUMRANGE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Numrange)),
        pg_type_name::TSRANGE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Tsrange)),
        pg_type_name::TSTZRANGE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Tstzrange)),
        pg_type_name::DATERANGE => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Daterange)),
        pg_type_name::CHAR_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Character)),
        pg_type_name::BOOL_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Boolean)),
        pg_type_name::INT2_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int2)),
        pg_type_name::INT4_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int4)),
        pg_type_name::INT8_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int8)),
        pg_type_name::NUMERIC_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Numeric)),
        pg_type_name::FLOAT4_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Float4)),
        pg_type_name::FLOAT8_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Float8)),
        pg_type_name::UUID_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Uuid)),
        pg_type_name::TEXT_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Text)),
        pg_type_name::VARCHAR_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Varchar)),
        pg_type_name::BPCHAR_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Bpchar)),
        pg_type_name::JSON_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Json)),
        pg_type_name::JSONB_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Jsonb)),
        pg_type_name::JSONPATH_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Jsonpath)),
        pg_type_name::XML_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Xml)),
        pg_type_name::TIMESTAMP_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Timestamp)),
        pg_type_name::TIMESTAMPTZ_ARRAY => {
            Ok(DbColumnType::Array(DbColumnTypePrimitive::Timestamptz))
        }
        pg_type_name::DATE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Date)),
        pg_type_name::TIME_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Time)),
        pg_type_name::TIMETZ_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Timetz)),
        pg_type_name::INTERVAL_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Interval)),
        pg_type_name::BYTEA_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Bytea)),
        pg_type_name::INET_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Inet)),
        pg_type_name::CIDR_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Cidr)),
        pg_type_name::MACADDR_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Macaddr)),
        pg_type_name::BIT_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Bit)),
        pg_type_name::VARBIT_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Varbit)),
        pg_type_name::OID_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Oid)),
        pg_type_name::MONEY_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Money)),
        pg_type_name::INT4RANGE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int4range)),
        pg_type_name::INT8RANGE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Int8range)),
        pg_type_name::NUMRANGE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Numrange)),
        pg_type_name::TSRANGE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Tsrange)),
        pg_type_name::TSTZRANGE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Tstzrange)),
        pg_type_name::DATERANGE_ARRAY => Ok(DbColumnType::Array(DbColumnTypePrimitive::Daterange)),
        _ => match type_kind {
            PgTypeKind::Enum(_) => Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Enum(
                EnumType::new(type_name),
            ))),
            PgTypeKind::Composite(vs) => {
                let attributes = get_db_column_type_attributes(vs.to_vec())?;
                Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Composite(
                    CompositeType::new(type_name, attributes),
                )))
            }
            PgTypeKind::Domain(t) => {
                let base_type = get_db_column_type(t)?;
                Ok(DbColumnType::Primitive(DbColumnTypePrimitive::Domain(
                    DomainType::new(type_name, base_type),
                )))
            }
            PgTypeKind::Array(element)
                if matches!(
                    element.kind(),
                    PgTypeKind::Enum(_) | PgTypeKind::Domain(_) | PgTypeKind::Composite(_)
                ) =>
            {
                let column_type = get_db_column_type(element)?;
                Ok(column_type.into_array())
            }
            _ => Err(format!("Column type '{}' is not supported", type_name))?,
        },
    }
}

fn get_db_column_type_attributes(
    attributes: Vec<(String, sqlx::postgres::PgTypeInfo)>,
) -> Result<Vec<(String, DbColumnType)>, String> {
    let mut result = Vec::with_capacity(attributes.len());
    for (n, t) in attributes.iter() {
        let t = get_db_column_type(t)?;
        let n = n.to_string();
        result.push((n, t));
    }

    Ok(result)
}

fn get_db_type_name(type_info: &sqlx::postgres::PgTypeInfo) -> String {
    match type_info.kind() {
        PgTypeKind::Enum(_) | PgTypeKind::Composite(_) | PgTypeKind::Domain(_) => {
            type_info.name().to_string()
        }
        PgTypeKind::Array(element)
            if matches!(
                element.kind(),
                PgTypeKind::Enum(_) | PgTypeKind::Composite(_) | PgTypeKind::Domain(_)
            ) =>
        {
            format!("{}[]", element.name())
        }
        PgTypeKind::Array(element) => {
            format!("{}[]", element.name().to_uppercase())
        }
        _ => type_info.name().to_uppercase(),
    }
}

impl<T> From<Range<T>> for PgRange<T> {
    fn from(range: Range<T>) -> Self {
        PgRange {
            start: range.start,
            end: range.end,
        }
    }
}

impl<T> From<PgRange<T>> for Range<T> {
    fn from(range: PgRange<T>) -> Self {
        Range {
            start: range.start,
            end: range.end,
        }
    }
}

impl From<PgInterval> for Interval {
    fn from(interval: PgInterval) -> Self {
        Self {
            months: interval.months,
            days: interval.days,
            microseconds: interval.microseconds,
        }
    }
}

impl From<Interval> for PgInterval {
    fn from(interval: Interval) -> Self {
        Self {
            months: interval.months,
            days: interval.days,
            microseconds: interval.microseconds,
        }
    }
}

impl From<PgTimeTz> for TimeTz {
    fn from(value: PgTimeTz) -> Self {
        Self {
            time: value.time,
            offset: value.offset,
        }
    }
}

impl From<TimeTz> for PgTimeTz {
    fn from(value: TimeTz) -> Self {
        Self {
            time: value.time,
            offset: value.offset,
        }
    }
}

trait PgValueGetter {
    fn try_get_value<T>(&mut self) -> Result<T, String>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + Type<sqlx::Postgres>;
}

trait PgValueSetter<'a> {
    fn try_set_value<T>(&mut self, value: T) -> Result<(), String>
    where
        T: 'a + sqlx::Encode<'a, sqlx::Postgres> + Type<sqlx::Postgres>;
}

impl<'a> PgValueSetter<'a> for sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    fn try_set_value<T>(&mut self, value: T) -> Result<(), String>
    where
        T: 'a + sqlx::Encode<'a, sqlx::Postgres> + Type<sqlx::Postgres>,
    {
        self.try_bind(value).map_err(|e| e.to_string())
    }
}

impl<'a> PgValueSetter<'a> for sqlx::postgres::types::PgRecordEncoder<'a> {
    fn try_set_value<T>(&mut self, value: T) -> Result<(), String>
    where
        T: 'a + sqlx::Encode<'a, sqlx::Postgres> + Type<sqlx::Postgres>,
    {
        let _ = self.encode(value).map_err(|e| e.to_string());
        Ok(())
    }
}

impl PgValueGetter for sqlx::postgres::types::PgRecordDecoder<'_> {
    fn try_get_value<T>(&mut self) -> Result<T, String>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + Type<sqlx::Postgres>,
    {
        self.try_decode::<T>().map_err(|e| e.to_string())
    }
}

impl<'a> PgValueSetter<'a> for sqlx::postgres::PgArgumentBuffer {
    fn try_set_value<T>(&mut self, value: T) -> Result<(), String>
    where
        T: 'a + sqlx::Encode<'a, sqlx::Postgres> + Type<sqlx::Postgres>,
    {
        let _ = <T as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&value, self)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

struct PgRowColumnValueGetter<'r> {
    index: usize,
    row: &'r sqlx::postgres::PgRow,
}

impl<'r> PgRowColumnValueGetter<'r> {
    fn new(index: usize, row: &'r sqlx::postgres::PgRow) -> Self {
        Self { index, row }
    }
}

impl PgValueGetter for PgRowColumnValueGetter<'_> {
    fn try_get_value<T>(&mut self) -> Result<T, String>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + Type<sqlx::Postgres>,
    {
        self.row.try_get(self.index).map_err(|e| e.to_string())
    }
}

struct PgValueRefValueGetter<'r>(sqlx::postgres::PgValueRef<'r>);

impl PgValueGetter for PgValueRefValueGetter<'_> {
    fn try_get_value<T>(&mut self) -> Result<T, String>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + Type<sqlx::Postgres>,
    {
        T::decode(self.0.clone()).map_err(|e| e.to_string())
    }
}

impl sqlx::types::Type<sqlx::Postgres> for Enum {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as sqlx::types::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Enum(_))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Enum {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let type_info = &value.type_info();
        let name = type_info.name().to_string();
        if matches!(type_info.kind(), PgTypeKind::Enum(_)) {
            let v = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
            Ok(Enum::new(name, v))
        } else {
            Err(format!("Type '{}' is not supported", name).into())
        }
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Enum {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.value, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        Some(sqlx::postgres::PgTypeInfo::with_name(
            self.name.clone().leak(),
        ))
    }
}

struct PgEnums(Vec<Enum>);

impl sqlx::Type<sqlx::Postgres> for PgEnums {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277))
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Array(ty) if <Enum as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for PgEnums {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <Vec<Enum> as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        get_array_pg_type_info(&self.0)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgEnums {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let value = <Vec<Enum> as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Self(value))
    }
}

struct PgJsonPath(String);

impl From<PgJsonPath> for String {
    fn from(value: PgJsonPath) -> Self {
        value.0
    }
}

impl sqlx::types::Type<sqlx::Postgres> for PgJsonPath {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(4072))
    }
}

impl sqlx::postgres::PgHasArrayType for PgJsonPath {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(4073))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgJsonPath {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let mut buf = value.as_bytes()?;
        buf = &buf[1..];
        let v: String = serde_json::from_slice(buf)?;
        Ok(PgJsonPath(v))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for PgJsonPath {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        buf.push(1);
        serde_json::to_writer(&mut **buf, &json!(self.0))?;
        Ok(sqlx::encode::IsNull::No)
    }
}

struct PgNull;

impl sqlx::Encode<'_, sqlx::Postgres> for PgNull {
    fn encode_by_ref(
        &self,
        _buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        Ok(sqlx::encode::IsNull::Yes)
    }
}

impl sqlx::types::Type<sqlx::Postgres> for PgNull {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        // https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat
        sqlx::postgres::PgTypeInfo::with_oid(Oid(705)) // unknown type
    }
}

struct PgXml(String);

impl From<PgXml> for String {
    fn from(value: PgXml) -> Self {
        value.0
    }
}

impl sqlx::types::Type<sqlx::Postgres> for PgXml {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        // https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat
        sqlx::postgres::PgTypeInfo::with_oid(Oid(142)) // xml type
    }
}

impl sqlx::postgres::PgHasArrayType for PgXml {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        // https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat
        sqlx::postgres::PgTypeInfo::with_oid(Oid(143)) // xml type array
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgXml {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        Ok(Self(<String as sqlx::Decode<sqlx::Postgres>>::decode(
            value,
        )?))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for PgXml {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }
}

// //https://github.com/launchbadge/sqlx/blob/42ce24dab87aad98f041cafb35cf9a7d5b2b09a7/tests/postgres/postgres.rs#L1241-L1281

impl sqlx::Type<sqlx::Postgres> for Composite {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2249)) // pseudo composite type
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Composite(_))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Composite {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let mut encoder = sqlx::postgres::types::PgRecordEncoder::new(buf);
        for v in self.values.iter() {
            set_value(&mut encoder, v.clone())?;
        }
        encoder.finish();
        Ok(sqlx::encode::IsNull::No)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        Some(sqlx::postgres::PgTypeInfo::with_name(
            self.name.clone().leak(),
        ))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Composite {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let type_info = &value.type_info();
        let name = type_info.name().to_string();
        if let PgTypeKind::Composite(vs) = type_info.kind() {
            let attributes = get_db_column_type_attributes(vs.to_vec())?;
            let mut decoder =
                sqlx::postgres::types::PgRecordDecoder::new(value).map_err(|e| e.to_string())?;
            let mut values: Vec<DbValue> = Vec::with_capacity(attributes.len());
            for (_, db_column_type) in attributes {
                let db_value = get_db_value(&db_column_type, &mut decoder)?;
                values.push(db_value);
            }
            Ok(Composite::new(name, values))
        } else {
            Err(format!("Type '{}' is not supported", name).into())
        }
    }
}

struct PgComposites(Vec<Composite>);

impl sqlx::Type<sqlx::Postgres> for PgComposites {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277)) // pseudo array type
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Array(ty) if <Composite as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for PgComposites {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <Vec<Composite> as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        get_array_pg_type_info(&self.0)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgComposites {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let value = <Vec<Composite> as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Self(value))
    }
}

impl sqlx::Type<sqlx::Postgres> for Domain {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2267)) // pseudo any type
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Domain(_))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Domain {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let v: DbValue = *self.value.clone();
        set_value(buf, v)?;

        Ok(sqlx::encode::IsNull::No)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        Some(sqlx::postgres::PgTypeInfo::with_name(
            self.name.clone().leak(),
        ))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Domain {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let type_info = &value.type_info();
        let name = type_info.name().to_string();
        if let PgTypeKind::Domain(t) = type_info.kind() {
            let db_column_type = get_db_column_type(t)?;
            let mut getter = PgValueRefValueGetter(value);
            let db_value = get_db_value(&db_column_type, &mut getter)?;
            Ok(Domain::new(name, db_value))
        } else {
            Err(format!("Type '{}' is not supported", name).into())
        }
    }
}

struct PgDomains(Vec<Domain>);

impl sqlx::Type<sqlx::Postgres> for PgDomains {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277)) // pseudo array type
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Array(ty) if <Domain as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for PgDomains {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <Vec<Domain> as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        get_array_pg_type_info(&self.0)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PgDomains {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let value = <Vec<Domain> as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Self(value))
    }
}

fn get_array_pg_type_info<T: NamedType>(values: &[T]) -> Option<sqlx::postgres::PgTypeInfo> {
    if values.is_empty() {
        None
    } else {
        let first = &values[0];
        let name = format!("_{}", first.name());
        Some(sqlx::postgres::PgTypeInfo::with_name(name.leak()))
    }
}

/// https://www.postgresql.org/docs/current/datatype.html
/// https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat
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
    pub(crate) const XML_ARRAY: &str = "XML[]";
}
