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
    Composite, CompositeType, DbColumn, DbColumnType, DbValue, Domain, DomainType, Enum, EnumType,
    Interval, NamedType, TimeTz, ValuesRange,
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
        DbValue::Character(v) => setter.try_set_value(v),
        DbValue::Int2(v) => setter.try_set_value(v),
        DbValue::Int4(v) => setter.try_set_value(v),
        DbValue::Int8(v) => setter.try_set_value(v),
        DbValue::Float4(v) => setter.try_set_value(v),
        DbValue::Float8(v) => setter.try_set_value(v),
        DbValue::Numeric(v) => setter.try_set_value(v),
        DbValue::Boolean(v) => setter.try_set_value(v),
        DbValue::Text(v) => setter.try_set_value(v),
        DbValue::Varchar(v) => setter.try_set_value(v),
        DbValue::Bpchar(v) => setter.try_set_value(v),
        DbValue::Bytea(v) => setter.try_set_value(v),
        DbValue::Uuid(v) => setter.try_set_value(v),
        DbValue::Json(v) => setter.try_set_value(v),
        DbValue::Jsonb(v) => setter.try_set_value(v),
        DbValue::Jsonpath(v) => setter.try_set_value(PgJsonPath(v)),
        DbValue::Xml(v) => setter.try_set_value(PgXml(v)),
        DbValue::Timestamp(v) => setter.try_set_value(v),
        DbValue::Timestamptz(v) => setter.try_set_value(v),
        DbValue::Time(v) => setter.try_set_value(v),
        DbValue::Timetz(v) => setter.try_set_value(PgTimeTz::from(v)),
        DbValue::Date(v) => setter.try_set_value(v),
        DbValue::Interval(v) => setter.try_set_value(PgInterval::from(v)),
        DbValue::Inet(v) => setter.try_set_value(v),
        DbValue::Cidr(v) => setter.try_set_value(v),
        DbValue::Macaddr(v) => setter.try_set_value(v),
        DbValue::Bit(v) => setter.try_set_value(v),
        DbValue::Varbit(v) => setter.try_set_value(v),
        DbValue::Int4range(v) => setter.try_set_value(PgRange::from(v)),
        DbValue::Int8range(v) => setter.try_set_value(PgRange::from(v)),
        DbValue::Numrange(v) => setter.try_set_value(PgRange::from(v)),
        DbValue::Tsrange(v) => setter.try_set_value(PgRange::from(v)),
        DbValue::Tstzrange(v) => setter.try_set_value(PgRange::from(v)),
        DbValue::Daterange(v) => setter.try_set_value(PgRange::from(v)),
        DbValue::Money(v) => setter.try_set_value(PgMoney(v)),
        DbValue::Oid(v) => setter.try_set_value(Oid(v)),
        DbValue::Enum(v) => setter.try_set_value(v),
        DbValue::Composite(v) => setter.try_set_value(v),
        DbValue::Domain(v) => setter.try_set_value(v),
        DbValue::Array(vs) => set_value_array(setter, vs),
        DbValue::Null => setter.try_set_value(PgNull {}),
    }
}

fn set_value_array<'a, S: PgValueSetter<'a>>(
    setter: &mut S,
    values: Vec<DbValue>,
) -> Result<(), String> {
    if values.is_empty() {
        setter.try_set_value(PgNull {})
    } else {
        let first = &values[0];
        match first {
            DbValue::Character(_) => {
                let values: Vec<i8> = get_plain_values(values, |v| {
                    if let DbValue::Character(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Int2(_) => {
                let values: Vec<i16> = get_plain_values(values, |v| {
                    if let DbValue::Int2(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Int4(_) => {
                let values: Vec<i32> = get_plain_values(values, |v| {
                    if let DbValue::Int4(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Int8(_) => {
                let values: Vec<i64> = get_plain_values(values, |v| {
                    if let DbValue::Int8(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Numeric(_) => {
                let values: Vec<BigDecimal> = get_plain_values(values, |v| {
                    if let DbValue::Numeric(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Float4(_) => {
                let values: Vec<f32> = get_plain_values(values, |v| {
                    if let DbValue::Float4(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }

            DbValue::Float8(_) => {
                let values: Vec<f64> = get_plain_values(values, |v| {
                    if let DbValue::Float8(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Boolean(_) => {
                let values: Vec<bool> = get_plain_values(values, |v| {
                    if let DbValue::Boolean(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Text(_) => {
                let values: Vec<String> = get_plain_values(values, |v| {
                    if let DbValue::Text(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Varchar(_) => {
                let values: Vec<String> = get_plain_values(values, |v| {
                    if let DbValue::Varchar(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Bpchar(_) => {
                let values: Vec<String> = get_plain_values(values, |v| {
                    if let DbValue::Bpchar(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Bytea(_) => {
                let values: Vec<Vec<u8>> = get_plain_values(values, |v| {
                    if let DbValue::Bytea(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Uuid(_) => {
                let values: Vec<Uuid> = get_plain_values(values, |v| {
                    if let DbValue::Uuid(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Json(_) => {
                let values: Vec<serde_json::Value> = get_plain_values(values, |v| {
                    if let DbValue::Json(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Jsonb(_) => {
                let values: Vec<serde_json::Value> = get_plain_values(values, |v| {
                    if let DbValue::Jsonb(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Jsonpath(_) => {
                let values: Vec<PgJsonPath> = get_plain_values(values, |v| {
                    if let DbValue::Jsonpath(v) = v {
                        Some(PgJsonPath(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Xml(_) => {
                let values: Vec<PgXml> = get_plain_values(values, |v| {
                    if let DbValue::Xml(v) = v {
                        Some(PgXml(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Timestamptz(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Timestamptz(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Timestamp(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Timestamp(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Date(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Date(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Time(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Time(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Timetz(_) => {
                let values: Vec<PgTimeTz> = get_plain_values(values, |v| {
                    if let DbValue::Timetz(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Interval(_) => {
                let values: Vec<PgInterval> = get_plain_values(values, |v| {
                    if let DbValue::Interval(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Inet(_) => {
                let values: Vec<IpAddr> = get_plain_values(values, |v| {
                    if let DbValue::Inet(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Cidr(_) => {
                let values: Vec<IpAddr> = get_plain_values(values, |v| {
                    if let DbValue::Cidr(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Macaddr(_) => {
                let values: Vec<MacAddress> = get_plain_values(values, |v| {
                    if let DbValue::Macaddr(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Bit(_) => {
                let values: Vec<BitVec> = get_plain_values(values, |v| {
                    if let DbValue::Bit(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Varbit(_) => {
                let values: Vec<BitVec> = get_plain_values(values, |v| {
                    if let DbValue::Varbit(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Int4range(_) => {
                let values: Vec<PgRange<i32>> = get_plain_values(values, |v| {
                    if let DbValue::Int4range(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Int8range(_) => {
                let values: Vec<PgRange<i64>> = get_plain_values(values, |v| {
                    if let DbValue::Int8range(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Numrange(_) => {
                let values: Vec<PgRange<BigDecimal>> = get_plain_values(values, |v| {
                    if let DbValue::Numrange(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Tsrange(_) => {
                let values: Vec<PgRange<chrono::NaiveDateTime>> = get_plain_values(values, |v| {
                    if let DbValue::Tsrange(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Tstzrange(_) => {
                let values: Vec<PgRange<chrono::DateTime<chrono::Utc>>> =
                    get_plain_values(values, |v| {
                        if let DbValue::Tstzrange(v) = v {
                            Some(v.into())
                        } else {
                            None
                        }
                    })?;
                setter.try_set_value(values)
            }
            DbValue::Daterange(_) => {
                let values: Vec<PgRange<chrono::NaiveDate>> = get_plain_values(values, |v| {
                    if let DbValue::Daterange(v) = v {
                        Some(v.into())
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Oid(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Oid(v) = v {
                        Some(Oid(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Money(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Money(v) = v {
                        Some(PgMoney(v))
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(values)
            }
            DbValue::Enum(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Enum(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(PgEnums(values))
            }
            DbValue::Composite(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Composite(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(PgComposites(values))
            }
            DbValue::Domain(_) => {
                let values: Vec<_> = get_plain_values(values, |v| {
                    if let DbValue::Domain(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                setter.try_set_value(PgDomains(values))
            }
            DbValue::Array(_) => Err("Array of arrays is not supported".to_string()),
            DbValue::Null => Err(format!(
                "Array param element '{}' with index 0 is not supported",
                first
            )),
        }
    }
}

fn get_plain_values<T>(
    values: Vec<DbValue>,
    f: impl Fn(DbValue) -> Option<T>,
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
    let value = match db_type {
        DbColumnType::Boolean => {
            let v: Option<bool> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Boolean))
        }
        DbColumnType::Character => {
            let v: Option<i8> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Character))
        }
        DbColumnType::Int2 => {
            let v: Option<i16> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Int2))
        }
        DbColumnType::Int4 => {
            let v: Option<i32> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Int4))
        }
        DbColumnType::Int8 => {
            let v: Option<i64> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Int8))
        }
        DbColumnType::Float4 => {
            let v: Option<f32> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Float4))
        }
        DbColumnType::Float8 => {
            let v: Option<f64> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Float8))
        }
        DbColumnType::Numeric => {
            let v: Option<BigDecimal> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Numeric))
        }
        DbColumnType::Text => {
            let v: Option<String> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Text))
        }
        DbColumnType::Varchar => {
            let v: Option<String> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Varchar))
        }
        DbColumnType::Bpchar => {
            let v: Option<String> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Bpchar))
        }
        DbColumnType::Json => {
            let v: Option<serde_json::Value> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Json))
        }
        DbColumnType::Jsonb => {
            let v: Option<serde_json::Value> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Jsonb))
        }
        DbColumnType::Jsonpath => {
            let v: Option<PgJsonPath> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Jsonpath(v.into())))
        }
        DbColumnType::Xml => {
            let v: Option<PgXml> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Xml(v.0)))
        }
        DbColumnType::Bytea => {
            let v: Option<Vec<u8>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Bytea))
        }
        DbColumnType::Uuid => {
            let v: Option<Uuid> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Uuid))
        }
        DbColumnType::Interval => {
            let v: Option<PgInterval> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Interval(v.into())))
        }
        DbColumnType::Timestamp => {
            let v: Option<chrono::NaiveDateTime> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Timestamp))
        }
        DbColumnType::Timestamptz => {
            let v: Option<chrono::DateTime<chrono::Utc>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Timestamptz))
        }
        DbColumnType::Date => {
            let v: Option<chrono::NaiveDate> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Date))
        }
        DbColumnType::Time => {
            let v: Option<chrono::NaiveTime> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Time))
        }
        DbColumnType::Timetz => {
            let v: Option<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>> =
                getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Timetz(v.into())))
        }
        DbColumnType::Inet => {
            let v: Option<IpAddr> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Inet))
        }
        DbColumnType::Cidr => {
            let v: Option<IpAddr> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Cidr))
        }
        DbColumnType::Macaddr => {
            let v: Option<MacAddress> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Macaddr))
        }
        DbColumnType::Bit => {
            let v: Option<BitVec> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Bit))
        }
        DbColumnType::Varbit => {
            let v: Option<BitVec> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Varbit))
        }
        DbColumnType::Int4range => {
            let v: Option<PgRange<i32>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Int4range(v.into())))
        }
        DbColumnType::Int8range => {
            let v: Option<PgRange<i64>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Int8range(v.into())))
        }
        DbColumnType::Numrange => {
            let v: Option<PgRange<BigDecimal>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Numrange(v.into())))
        }
        DbColumnType::Tsrange => {
            let v: Option<PgRange<chrono::NaiveDateTime>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Tsrange(v.into())))
        }
        DbColumnType::Tstzrange => {
            let v: Option<PgRange<chrono::DateTime<chrono::Utc>>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Tstzrange(v.into())))
        }
        DbColumnType::Daterange => {
            let v: Option<PgRange<chrono::NaiveDate>> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Daterange(v.into())))
        }
        DbColumnType::Oid => {
            let v: Option<Oid> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Oid(v.0)))
        }
        DbColumnType::Money => {
            let v: Option<PgMoney> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(|v| DbValue::Money(v.0)))
        }
        DbColumnType::Enum(_) => {
            let v: Option<Enum> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Enum))
        }
        DbColumnType::Composite(_) => {
            let v: Option<Composite> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Composite))
        }
        DbColumnType::Domain(_) => {
            let v: Option<Domain> = getter.try_get_value()?;
            DbValue::primitive_from(v.map(DbValue::Domain))
        }
        DbColumnType::Array(v) => get_db_value_array(v, getter)?,
    };
    Ok(value)
}

fn get_db_value_array<G: PgValueGetter>(
    db_type: &DbColumnType,
    getter: &mut G,
) -> Result<DbValue, String> {
    let value = match db_type {
        DbColumnType::Boolean => {
            let vs: Option<Vec<bool>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Boolean)
        }
        DbColumnType::Character => {
            let vs: Option<Vec<i8>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Character)
        }
        DbColumnType::Int2 => {
            let vs: Option<Vec<i16>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Int2)
        }
        DbColumnType::Int4 => {
            let vs: Option<Vec<i32>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Int4)
        }
        DbColumnType::Int8 => {
            let vs: Option<Vec<i64>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Int8)
        }
        DbColumnType::Float4 => {
            let vs: Option<Vec<f32>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Float4)
        }
        DbColumnType::Float8 => {
            let vs: Option<Vec<f64>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Float8)
        }
        DbColumnType::Numeric => {
            let vs: Option<Vec<BigDecimal>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Numeric)
        }
        DbColumnType::Text => {
            let vs: Option<Vec<String>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Text)
        }
        DbColumnType::Varchar => {
            let vs: Option<Vec<String>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Varchar)
        }
        DbColumnType::Bpchar => {
            let vs: Option<Vec<String>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Bpchar)
        }
        DbColumnType::Json => {
            let vs: Option<Vec<serde_json::Value>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Json)
        }
        DbColumnType::Jsonb => {
            let vs: Option<Vec<serde_json::Value>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Jsonb)
        }
        DbColumnType::Jsonpath => {
            let vs: Option<Vec<PgJsonPath>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Jsonpath(v.into()))
        }
        DbColumnType::Xml => {
            let vs: Option<Vec<PgXml>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Xml(v.0))
        }
        DbColumnType::Bytea => {
            let vs: Option<Vec<Vec<u8>>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Bytea)
        }
        DbColumnType::Uuid => {
            let vs: Option<Vec<Uuid>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Uuid)
        }
        DbColumnType::Interval => {
            let vs: Option<Vec<PgInterval>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Interval(v.into()))
        }
        DbColumnType::Timestamp => {
            let vs: Option<Vec<chrono::NaiveDateTime>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Timestamp)
        }
        DbColumnType::Timestamptz => {
            let vs: Option<Vec<chrono::DateTime<chrono::Utc>>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Timestamptz)
        }
        DbColumnType::Date => {
            let vs: Option<Vec<chrono::NaiveDate>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Date)
        }
        DbColumnType::Time => {
            let vs: Option<Vec<chrono::NaiveTime>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Time)
        }
        DbColumnType::Timetz => {
            let vs: Option<Vec<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>>> =
                getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Timetz(v.into()))
        }
        DbColumnType::Inet => {
            let vs: Option<Vec<IpAddr>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Inet)
        }
        DbColumnType::Cidr => {
            let vs: Option<Vec<IpAddr>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Cidr)
        }
        DbColumnType::Macaddr => {
            let vs: Option<Vec<MacAddress>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Macaddr)
        }
        DbColumnType::Bit => {
            let vs: Option<Vec<BitVec>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Bit)
        }
        DbColumnType::Varbit => {
            let vs: Option<Vec<BitVec>> = getter.try_get_value()?;
            DbValue::array_from(vs, DbValue::Varbit)
        }
        DbColumnType::Int4range => {
            let vs: Option<Vec<PgRange<i32>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Int4range(v.into()))
        }
        DbColumnType::Int8range => {
            let vs: Option<Vec<PgRange<i64>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Int8range(v.into()))
        }
        DbColumnType::Numrange => {
            let vs: Option<Vec<PgRange<BigDecimal>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Numrange(v.into()))
        }
        DbColumnType::Tsrange => {
            let vs: Option<Vec<PgRange<chrono::NaiveDateTime>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Tsrange(v.into()))
        }
        DbColumnType::Tstzrange => {
            let vs: Option<Vec<PgRange<chrono::DateTime<chrono::Utc>>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Tstzrange(v.into()))
        }
        DbColumnType::Daterange => {
            let vs: Option<Vec<PgRange<chrono::NaiveDate>>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Daterange(v.into()))
        }
        DbColumnType::Money => {
            let vs: Option<Vec<PgMoney>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Money(v.0))
        }
        DbColumnType::Oid => {
            let vs: Option<Vec<Oid>> = getter.try_get_value()?;
            DbValue::array_from(vs, |v| DbValue::Oid(v.0))
        }
        DbColumnType::Enum(_) => {
            let vs: Option<PgEnums> = getter.try_get_value()?;
            DbValue::array_from(vs.map(|v| v.0), DbValue::Enum)
        }
        DbColumnType::Composite(_) => {
            let vs: Option<PgComposites> = getter.try_get_value()?;
            DbValue::array_from(vs.map(|v| v.0), DbValue::Composite)
        }
        DbColumnType::Domain(_) => {
            let vs: Option<PgDomains> = getter.try_get_value()?;
            DbValue::array_from(vs.map(|v| v.0), DbValue::Domain)
        }
        DbColumnType::Array(_) => Err("Array of arrays is not supported".to_string())?,
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
        pg_type_name::BOOL => Ok(DbColumnType::Boolean),
        pg_type_name::CHAR => Ok(DbColumnType::Character),
        pg_type_name::INT2 => Ok(DbColumnType::Int2),
        pg_type_name::INT4 => Ok(DbColumnType::Int4),
        pg_type_name::INT8 => Ok(DbColumnType::Int8),
        pg_type_name::NUMERIC => Ok(DbColumnType::Numeric),
        pg_type_name::FLOAT4 => Ok(DbColumnType::Float4),
        pg_type_name::FLOAT8 => Ok(DbColumnType::Float8),
        pg_type_name::UUID => Ok(DbColumnType::Uuid),
        pg_type_name::TEXT => Ok(DbColumnType::Text),
        pg_type_name::VARCHAR => Ok(DbColumnType::Varchar),
        pg_type_name::BPCHAR => Ok(DbColumnType::Bpchar),
        pg_type_name::JSON => Ok(DbColumnType::Json),
        pg_type_name::JSONB => Ok(DbColumnType::Jsonb),
        pg_type_name::JSONPATH => Ok(DbColumnType::Jsonpath),
        pg_type_name::XML => Ok(DbColumnType::Xml),
        pg_type_name::TIMESTAMP => Ok(DbColumnType::Timestamp),
        pg_type_name::TIMESTAMPTZ => Ok(DbColumnType::Timestamptz),
        pg_type_name::DATE => Ok(DbColumnType::Date),
        pg_type_name::TIME => Ok(DbColumnType::Time),
        pg_type_name::TIMETZ => Ok(DbColumnType::Timetz),
        pg_type_name::INTERVAL => Ok(DbColumnType::Interval),
        pg_type_name::BYTEA => Ok(DbColumnType::Bytea),
        pg_type_name::INET => Ok(DbColumnType::Inet),
        pg_type_name::CIDR => Ok(DbColumnType::Cidr),
        pg_type_name::MACADDR => Ok(DbColumnType::Macaddr),
        pg_type_name::BIT => Ok(DbColumnType::Bit),
        pg_type_name::VARBIT => Ok(DbColumnType::Varbit),
        pg_type_name::OID => Ok(DbColumnType::Oid),
        pg_type_name::MONEY => Ok(DbColumnType::Money),
        pg_type_name::INT4RANGE => Ok(DbColumnType::Int4range),
        pg_type_name::INT8RANGE => Ok(DbColumnType::Int8range),
        pg_type_name::NUMRANGE => Ok(DbColumnType::Numrange),
        pg_type_name::TSRANGE => Ok(DbColumnType::Tsrange),
        pg_type_name::TSTZRANGE => Ok(DbColumnType::Tstzrange),
        pg_type_name::DATERANGE => Ok(DbColumnType::Daterange),
        pg_type_name::CHAR_ARRAY => Ok(DbColumnType::Character.into_array()),
        pg_type_name::BOOL_ARRAY => Ok(DbColumnType::Boolean.into_array()),
        pg_type_name::INT2_ARRAY => Ok(DbColumnType::Int2.into_array()),
        pg_type_name::INT4_ARRAY => Ok(DbColumnType::Int4.into_array()),
        pg_type_name::INT8_ARRAY => Ok(DbColumnType::Int8.into_array()),
        pg_type_name::NUMERIC_ARRAY => Ok(DbColumnType::Numeric.into_array()),
        pg_type_name::FLOAT4_ARRAY => Ok(DbColumnType::Float4.into_array()),
        pg_type_name::FLOAT8_ARRAY => Ok(DbColumnType::Float8.into_array()),
        pg_type_name::UUID_ARRAY => Ok(DbColumnType::Uuid.into_array()),
        pg_type_name::TEXT_ARRAY => Ok(DbColumnType::Text.into_array()),
        pg_type_name::VARCHAR_ARRAY => Ok(DbColumnType::Varchar.into_array()),
        pg_type_name::BPCHAR_ARRAY => Ok(DbColumnType::Bpchar.into_array()),
        pg_type_name::JSON_ARRAY => Ok(DbColumnType::Json.into_array()),
        pg_type_name::JSONB_ARRAY => Ok(DbColumnType::Jsonb.into_array()),
        pg_type_name::JSONPATH_ARRAY => Ok(DbColumnType::Jsonpath.into_array()),
        pg_type_name::XML_ARRAY => Ok(DbColumnType::Xml.into_array()),
        pg_type_name::TIMESTAMP_ARRAY => Ok(DbColumnType::Timestamp.into_array()),
        pg_type_name::TIMESTAMPTZ_ARRAY => Ok(DbColumnType::Timestamptz.into_array()),
        pg_type_name::DATE_ARRAY => Ok(DbColumnType::Date.into_array()),
        pg_type_name::TIME_ARRAY => Ok(DbColumnType::Time.into_array()),
        pg_type_name::TIMETZ_ARRAY => Ok(DbColumnType::Timetz.into_array()),
        pg_type_name::INTERVAL_ARRAY => Ok(DbColumnType::Interval.into_array()),
        pg_type_name::BYTEA_ARRAY => Ok(DbColumnType::Bytea.into_array()),
        pg_type_name::INET_ARRAY => Ok(DbColumnType::Inet.into_array()),
        pg_type_name::CIDR_ARRAY => Ok(DbColumnType::Cidr.into_array()),
        pg_type_name::MACADDR_ARRAY => Ok(DbColumnType::Macaddr.into_array()),
        pg_type_name::BIT_ARRAY => Ok(DbColumnType::Bit.into_array()),
        pg_type_name::VARBIT_ARRAY => Ok(DbColumnType::Varbit.into_array()),
        pg_type_name::OID_ARRAY => Ok(DbColumnType::Oid.into_array()),
        pg_type_name::MONEY_ARRAY => Ok(DbColumnType::Money.into_array()),
        pg_type_name::INT4RANGE_ARRAY => Ok(DbColumnType::Int4range.into_array()),
        pg_type_name::INT8RANGE_ARRAY => Ok(DbColumnType::Int8range.into_array()),
        pg_type_name::NUMRANGE_ARRAY => Ok(DbColumnType::Numrange.into_array()),
        pg_type_name::TSRANGE_ARRAY => Ok(DbColumnType::Tsrange.into_array()),
        pg_type_name::TSTZRANGE_ARRAY => Ok(DbColumnType::Tstzrange.into_array()),
        pg_type_name::DATERANGE_ARRAY => Ok(DbColumnType::Daterange.into_array()),
        _ => match type_kind {
            PgTypeKind::Enum(_) => Ok(DbColumnType::Enum(EnumType::new(type_name))),
            PgTypeKind::Composite(attributes) => {
                let attributes = get_db_column_type_attributes(attributes.to_vec())?;
                Ok(DbColumnType::Composite(CompositeType::new(
                    type_name, attributes,
                )))
            }
            PgTypeKind::Domain(base_type) => {
                let base_type = get_db_column_type(base_type)?;
                Ok(DbColumnType::Domain(DomainType::new(type_name, base_type)))
            }
            PgTypeKind::Array(element_type)
                if matches!(
                    element_type.kind(),
                    PgTypeKind::Enum(_) | PgTypeKind::Domain(_) | PgTypeKind::Composite(_)
                ) =>
            {
                let column_type = get_db_column_type(element_type)?;
                Ok(column_type.into_array())
            }
            _ => Err(format!("Column type '{}' is not supported", type_name))?,
        },
    }
}

impl<T> From<ValuesRange<T>> for PgRange<T> {
    fn from(range: ValuesRange<T>) -> Self {
        PgRange {
            start: range.start,
            end: range.end,
        }
    }
}

impl<T> From<PgRange<T>> for ValuesRange<T> {
    fn from(range: PgRange<T>) -> Self {
        ValuesRange {
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
        PgTypeKind::Array(element_type)
            if matches!(
                element_type.kind(),
                PgTypeKind::Enum(_) | PgTypeKind::Composite(_) | PgTypeKind::Domain(_)
            ) =>
        {
            format!("{}[]", element_type.name())
        }
        PgTypeKind::Array(element_type) => {
            format!("{}[]", element_type.name().to_uppercase())
        }
        _ => type_info.name().to_uppercase(),
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
